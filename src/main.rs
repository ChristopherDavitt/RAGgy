use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::*;
use std::path::{Path, PathBuf};
use std::time::Instant;

use raggy::config::GlobalConfig;
use raggy::graph::store::GraphStore;
use raggy::index::fulltext::FulltextIndex;
use raggy::index::vector::VectorIndex;
use raggy::index::metadata;
use raggy::entity;
use raggy::ingest;
use raggy::query::parser;
use raggy::query::plan;
use raggy::graph::traverse;
use raggy::workspace::Workspace;

#[derive(Parser)]
#[command(name = "raggy", about = "Structured search engine for AI agents")]
struct Cli {
    /// Database to use (overrides default)
    #[arg(long, global = true)]
    db: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Index one or more directories
    Index {
        /// Directories to index
        paths: Vec<PathBuf>,

        /// Glob patterns to exclude
        #[arg(long, short)]
        exclude: Vec<String>,

        /// Tags to apply to indexed documents
        #[arg(long, short)]
        tag: Vec<String>,

        /// Metadata key=value pairs
        #[arg(long, short)]
        meta: Vec<String>,
    },

    /// Search the index
    Query {
        /// Natural language query
        query: String,

        /// Output format
        #[arg(long, default_value = "human")]
        format: OutputFormat,

        /// Max results to return
        #[arg(long, default_value = "10")]
        limit: usize,

        /// Minimum relevance score (0.0 - 1.0)
        #[arg(long, default_value = "0.0")]
        threshold: f32,

        /// Hybrid search weight: 0.0 = pure keyword, 1.0 = pure semantic
        #[arg(long)]
        alpha: Option<f32>,

        /// Disable vector/semantic search, use keyword search only
        #[arg(long)]
        no_vectors: bool,

        /// Filter by tag
        #[arg(long)]
        tag: Option<String>,

        /// Number of sibling chunks to include before and after each matched
        /// chunk for context. 0 = matched chunk only, 1 = ±1 chunk (default),
        /// 2 = ±2 chunks, etc.
        #[arg(long, default_value = "1")]
        window: usize,
    },

    /// Show index statistics
    Status,

    /// Update the index
    Reindex {
        /// Force full rebuild
        #[arg(long)]
        full: bool,
    },

    /// Get or set configuration
    Config {
        key: Option<String>,
        value: Option<String>,
    },

    /// Show entities connected to a given entity
    Graph {
        /// Entity name to explore
        entity: String,

        /// Max results
        #[arg(long, default_value = "20")]
        limit: usize,

        /// Output format
        #[arg(long, default_value = "human")]
        format: OutputFormat,
    },

    /// Find path between two entities
    Connect {
        /// Source entity
        from: String,

        /// Target entity
        to: String,

        /// Maximum hops to search
        #[arg(long, default_value = "5")]
        depth: usize,

        /// Output format
        #[arg(long, default_value = "human")]
        format: OutputFormat,
    },

    /// Start HTTP API server
    Serve {
        /// Port to listen on
        #[arg(long, default_value = "3000")]
        port: u16,
    },

    /// Start MCP (Model Context Protocol) server on stdio.
    /// Add to Claude Desktop via: {"command":"raggy","args":["mcp"]}
    Mcp,

    /// Manage databases
    Db {
        #[command(subcommand)]
        action: DbAction,
    },
}

#[derive(Subcommand)]
enum DbAction {
    /// List all databases
    List,
    /// Create a new database
    Create { name: String },
    /// Delete a database
    Delete { name: String },
    /// Set the default database
    Use { name: String },
    /// Show info about a database
    Info { name: Option<String> },
}

#[derive(Clone, clap::ValueEnum)]
enum OutputFormat {
    Human,
    Json,
}

