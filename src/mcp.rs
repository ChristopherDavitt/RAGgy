/// MCP (Model Context Protocol) server — stdio transport.
///
/// Starts a long-lived process that reads newline-delimited JSON-RPC 2.0
/// messages from stdin and writes responses to stdout.  All diagnostic
/// output goes to stderr so it never corrupts the protocol stream.
///
/// Exposed tools:
///   • search  — hybrid BM25 + semantic search with context window
///   • status  — index statistics
///   • graph   — entity co-occurrence connections
///
/// Usage (Claude Desktop config):
///   {
///     "mcpServers": {
///       "raggy": {
///         "command": "raggy",
///         "args": ["mcp"],
///         "env": {}
///       }
///     }
///   }

use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};

use crate::config::Config;
use crate::graph::store::GraphStore;
use crate::index::fulltext::FulltextIndex;
use crate::index::vector::VectorIndex;
use crate::query::{parser, plan};
use crate::workspace::Workspace;

// ── JSON-RPC types ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    /// Absent on notifications — we skip responding in that case.
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run(ws: &Workspace) -> Result<()> {
    if !ws.exists() {
        anyhow::bail!(
            "No index found for database '{}'. Run `raggy index <path>` first.",
            ws.db_name
        );
    }

    let config = ws.config()?;
    let store = GraphStore::open(&ws.db_path())?;
    let fulltext = FulltextIndex::open(&ws.tantivy_dir())?;

    eprintln!("raggy mcp: loading indexes for '{}'…", ws.db_name);

    let mut vector = if config.embedding.enabled {
        let model_dir = ws.embedding_model_dir(&config);
        match VectorIndex::open(&ws.vector_dir(), &model_dir) {
            Ok(v) if !v.is_empty() => {
                eprintln!("raggy mcp: vector index ready ({} vectors)", v.len());
                Some(v)
            }
            _ => {
                eprintln!("raggy mcp: vector index unavailable, using keyword-only search");
                None
            }
        }
    } else {
        None
    };

    eprintln!("raggy mcp: ready");

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in BufReader::new(stdin.lock()).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let req: RpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let err = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {"code": -32700, "message": format!("Parse error: {}", e)}
                });
                writeln!(out, "{}", serde_json::to_string(&err)?)?;
                out.flush()?;
                continue;
            }
        };

        // Notifications have no id — acknowledge but don't respond.
        let is_notification = req.id.as_ref().map_or(true, |v| v.is_null());
        if is_notification {
            continue;
        }

        let resp = dispatch(&req, &store, &fulltext, &mut vector, &config);
        writeln!(out, "{}", serde_json::to_string(&resp)?)?;
        out.flush()?;
    }

    Ok(())
}

// ── Dispatcher ────────────────────────────────────────────────────────────────

fn dispatch(
    req: &RpcRequest,
    store: &GraphStore,
    fulltext: &FulltextIndex,
    vector: &mut Option<VectorIndex>,
    config: &Config,
) -> Value {
    match req.method.as_str() {
        "initialize" => handle_initialize(req),
        "ping" => rpc_ok(&req.id, json!({})),
        "tools/list" => handle_tools_list(req),
        "tools/call" => {
            let params = req.params.as_ref().unwrap_or(&Value::Null);
            let name = params["name"].as_str().unwrap_or("");
            let args = &params["arguments"];
            match name {
                "search" => tool_search(&req.id, args, store, fulltext, vector, config),
                "status" => tool_status(&req.id, store),
                "graph"  => tool_graph(&req.id, args, store),
                _ => rpc_error(&req.id, -32601, &format!("Unknown tool: {}", name)),
            }
        }
        other => rpc_error(&req.id, -32601, &format!("Method not found: {}", other)),
    }
}

// ── MCP method handlers ───────────────────────────────────────────────────────

fn handle_initialize(req: &RpcRequest) -> Value {
    rpc_ok(&req.id, json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": "raggy",
            "version": env!("CARGO_PKG_VERSION")
        }
    }))
}

fn handle_tools_list(req: &RpcRequest) -> Value {
    rpc_ok(&req.id, json!({
        "tools": [
            {
                "name": "search",
                "description": "Search the indexed document corpus using hybrid BM25 + semantic \
                                search. Returns relevant document chunks with surrounding context \
                                so you have enough text to answer questions without a follow-up fetch.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Natural language search query"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of results to return (default: 5)",
                            "default": 5
                        },
                        "window": {
                            "type": "integer",
                            "description": "Sibling chunks to include before and after each match \
                                           for context. 0 = matched chunk only, 1 = ±1 chunk (default).",
                            "default": 1
                        },
                        "threshold": {
                            "type": "number",
                            "description": "Minimum relevance score 0.0–1.0 (default: 0.0)",
                            "default": 0.0
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "status",
                "description": "Return statistics about the current RAGgy index: document count, \
                                total nodes, entity count, and last indexed time. Useful for \
                                understanding what has been indexed before querying.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "graph",
                "description": "Explore entity connections in the knowledge graph. Given an entity \
                                name, returns the entities that co-occur with it most frequently and \
                                the documents they appear in. Useful for discovering related concepts \
                                and people around a topic.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "entity": {
                            "type": "string",
                            "description": "Entity name to look up (person, organization, location, etc.)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of connections to return (default: 10)",
                            "default": 10
                        }
                    },
                    "required": ["entity"]
                }
            }
        ]
    }))
}

