/// File watcher — keeps the RAGgy index in sync with directories on disk.
///
/// Watches one or more paths recursively.  When a file is created or
/// modified, it is incrementally re-indexed (only that file is processed).
/// When a file is removed, its entries are purged from the index.
///
/// Indexes (including the ONNX embedding model) are loaded once at startup
/// and reused across all events, so per-file latency is low after the
/// initial load.
///
/// Typical use alongside OpenClaw:
///   raggy watch ~/.openclaw/memory/
///
/// OpenClaw writes Markdown memory files; raggy watch picks them up within
/// ~300 ms and makes them immediately searchable via `raggy query` or the
/// MCP server.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};

use crate::config::Config;
use crate::graph::store::GraphStore;
use crate::index::fulltext::FulltextIndex;
use crate::index::vector::VectorIndex;
use crate::ingest;
use crate::workspace::Workspace;

pub fn run(paths: &[PathBuf], ws: &Workspace) -> Result<()> {
    if !ws.exists() {
        anyhow::bail!(
            "No index found for database '{}'. Run `raggy index <path>` first.",
            ws.db_name
        );
    }

    let config = ws.config()?;
    let store = GraphStore::open(&ws.db_path())?;
    let fulltext = FulltextIndex::open(&ws.tantivy_dir())?;

    eprintln!("raggy watch: loading indexes for '{}'…", ws.db_name);

    let mut vector = if config.embedding.enabled {
        let model_dir = ws.embedding_model_dir(&config);
        match VectorIndex::open_verbose(&ws.vector_dir(), &model_dir) {
            Ok(v) => {
                eprintln!("raggy watch: vector index ready ({} vectors)", v.len());
                Some(v)
            }
            Err(e) => {
                eprintln!("raggy watch: vector index unavailable ({}), using keyword-only", e);
                None
            }
        }
    } else {
        None
    };

    // Set up debounced watcher — 300 ms timeout collapses burst writes
    // (e.g. an editor saving multiple times in quick succession) into a
    // single reindex event.
    let (tx, rx) = std::sync::mpsc::channel();
    let mut debouncer = new_debouncer(Duration::from_millis(300), None, tx)?;

    let watch_paths: Vec<PathBuf> = if paths.is_empty() {
        vec![std::env::current_dir()?]
    } else {
        paths.to_vec()
    };

    for path in &watch_paths {
        debouncer.watcher().watch(path, RecursiveMode::Recursive)?;
        eprintln!("raggy watch: watching {}", path.display());
    }

    eprintln!("raggy watch: ready — press Ctrl+C to stop\n");

    for result in rx {
        match result {
            Ok(events) => {
                // Collect unique file paths from this debounced batch,
                // ignoring directory events and deletions (handled separately).
                let mut to_index: Vec<PathBuf> = Vec::new();
                let mut to_delete: Vec<PathBuf> = Vec::new();

                for event in &events {
                    let path = &event.path;

                    // Skip directories
                    if path.is_dir() {
                        continue;
                    }

                    match event.kind {
                        DebouncedEventKind::Any => {
                            if path.exists() {
                                if !to_index.contains(path) {
                                    to_index.push(path.clone());
                                }
                            } else {
                                // File was removed
                                if !to_delete.contains(path) {
                                    to_delete.push(path.clone());
                                }
                            }
                        }
                        DebouncedEventKind::AnyContinuous => {
                            // Still changing — debouncer will fire again when stable
                        }
                        _ => {}
                    }
                }

                // Handle deletions
                for path in &to_delete {
                    let path_str = path.to_string_lossy();
                    eprintln!("raggy watch: removing {}", path_str);
                    store.delete_by_path(&path_str)?;
                    fulltext.delete_by_path(&path_str)?;
                    if let Some(ref mut v) = vector {
                        v.delete_by_path(&path_str);
                    }
                    // Commit deletions immediately
                    fulltext.commit()?;
                    if let Some(ref mut v) = vector {
                        v.commit()?;
                    }
                }

                // Handle additions / modifications
                if !to_index.is_empty() {
                    let start = Instant::now();
                    let result = ingest::ingest_paths(
                        &to_index,
                        &[],
                        &store,
                        &fulltext,
                        vector.as_mut(),
                        None, // GLiNER not needed for watch mode
                        &config,
                    );
                    match result {
                        Ok(r) => {
                            let elapsed = start.elapsed().as_millis();
                            let names: Vec<String> = to_index
                                .iter()
                                .filter_map(|p| {
                                    p.file_name().map(|n| n.to_string_lossy().into_owned())
                                })
                                .collect();
                            eprintln!(
                                "raggy watch: indexed {} ({} chunks, {}ms)",
                                names.join(", "),
                                r.chunks_embedded,
                                elapsed,
                            );
                        }
                        Err(e) => {
                            eprintln!("raggy watch: index error: {}", e);
                        }
                    }
                }
            }
            Err(errors) => {
                for e in errors {
                    eprintln!("raggy watch: watcher error: {}", e);
                }
            }
        }
    }

    Ok(())
}