fn main() -> Result<()> {
    // Ctrl+C handler — abort immediately (exit doesn't work well in Git Bash)
    ctrlc::set_handler(|| {
        let _ = std::io::Write::write_all(&mut std::io::stderr(), b"\nInterrupted.\n");
        std::process::abort();
    }).ok();

    let cli = Cli::parse();

    match cli.command {
        Commands::Db { action } => cmd_db(action, cli.db.as_deref()),
        _ => {
            let ws = Workspace::resolve(cli.db.as_deref())?;
            match cli.command {
                Commands::Index { paths, exclude, tag, meta } => cmd_index(paths, exclude, tag, meta, &ws),
                Commands::Query { query, format, limit, threshold, alpha, no_vectors, tag, window } => {
                    cmd_query(&query, format, limit, threshold, alpha, no_vectors, tag.as_deref(), window, &ws)
                }
                Commands::Status => cmd_status(&ws),
                Commands::Reindex { full } => cmd_reindex(full, &ws),
                Commands::Config { key, value } => cmd_config(key, value, &ws),
                Commands::Graph { entity, limit, format } => cmd_graph(&entity, limit, format, &ws),
                Commands::Connect { from, to, depth, format } => cmd_connect(&from, &to, depth, format, &ws),
                Commands::Serve { port } => raggy::serve::start_server(port, ws),
                Commands::Mcp => raggy::mcp::run(&ws),
                Commands::Db { .. } => unreachable!(),
            }
        }
    }
}

fn cmd_index(paths: Vec<PathBuf>, exclude: Vec<String>, tags: Vec<String>, meta: Vec<String>, ws: &Workspace) -> Result<()> {
    let paths = if paths.is_empty() {
        vec![std::env::current_dir()?]
    } else {
        paths
    };

    ws.ensure_dirs()?;

    let config = ws.config()?;
    ws.save_config(&config)?;

    println!("{} Database: {}", "→".blue().bold(), ws.db_name);

    let store = GraphStore::open(&ws.db_path())?;
    let fulltext = FulltextIndex::open(&ws.tantivy_dir())?;

    // Parse metadata key=value pairs
    let meta_pairs: Vec<(String, String)> = meta.iter()
        .filter_map(|m| {
            let parts: Vec<&str> = m.splitn(2, '=').collect();
            if parts.len() == 2 {
                Some((parts[0].to_string(), parts[1].to_string()))
            } else {
                eprintln!("{} Invalid metadata format '{}', expected key=value", "⚠".yellow().bold(), m);
                None
            }
        })
        .collect();

    // Try to open vector index if embeddings are enabled
    let mut vector = if config.embedding.enabled {
        let model_dir = ws.embedding_model_dir(&config);
        match VectorIndex::open_verbose(&ws.vector_dir(), &model_dir) {
            Ok(v) => {
                println!("{} Vector embeddings enabled ({})", "→".blue().bold(), config.embedding.model);
                Some(v)
            }
            Err(e) => {
                eprintln!(
                    "{} Vector embeddings disabled: {}\n  Falling back to keyword-only indexing.",
                    "⚠".yellow().bold(), e
                );
                None
            }
        }
    } else {
        None
    };

    // Try to load GLiNER NER model if enabled
    let mut ner_model = if config.ner.enabled {
        let model_dir = ws.ner_model_dir(&config);
        match entity::gliner::GlinerModel::load_verbose(&model_dir) {
            Ok(m) => {
                println!("{} NER enabled ({}, {} labels)", "→".blue().bold(), config.ner.model, config.ner.labels.len());
                Some(m)
            }
            Err(e) => {
                eprintln!(
                    "{} NER disabled: {}\n  Falling back to regex-only entity extraction.",
                    "⚠".yellow().bold(), e
                );
                None
            }
        }
    } else {
        None
    };

    if !tags.is_empty() {
        println!("{} Tags: {}", "→".blue().bold(), tags.join(", "));
    }
    if !meta_pairs.is_empty() {
        println!("{} Metadata: {}", "→".blue().bold(),
            meta_pairs.iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>().join(", "));
    }

    let has_vectors = vector.is_some();
    let start = Instant::now();

    let tags_clone = tags.clone();
    let meta_clone = meta_pairs.clone();

    let progress: ingest::ProgressFn = Box::new(|event| {
        use ingest::ProgressEvent::*;
        match event {
            FileStart { path, file_num } => {
                let name = Path::new(&path).file_name()
                    .unwrap_or_default().to_string_lossy();
                eprintln!("  [{}] {}", file_num, name);
            }
            EmbeddingChunk { chunk_num: _, total_in_file } => {
                eprintln!("      embedding {} chunks...", total_in_file);
            }
            FileComplete { path, chunks, embeddings } => {
                let name = Path::new(&path).file_name()
                    .unwrap_or_default().to_string_lossy();
                if embeddings > 0 {
                    eprintln!("  {} {} - {} chunks, {} embedded", "done".green(), name, chunks, embeddings);
                } else {
                    eprintln!("  {} {} - {} chunks", "done".green(), name, chunks);
                }
            }
            FileSkipped { path } => {
                let name = Path::new(&path).file_name()
                    .unwrap_or_default().to_string_lossy();
                eprintln!("  skip {}", name);
            }
            FileFailed { path, error } => {
                let name = Path::new(&path).file_name()
                    .unwrap_or_default().to_string_lossy();
                eprintln!("  FAIL {} ({})", name, error);
            }
            EntitiesResolved { merges } => {
                eprintln!("  Resolved {} entity aliases", merges);
            }
            Committing => {
                eprintln!("  Committing indexes...");
            }
        }
    });

    let result = ingest::ingest_paths_with_progress(
        &paths, &exclude, &store, &fulltext, vector.as_mut(), ner_model.as_mut(), &config, Some(progress),
    )?;

    // Apply tags and metadata to newly indexed documents
    if !tags_clone.is_empty() || !meta_clone.is_empty() {
        let all_paths = store.get_all_document_paths()?;
        for doc_path in &all_paths {
            for tag in &tags_clone {
                store.add_tag(doc_path, tag)?;
            }
            for (key, value) in &meta_clone {
                store.set_metadata(doc_path, key, value)?;
            }
        }
    }

    let elapsed = start.elapsed();

    eprintln!();
    println!(
        "{} Indexed {} files ({} skipped, {} failed) in {:.1}s",
        "✓".green().bold(),
        result.files_indexed,
        result.files_skipped,
        result.files_failed,
        elapsed.as_secs_f64(),
    );

    if has_vectors {
        println!("  Embeddings: {} chunks embedded", result.chunks_embedded);
    }

    Ok(())
}

