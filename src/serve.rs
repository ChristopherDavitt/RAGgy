use std::sync::{Arc, Mutex};

use anyhow::Result;
use axum::{
    extract::{Multipart, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::cors::CorsLayer;

use crate::config::Config;
use crate::entity::gliner::GlinerModel;
use crate::graph::store::GraphStore;
use crate::graph::traverse;
use crate::graph::ContentType;
use crate::index::fulltext::FulltextIndex;
use crate::index::metadata;
use crate::index::vector::VectorIndex;
use crate::ingest::{self, DocumentInput};
use crate::query::{parser, plan};
use crate::workspace::Workspace;

// ── Application State ──

pub struct AppState {
    pub ws: Workspace,
    pub config: Config,
    pub store: Mutex<GraphStore>,
    pub fulltext: Mutex<FulltextIndex>,
    pub vector: Mutex<Option<VectorIndex>>,
    pub ner: Mutex<Option<GlinerModel>>,
}

// ── Error handling ──

struct ApiError(anyhow::Error);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({"error": self.0.to_string()});
        (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError(e)
    }
}

// ── Query parameters ──

#[derive(Deserialize)]
struct QueryParams {
    q: String,
    #[serde(default = "default_limit")]
    limit: usize,
    alpha: Option<f32>,
    #[serde(default)]
    threshold: f32,
    tag: Option<String>,
}

#[derive(Deserialize)]
struct GraphParams {
    #[serde(default = "default_graph_limit")]
    limit: usize,
}

#[derive(Deserialize)]
struct ConnectParams {
    from: String,
    to: String,
    #[serde(default = "default_depth")]
    depth: usize,
}

#[derive(Deserialize)]
struct EntitySearchParams {
    #[serde(default)]
    q: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize { 10 }
fn default_graph_limit() -> usize { 20 }
fn default_depth() -> usize { 5 }

// ── Router ──

fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/query", get(handle_query))
        .route("/api/graph/{entity}", get(handle_graph))
        .route("/api/connect", get(handle_connect))
        .route("/api/status", get(handle_status))
        .route("/api/entities", get(handle_entities))
        .route("/api/databases", get(handle_databases))
        .route("/api/ingest", post(handle_ingest))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

// ── Handlers ──

async fn handle_query(
    State(state): State<Arc<AppState>>,
    Query(params): Query<QueryParams>,
) -> Result<Json<Value>, ApiError> {
    let state = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        let store = state.store.lock().unwrap();
        let fulltext = state.fulltext.lock().unwrap();
        let mut vector_guard = state.vector.lock().unwrap();

        let query_plan = parser::parse_query(&params.q);
        let alpha = params.alpha.unwrap_or(state.config.embedding.alpha);

        let results = plan::execute_plan(
            &query_plan,
            &fulltext,
            vector_guard.as_mut(),
            &store,
            params.limit,
            params.threshold,
            alpha,
        )?;

        // Apply tag filter
        let results: Vec<_> = if let Some(ref tag) = params.tag {
            let tagged_docs = store.find_docs_by_tag(tag).unwrap_or_default();
            results
                .into_iter()
                .filter(|r| tagged_docs.iter().any(|p| r.path.contains(p)))
                .collect()
        } else {
            results
        };

        let results_json: Vec<Value> = results
            .iter()
            .map(|r| {
                json!({
                    "path": r.path,
                    "score": r.score,
                    "content_type": r.content_type,
                    "title": r.title,
                    "snippet": r.snippet,
                })
            })
            .collect();

        Ok::<_, anyhow::Error>(json!({
            "query": params.q,
            "database": state.ws.db_name,
            "count": results_json.len(),
            "results": results_json,
        }))
    })
    .await
    .map_err(|e| anyhow::anyhow!("Task failed: {}", e))??;

    Ok(Json(result))
}