// ── Tool implementations ──────────────────────────────────────────────────────

fn tool_search(
    id: &Option<Value>,
    args: &Value,
    store: &GraphStore,
    fulltext: &FulltextIndex,
    vector: &mut Option<VectorIndex>,
    config: &Config,
) -> Value {
    let query = match args["query"].as_str() {
        Some(q) if !q.trim().is_empty() => q,
        _ => return tool_error(id, "Missing required argument: query"),
    };
    let limit     = args["limit"].as_u64().unwrap_or(5) as usize;
    let window    = args["window"].as_u64().unwrap_or(1) as usize;
    let threshold = args["threshold"].as_f64().unwrap_or(0.0) as f32;

    let query_plan = parser::parse_query(query);
    let alpha = query_plan.suggested_alpha.unwrap_or(config.embedding.alpha);

    let results = match plan::execute_plan(
        &query_plan,
        fulltext,
        vector.as_mut(),
        store,
        limit,
        threshold,
        alpha,
        window,
    ) {
        Ok(r) => r,
        Err(e) => return tool_error(id, &format!("Search failed: {}", e)),
    };

    if results.is_empty() {
        return tool_ok(id, &format!("No results found for \"{}\".", query));
    }

    let mut text = format!("Found {} result(s) for \"{}\"\n\n", results.len(), query);
    for (i, r) in results.iter().enumerate() {
        text.push_str(&format!("### Result {} — {}\n", i + 1, r.path));
        text.push_str(&format!("Score: {:.3}  |  Type: {}", r.score, r.content_type));
        if !r.title.is_empty() {
            text.push_str(&format!("  |  Title: {}", r.title));
        }
        text.push_str("\n\n");

        // Prefer the context window; fall back to the snippet.
        let body = if !r.context.is_empty() { &r.context } else { &r.snippet };
        text.push_str(body);
        text.push_str("\n\n---\n\n");
    }

    tool_ok(id, text.trim_end())
}

fn tool_status(id: &Option<Value>, store: &GraphStore) -> Value {
    let doc_count    = store.get_document_count().unwrap_or(0);
    let node_count   = store.get_node_count().unwrap_or(0);
    let entity_count = store.get_ner_entity_count().unwrap_or(0);
    let mention_count = store.get_ner_mention_count().unwrap_or(0);
    let last_indexed = store
        .get_last_indexed_time()
        .unwrap_or(None)
        .unwrap_or_else(|| "never".to_string());

    tool_ok(id, &format!(
        "RAGgy Index Status\n\n\
         Documents : {doc_count}\n\
         Nodes     : {node_count}\n\
         Entities  : {entity_count} (canonical)\n\
         Mentions  : {mention_count}\n\
         Last indexed: {last_indexed}",
    ))
}

fn tool_graph(id: &Option<Value>, args: &Value, store: &GraphStore) -> Value {
    let entity_name = match args["entity"].as_str() {
        Some(e) if !e.trim().is_empty() => e,
        _ => return tool_error(id, "Missing required argument: entity"),
    };
    let limit = args["limit"].as_u64().unwrap_or(10) as usize;

    let (entity_id, resolved_name, entity_type) =
        match store.find_ner_entity_exact(entity_name) {
            Ok(Some(e)) => e,
            Ok(None) => {
                return tool_ok(
                    id,
                    &format!("Entity '{}' not found in the knowledge graph.", entity_name),
                )
            }
            Err(e) => return tool_error(id, &format!("Graph lookup failed: {}", e)),
        };

    let connections = store.get_ner_connections(entity_id).unwrap_or_default();

    if connections.is_empty() {
        return tool_ok(
            id,
            &format!(
                "Entity '{}' ({}) is in the graph but has no recorded connections.",
                resolved_name, entity_type
            ),
        );
    }

    let mut text = format!("Entity: {} ({})\n\n", resolved_name, entity_type);
    text.push_str(&format!(
        "Top {} connection(s) by co-occurrence weight:\n\n",
        limit.min(connections.len())
    ));

    for (_, name, etype, weight, doc_count) in connections.iter().take(limit) {
        text.push_str(&format!(
            "  • {} ({}) — {} co-occurrence(s) across {} document(s)\n",
            name, etype, *weight as u32, doc_count
        ));
    }

    // Also surface the documents where this entity appears
    let docs = store.get_ner_entity_documents(entity_id).unwrap_or_default();
    if !docs.is_empty() {
        text.push_str(&format!("\nAppears in {} document(s):\n", docs.len()));
        for (path, confidence) in docs.iter().take(10) {
            text.push_str(&format!("  • {}  (conf {:.2})\n", path, confidence));
        }
        if docs.len() > 10 {
            text.push_str(&format!("  … and {} more\n", docs.len() - 10));
        }
    }

    tool_ok(id, text.trim_end())
}

// ── Response helpers ──────────────────────────────────────────────────────────

fn rpc_ok(id: &Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: &Option<Value>, code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

/// Successful tool result — content returned to the LLM.
fn tool_ok(id: &Option<Value>, text: &str) -> Value {
    rpc_ok(id, json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false
    }))
}

/// Tool-level error (not a protocol error) — the LLM sees the message.
fn tool_error(id: &Option<Value>, message: &str) -> Value {
    rpc_ok(id, json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true
    }))
}