fn cmd_query(
    query: &str,
    format: OutputFormat,
    limit: usize,
    threshold: f32,
    alpha: Option<f32>,
    no_vectors: bool,
    tag_filter: Option<&str>,
    window: usize,
    ws: &Workspace,
) -> Result<()> {
    if !ws.exists() {
        anyhow::bail!("No index found in database '{}'. Run `raggy index <path>` first.", ws.db_name);
    }

    let config = ws.config()?;
    let store = GraphStore::open(&ws.db_path())?;
    let fulltext = FulltextIndex::open(&ws.tantivy_dir())?;

    // Open vector index unless disabled
    let mut vector = if !no_vectors && config.embedding.enabled {
        let model_dir = ws.embedding_model_dir(&config);
        match VectorIndex::open(&ws.vector_dir(), &model_dir) {
            Ok(v) if !v.is_empty() => Some(v),
            _ => None,
        }
    } else {
        None
    };

    let start = Instant::now();

    let query_plan = parser::parse_query(query);

    let alpha = alpha.unwrap_or_else(|| {
        query_plan.suggested_alpha.unwrap_or(config.embedding.alpha)
    });

    let mut results = plan::execute_plan(
        &query_plan, &fulltext, vector.as_mut(), &store, limit, threshold, alpha, window,
    )?;

    // Filter by tag if specified
    if let Some(tag) = tag_filter {
        let tagged_docs = store.find_docs_by_tag(tag)?;
        results.retain(|r| tagged_docs.contains(&r.path));
    }

    let elapsed = start.elapsed();

    match format {
        OutputFormat::Human => {
            let db_info = if ws.db_name != "default" {
                format!(" [db: {}]", ws.db_name)
            } else {
                String::new()
            };
            println!(
                "Found {} results for \"{}\" ({}ms, alpha={:.2}){}\n",
                results.len(),
                query,
                elapsed.as_millis(),
                alpha,
                db_info,
            );

            for (i, result) in results.iter().enumerate() {
                let tags = store.get_tags(&result.path).unwrap_or_default();
                let tag_str = if tags.is_empty() {
                    String::new()
                } else {
                    format!("  {}", tags.iter().map(|t| format!("#{}", t)).collect::<Vec<_>>().join(" "))
                };

                println!(
                    "  {}. {}  [{}]  {}{}",
                    (i + 1).to_string().bold(),
                    result.path.cyan(),
                    format!("{:.2}", result.score).yellow(),
                    result.content_type.dimmed(),
                    tag_str.magenta(),
                );

                // Show context window if available, otherwise fall back to snippet
                let display_text = if !result.context.is_empty() {
                    &result.context
                } else {
                    &result.snippet
                };
                println!(
                    "     \"{}\"",
                    truncate_snippet(display_text, 400).dimmed(),
                );
                println!();
            }
        }
        OutputFormat::Json => {
            let json_results: Vec<serde_json::Value> = results.iter().map(|r| {
                let tags = store.get_tags(&r.path).unwrap_or_default();
                let meta = store.get_metadata(&r.path).unwrap_or_default();
                let meta_obj: serde_json::Map<String, serde_json::Value> = meta.into_iter()
                    .map(|(k, v)| (k, serde_json::Value::String(v)))
                    .collect();
                serde_json::json!({
                    "path": r.path,
                    "score": r.score,
                    "content_type": r.content_type,
                    "title": r.title,
                    "snippet": r.snippet,
                    "context": r.context,
                    "node_id": r.node_id,
                    "entities": r.entities,
                    "modified": r.modified,
                    "tags": tags,
                    "metadata": meta_obj,
                })
            }).collect();

            let output = serde_json::json!({
                "query": query,
                "database": ws.db_name,
                "plan": {
                    "keywords": query_plan.keywords,
                    "negations": query_plan.negations,
                    "type_filter": query_plan.type_filter,
                    "scope_filter": query_plan.scope_filter,
                    "entities": query_plan.entities,
                    "confidence": query_plan.confidence,
                    "suggested_alpha": query_plan.suggested_alpha,
                },
                "results": json_results,
                "meta": {
                    "elapsed_ms": elapsed.as_millis() as u64,
                    "total_candidates": results.len(),
                    "alpha_used": alpha,
                    "index_version": "0.1.0",
                }
            });

            println!("{}", serde_json::to_string_pretty(&output)?);
        }
    }

    Ok(())
}