async fn handle_graph(
    State(state): State<Arc<AppState>>,
    Path(entity_name): Path<String>,
    Query(params): Query<GraphParams>,
) -> Result<Json<Value>, ApiError> {
    let state = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        let store = state.store.lock().unwrap();

        let (entity_id, resolved_name, entity_type) =
            match store.find_ner_entity_exact(&entity_name)? {
                Some(e) => e,
                None => {
                    return Ok(json!({
                        "error": "Entity not found",
                        "query": entity_name,
                    }))
                }
            };

        let connections = store.get_ner_connections(entity_id)?;
        let docs = store.get_ner_entity_documents(entity_id)?;

        let conn_json: Vec<Value> = connections
            .iter()
            .take(params.limit)
            .map(|(id, name, etype, weight, doc_count)| {
                json!({
                    "id": id,
                    "name": name,
                    "type": etype,
                    "weight": weight,
                    "doc_count": doc_count,
                })
            })
            .collect();

        Ok::<_, anyhow::Error>(json!({
            "entity": {
                "id": entity_id,
                "name": resolved_name,
                "type": entity_type,
            },
            "documents": docs.iter().map(|(p, c)| json!({"path": p, "confidence": c})).collect::<Vec<_>>(),
            "connections": conn_json,
        }))
    })
    .await
    .map_err(|e| anyhow::anyhow!("Task failed: {}", e))??;

    Ok(Json(result))
}

async fn handle_connect(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ConnectParams>,
) -> Result<Json<Value>, ApiError> {
    let state = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        let store = state.store.lock().unwrap();

        let (from_id, from_name, from_type) =
            match store.find_ner_entity_exact(&params.from)? {
                Some(e) => e,
                None => {
                    return Ok(json!({
                        "error": format!("Entity '{}' not found", params.from),
                    }))
                }
            };

        let (to_id, to_name, to_type) = match store.find_ner_entity_exact(&params.to)? {
            Some(e) => e,
            None => {
                return Ok(json!({
                    "error": format!("Entity '{}' not found", params.to),
                }))
            }
        };

        let path = traverse::find_shortest_path(&store, from_id, to_id, params.depth)?;

        Ok::<_, anyhow::Error>(json!({
            "from": { "id": from_id, "name": from_name, "type": from_type },
            "to": { "id": to_id, "name": to_name, "type": to_type },
            "path": path.as_ref().map(|hops| {
                hops.iter().map(|h| json!({
                    "entity_id": h.entity_id,
                    "name": h.entity_name,
                    "type": h.entity_type,
                    "via_documents": h.shared_docs,
                })).collect::<Vec<_>>()
            }),
            "hops": path.as_ref().map(|h| if h.is_empty() { 0 } else { h.len() - 1 }),
            "connected": path.is_some(),
        }))
    })
    .await
    .map_err(|e| anyhow::anyhow!("Task failed: {}", e))??;

    Ok(Json(result))
}

async fn handle_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    let state = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        let store = state.store.lock().unwrap();

        let doc_count = store.get_document_count()?;
        let node_count = store.get_node_count()?;
        let edge_count = store.get_edge_count()?;
        let entity_count = store.get_entity_count()?;
        let ner_entity_count = store.get_ner_entity_count().unwrap_or(0);
        let ner_mention_count = store.get_ner_mention_count().unwrap_or(0);
        let last_indexed = store.get_last_indexed_time()?;
        let type_counts = metadata::get_content_type_counts(&store)?;
        let all_tags = store.get_all_tags().unwrap_or_default();

        Ok::<_, anyhow::Error>(json!({
            "database": state.ws.db_name,
            "documents": doc_count,
            "nodes": node_count,
            "edges": edge_count,
            "entities": entity_count,
            "ner_entities": ner_entity_count,
            "ner_mentions": ner_mention_count,
            "last_indexed": last_indexed,
            "content_types": type_counts.iter().map(|(t, c)| json!({"type": t, "count": c})).collect::<Vec<_>>(),
            "tags": all_tags.iter().map(|(t, c)| json!({"tag": t, "count": c})).collect::<Vec<_>>(),
            "vector_enabled": state.vector.lock().unwrap().is_some(),
            "ner_enabled": state.ner.lock().unwrap().is_some(),
        }))
    })
    .await
    .map_err(|e| anyhow::anyhow!("Task failed: {}", e))??;

    Ok(Json(result))
}

async fn handle_entities(
    State(state): State<Arc<AppState>>,
    Query(params): Query<EntitySearchParams>,
) -> Result<Json<Value>, ApiError> {
    let state = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        let store = state.store.lock().unwrap();

        let entities = store.find_ner_entities(&params.q)?;
        let entities_json: Vec<Value> = entities
            .iter()
            .take(params.limit)
            .map(|(id, name, etype)| {
                json!({
                    "id": id,
                    "name": name,
                    "type": etype,
                })
            })
            .collect();

        Ok::<_, anyhow::Error>(json!({
            "query": params.q,
            "count": entities_json.len(),
            "entities": entities_json,
        }))
    })
    .await
    .map_err(|e| anyhow::anyhow!("Task failed: {}", e))??;

    Ok(Json(result))
}