fn cmd_status(ws: &Workspace) -> Result<()> {
    if !ws.exists() {
        println!("No index found in database '{}'. Run `raggy index <path>` first.", ws.db_name);
        return Ok(());
    }

    let store = GraphStore::open(&ws.db_path())?;

    let doc_count = store.get_document_count()?;
    let node_count = store.get_node_count()?;
    let edge_count = store.get_edge_count()?;
    let entity_count = store.get_entity_count()?;
    let last_indexed = store.get_last_indexed_time()?;

    println!("{} (database: {})", "raggy index status".bold(), ws.db_name.cyan());
    println!("  Documents:    {}", doc_count);
    println!("  Nodes:        {}", node_count);
    println!("  Edges:        {}", edge_count);
    println!("  Entities:     {}", entity_count);

    let ner_entity_count = store.get_ner_entity_count().unwrap_or(0);
    let ner_mention_count = store.get_ner_mention_count().unwrap_or(0);
    if ner_entity_count > 0 {
        println!("  NER entities: {} ({} mentions)", ner_entity_count, ner_mention_count);
    }

    if let Some(time) = last_indexed {
        println!("  Last indexed: {}", time);
    }

    // Tags
    let all_tags = store.get_all_tags().unwrap_or_default();
    if !all_tags.is_empty() {
        println!("\n  {}", "Tags:".bold());
        for (tag, count) in &all_tags {
            println!("    #{}: {} docs", tag, count);
        }
    }

    // Content type breakdown
    let type_counts = metadata::get_content_type_counts(&store)?;
    if !type_counts.is_empty() {
        println!("\n  {}", "Content types:".bold());
        for (ct, count) in &type_counts {
            println!("    {}: {}", ct, count);
        }
    }

    // Index sizes
    let tantivy_dir = ws.tantivy_dir();
    if tantivy_dir.exists() {
        let size = dir_size(&tantivy_dir);
        println!("\n  Fulltext index: {}", format_bytes(size));
    }

    let vector_dir = ws.vector_dir();
    if vector_dir.exists() {
        let size = dir_size(&vector_dir);
        println!("  Vector index:   {}", format_bytes(size));
    }

    Ok(())
}

fn cmd_reindex(full: bool, ws: &Workspace) -> Result<()> {
    if !ws.exists() {
        anyhow::bail!("No index found in database '{}'. Run `raggy index <path>` first.", ws.db_name);
    }

    let store = GraphStore::open(&ws.db_path())?;
    let fulltext = FulltextIndex::open(&ws.tantivy_dir())?;
    let config = ws.config()?;

    // Try to open vector index
    let mut vector = if config.embedding.enabled {
        let model_dir = ws.embedding_model_dir(&config);
        VectorIndex::open(&ws.vector_dir(), &model_dir).ok()
    } else {
        None
    };

    // Try to load NER model
    let mut ner_model = if config.ner.enabled {
        let model_dir = ws.ner_model_dir(&config);
        entity::gliner::GlinerModel::load(&model_dir).ok()
    } else {
        None
    };

    println!("{} Database: {}", "→".blue().bold(), ws.db_name);

    if full {
        println!("Full reindex requested — rebuilding from scratch...");
        let paths = store.get_all_document_paths()?;
        let dirs: Vec<PathBuf> = paths.iter()
            .filter_map(|p| PathBuf::from(p).parent().map(|p| p.to_path_buf()))
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        if dirs.is_empty() {
            println!("No directories to reindex.");
            return Ok(());
        }

        let start = Instant::now();
        let result = ingest::ingest_paths(&dirs, &[], &store, &fulltext, vector.as_mut(), ner_model.as_mut(), &config)?;
        let elapsed = start.elapsed();

        println!(
            "{} Reindexed {} files ({} skipped, {} failed) in {:.1}s",
            "✓".green().bold(),
            result.files_indexed,
            result.files_skipped,
            result.files_failed,
            elapsed.as_secs_f64(),
        );
    } else {
        println!("Incremental reindex...");
        let paths = store.get_all_document_paths()?;
        let dirs: Vec<PathBuf> = paths.iter()
            .filter_map(|p| PathBuf::from(p).parent().map(|p| p.to_path_buf()))
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        if dirs.is_empty() {
            println!("No directories to reindex.");
            return Ok(());
        }

        let start = Instant::now();

        // Check for deleted files
        let mut deleted = 0u64;
        for path in &paths {
            if !PathBuf::from(path).exists() {
                store.delete_by_path(path)?;
                fulltext.delete_by_path(path)?;
                if let Some(ref mut v) = vector {
                    v.delete_by_path(path);
                }
                deleted += 1;
            }
        }

        let result = ingest::ingest_paths(&dirs, &[], &store, &fulltext, vector.as_mut(), ner_model.as_mut(), &config)?;
        let elapsed = start.elapsed();

        println!(
            "{} Reindexed {} files ({} skipped, {} deleted, {} failed) in {:.1}s",
            "✓".green().bold(),
            result.files_indexed,
            result.files_skipped,
            deleted,
            result.files_failed,
            elapsed.as_secs_f64(),
        );
    }

    Ok(())
}