async fn handle_databases(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    let state = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        let databases = state.ws.list_databases()?;

        let dbs_json: Vec<Value> = databases
            .iter()
            .map(|name| {
                let is_current = *name == state.ws.db_name;
                json!({
                    "name": name,
                    "current": is_current,
                })
            })
            .collect();

        Ok::<_, anyhow::Error>(json!({
            "databases": dbs_json,
            "current": state.ws.db_name,
        }))
    })
    .await
    .map_err(|e| anyhow::anyhow!("Task failed: {}", e))??;

    Ok(Json(result))
}

async fn handle_ingest(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<Value>, ApiError> {
    // Collect files from multipart upload
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => return Err(ApiError(anyhow::anyhow!("Multipart error: {}", e))),
        };
        let name = field.file_name().unwrap_or("upload.txt").to_string();
        let data = match field.bytes().await {
            Ok(b) => b,
            Err(e) => return Err(ApiError(anyhow::anyhow!("Failed to read field: {}", e))),
        };
        files.push((name, data.to_vec()));
    }

    if files.is_empty() {
        return Ok(Json(json!({"error": "No files uploaded"})));
    }

    let state = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        let store = state.store.lock().unwrap();
        let fulltext = state.fulltext.lock().unwrap();
        let mut vector_guard = state.vector.lock().unwrap();
        let mut ner_guard = state.ner.lock().unwrap();

        let mut docs = Vec::new();
        for (name, bytes) in &files {
            let ext = std::path::Path::new(&name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("txt")
                .to_lowercase();

            let content_type = ContentType::from_extension(&ext)
                .unwrap_or(ContentType::PlainText);

            match DocumentInput::from_bytes(
                name.clone(),
                name.clone(),
                bytes,
                content_type,
                &ext,
                None,
            ) {
                Ok(doc) => docs.push(doc),
                Err(e) => {
                    eprintln!("Warning: failed to extract {}: {}", name, e);
                }
            }
        }

        let ingest_result = ingest::ingest_documents(
            &docs,
            &store,
            &fulltext,
            vector_guard.as_mut(),
            ner_guard.as_mut(),
            &state.config,
            None,
        )?;

        Ok::<_, anyhow::Error>(json!({
            "files_indexed": ingest_result.files_indexed,
            "files_skipped": ingest_result.files_skipped,
            "files_failed": ingest_result.files_failed,
            "chunks_embedded": ingest_result.chunks_embedded,
        }))
    })
    .await
    .map_err(|e| anyhow::anyhow!("Task failed: {}", e))??;

    Ok(Json(result))
}

// ── Server startup ──

pub fn start_server(port: u16, ws: Workspace) -> Result<()> {
    let config = ws.config()?;
    ws.ensure_dirs()?;

    let store = GraphStore::open(&ws.db_path())?;
    let fulltext = FulltextIndex::open(&ws.tantivy_dir())?;

    let vector = {
        let model_dir = ws.embedding_model_dir(&config);
        match VectorIndex::open(&ws.vector_dir(), &model_dir) {
            Ok(v) => {
                eprintln!("  Vector embeddings enabled");
                Some(v)
            }
            Err(_) => {
                eprintln!("  Vector embeddings disabled (model not found)");
                None
            }
        }
    };

    let ner = {
        let model_dir = ws.ner_model_dir(&config);
        match GlinerModel::load(&model_dir) {
            Ok(m) => {
                eprintln!("  NER enabled ({})", config.ner.model);
                Some(m)
            }
            Err(_) => {
                eprintln!("  NER disabled (model not found)");
                None
            }
        }
    };

    let state = Arc::new(AppState {
        ws,
        config,
        store: Mutex::new(store),
        fulltext: Mutex::new(fulltext),
        vector: Mutex::new(vector),
        ner: Mutex::new(ner),
    });

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let app = build_router(state);
        let addr = format!("0.0.0.0:{}", port);
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        eprintln!("\nraggy server listening on http://{}", addr);
        eprintln!("  POST /api/ingest       Upload documents");
        eprintln!("  GET  /api/query?q=...   Search");
        eprintln!("  GET  /api/graph/{{name}} Entity connections");
        eprintln!("  GET  /api/connect       Path between entities");
        eprintln!("  GET  /api/status        Index stats");
        eprintln!("  GET  /api/entities      Search entities");
        eprintln!("  GET  /api/databases     List databases");
        axum::serve(listener, app).await?;
        Ok::<_, anyhow::Error>(())
    })?;

    Ok(())
}