fn cmd_config(key: Option<String>, value: Option<String>, ws: &Workspace) -> Result<()> {
    ws.ensure_dirs()?;
    let mut config = ws.config()?;

    match (key, value) {
        (None, None) => {
            println!("{} (database: {})\n", "Configuration".bold(), ws.db_name.cyan());
            let toml_str = toml::to_string_pretty(&config)?;
            println!("{}", toml_str);
        }
        (Some(key), None) => {
            match config.get(&key) {
                Some(val) => println!("{} = {}", key, val),
                None => println!("Unknown config key: {}", key),
            }
        }
        (Some(key), Some(value)) => {
            config.set(&key, &value)?;
            ws.save_config(&config)?;
            println!("Set {} = {} (database: {})", key, value, ws.db_name);
        }
        (None, Some(_)) => {
            anyhow::bail!("Cannot set a value without a key");
        }
    }

    Ok(())
}

fn cmd_graph(entity: &str, limit: usize, format: OutputFormat, ws: &Workspace) -> Result<()> {
    if !ws.exists() {
        anyhow::bail!("No index found in database '{}'. Run `raggy index <path>` first.", ws.db_name);
    }

    let store = GraphStore::open(&ws.db_path())?;

    // Find the entity
    let (entity_id, entity_name, entity_type) = match store.find_ner_entity_exact(entity)? {
        Some(e) => e,
        None => {
            // Show suggestions
            let similar = store.find_ner_entities(entity)?;
            if similar.is_empty() {
                println!("Entity '{}' not found.", entity);
            } else {
                println!("Entity '{}' not found. Did you mean:", entity);
                for (_, name, etype) in &similar {
                    println!("  {} ({})", name, etype);
                }
            }
            return Ok(());
        }
    };

    // Get connections
    let connections = store.get_ner_connections(entity_id)?;
    let docs = store.get_ner_entity_documents(entity_id)?;

    match format {
        OutputFormat::Human => {
            println!("{} {} ({})\n", "Entity:".bold(), entity_name.cyan().bold(), entity_type);

            // Documents
            if !docs.is_empty() {
                println!("  {} ({}):", "Mentioned in".bold(), docs.len());
                for (doc_path, conf) in &docs {
                    let name = Path::new(doc_path).file_name()
                        .unwrap_or_default().to_string_lossy();
                    println!("    {} (confidence: {:.2})", name, conf);
                }
                println!();
            }

            // Connections
            if connections.is_empty() {
                println!("  No connections found.");
            } else {
                let shown = connections.len().min(limit);
                println!("  {} ({} total, showing {}):", "Connected to".bold(), connections.len(), shown);
                for (_, name, etype, weight, doc_count) in connections.iter().take(limit) {
                    let weight_bar = "█".repeat((*weight as usize).min(20));
                    println!(
                        "    {} ({})  {} w={:.0} docs={}",
                        name.cyan(),
                        etype.dimmed(),
                        weight_bar.yellow(),
                        weight,
                        doc_count,
                    );
                }
            }
        }
        OutputFormat::Json => {
            let conn_json: Vec<serde_json::Value> = connections.iter().take(limit).map(|(id, name, etype, weight, doc_count)| {
                serde_json::json!({
                    "id": id,
                    "name": name,
                    "type": etype,
                    "weight": weight,
                    "doc_count": doc_count,
                })
            }).collect();

            let output = serde_json::json!({
                "entity": {
                    "id": entity_id,
                    "name": entity_name,
                    "type": entity_type,
                },
                "documents": docs.iter().map(|(p, c)| serde_json::json!({"path": p, "confidence": c})).collect::<Vec<_>>(),
                "connections": conn_json,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
    }

    Ok(())
}

fn cmd_connect(from: &str, to: &str, max_depth: usize, format: OutputFormat, ws: &Workspace) -> Result<()> {
    if !ws.exists() {
        anyhow::bail!("No index found in database '{}'. Run `raggy index <path>` first.", ws.db_name);
    }

    let store = GraphStore::open(&ws.db_path())?;

    // Find source entity
    let (from_id, from_name, from_type) = match store.find_ner_entity_exact(from)? {
        Some(e) => e,
        None => {
            let similar = store.find_ner_entities(from)?;
            if similar.is_empty() {
                println!("Entity '{}' not found.", from);
            } else {
                println!("Entity '{}' not found. Did you mean:", from);
                for (_, name, etype) in similar.iter().take(5) {
                    println!("  {} ({})", name, etype);
                }
            }
            return Ok(());
        }
    };

    // Find target entity
    let (to_id, to_name, to_type) = match store.find_ner_entity_exact(to)? {
        Some(e) => e,
        None => {
            let similar = store.find_ner_entities(to)?;
            if similar.is_empty() {
                println!("Entity '{}' not found.", to);
            } else {
                println!("Entity '{}' not found. Did you mean:", to);
                for (_, name, etype) in similar.iter().take(5) {
                    println!("  {} ({})", name, etype);
                }
            }
            return Ok(());
        }
    };

    // Find shortest path
    let path = traverse::find_shortest_path(&store, from_id, to_id, max_depth)?;

    match format {
        OutputFormat::Human => {
            println!(
                "{} {} ({}) {} ({}):\n",
                "Path:".bold(),
                from_name.cyan().bold(),
                from_type,
                to_name.cyan().bold(),
                to_type,
            );

            match path {
                None => {
                    println!("  No connection found within {} hops.", max_depth);
                }
                Some(hops) => {
                    if hops.is_empty() {
                        println!("  Same entity.");
                        return Ok(());
                    }

                    let total_hops = hops.len() - 1;
                    println!("  {} {} found:\n", total_hops, if total_hops == 1 { "hop" } else { "hops" });

                    for (i, hop) in hops.iter().enumerate() {
                        let prefix = if i == 0 {
                            "  START".green().bold().to_string()
                        } else {
                            format!("  {:>5}", format!("[{}]", i))
                        };

                        println!(
                            "{}  {} ({})",
                            prefix,
                            hop.entity_name.cyan().bold(),
                            hop.entity_type.dimmed(),
                        );

                        if !hop.shared_docs.is_empty() {
                            let doc_names: Vec<String> = hop.shared_docs.iter()
                                .map(|p| Path::new(p).file_name().unwrap_or_default().to_string_lossy().to_string())
                                .collect();
                            println!(
                                "         {} via: {}",
                                "│".dimmed(),
                                doc_names.join(", ").dimmed(),
                            );
                        }
                    }
                }
            }
        }
        OutputFormat::Json => {
            let output = serde_json::json!({
                "from": { "id": from_id, "name": from_name, "type": from_type },
                "to": { "id": to_id, "name": to_name, "type": to_type },
                "path": path.as_ref().map(|hops| {
                    hops.iter().map(|h| serde_json::json!({
                        "entity_id": h.entity_id,
                        "name": h.entity_name,
                        "type": h.entity_type,
                        "edge_weight": h.edge_weight,
                        "shared_documents": h.shared_docs,
                    })).collect::<Vec<_>>()
                }),
                "hops": path.as_ref().map(|h| if h.is_empty() { 0 } else { h.len() - 1 }),
                "max_depth": max_depth,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
    }

    Ok(())
}

fn cmd_db(action: DbAction, db_override: Option<&str>) -> Result<()> {
    // For db commands, we need the raggy_dir but not necessarily a resolved database
    let ws = Workspace::resolve(db_override)?;

    match action {
        DbAction::List => {
            let databases = ws.list_databases()?;
            let global = GlobalConfig::load(&ws.raggy_dir)?;

            if databases.is_empty() {
                println!("No databases. Run `raggy index <path>` to create one.");
            } else {
                println!("{}", "Databases:".bold());
                for name in &databases {
                    let is_default = name == &global.default_db;
                    let marker = if is_default { " (default)" } else { "" };

                    // Get doc count if possible
                    let db_dir = ws.raggy_dir.join("dbs").join(name);
                    let db_path = db_dir.join("raggy.db");
                    let doc_info = if db_path.exists() {
                        match GraphStore::open(&db_path) {
                            Ok(store) => {
                                let count = store.get_document_count().unwrap_or(0);
                                format!(" - {} documents", count)
                            }
                            Err(_) => String::new(),
                        }
                    } else {
                        " - empty".to_string()
                    };

                    if is_default {
                        println!("  {} {}{}{}", "*".green().bold(), name.green().bold(), marker.green(), doc_info);
                    } else {
                        println!("    {}{}", name, doc_info);
                    }
                }
            }
        }

        DbAction::Create { name } => {
            Workspace::create_database(&ws.raggy_dir, &name)?;
            println!("{} Created database '{}'", "✓".green().bold(), name);
            println!("  Use: raggy --db {} index <path>", name);
            println!("  Or:  raggy db use {}", name);
        }

        DbAction::Delete { name } => {
            // Show what will be deleted
            let db_dir = ws.raggy_dir.join("dbs").join(&name);
            let db_path = db_dir.join("raggy.db");
            if db_path.exists() {
                if let Ok(store) = GraphStore::open(&db_path) {
                    let count = store.get_document_count().unwrap_or(0);
                    eprintln!("Database '{}' contains {} documents.", name, count);
                }
            }

            Workspace::delete_database(&ws.raggy_dir, &name)?;
            println!("{} Deleted database '{}'", "✓".green().bold(), name);
        }

        DbAction::Use { name } => {
            let db_dir = ws.raggy_dir.join("dbs").join(&name);
            if !db_dir.exists() {
                anyhow::bail!("Database '{}' does not exist. Create it with `raggy db create {}`", name, name);
            }
            let mut global = GlobalConfig::load(&ws.raggy_dir)?;
            global.default_db = name.clone();
            global.save(&ws.raggy_dir)?;
            println!("{} Default database set to '{}'", "✓".green().bold(), name);
        }

        DbAction::Info { name } => {
            let db_name = name.unwrap_or(ws.db_name.clone());
            let db_dir = ws.raggy_dir.join("dbs").join(&db_name);
            let db_path = db_dir.join("raggy.db");

            if !db_path.exists() {
                println!("Database '{}' is empty (no index yet).", db_name);
                return Ok(());
            }

            let store = GraphStore::open(&db_path)?;
            let doc_count = store.get_document_count()?;
            let node_count = store.get_node_count()?;
            let entity_count = store.get_entity_count()?;
            let ner_count = store.get_ner_entity_count().unwrap_or(0);

            println!("{} {}", "Database:".bold(), db_name.cyan());
            println!("  Path:         {}", db_dir.display());
            println!("  Documents:    {}", doc_count);
            println!("  Nodes:        {}", node_count);
            println!("  Entities:     {} (regex) + {} (NER)", entity_count, ner_count);

            let tags = store.get_all_tags().unwrap_or_default();
            if !tags.is_empty() {
                println!("  Tags:         {}", tags.iter().map(|(t, c)| format!("#{} ({})", t, c)).collect::<Vec<_>>().join(", "));
            }

            // Size on disk
            let mut total_size = 0u64;
            if db_path.exists() {
                total_size += std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
            }
            let tantivy_dir = db_dir.join("tantivy");
            if tantivy_dir.exists() {
                total_size += dir_size(&tantivy_dir);
            }
            let vector_dir = db_dir.join("vectors");
            if vector_dir.exists() {
                total_size += dir_size(&vector_dir);
            }
            println!("  Size:         {}", format_bytes(total_size));
        }
    }

    Ok(())
}

fn truncate_snippet(s: &str, max_len: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() > max_len {
        let end: String = s.chars().take(max_len).collect();
        format!("{}...", end)
    } else {
        s
    }
}

fn dir_size(path: &PathBuf) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    total += meta.len();
                } else if meta.is_dir() {
                    total += dir_size(&entry.path());
                }
            }
        }
    }
    total
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
