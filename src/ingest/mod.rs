pub mod markdown;
pub mod plaintext;
pub mod code;
pub mod structured;

use anyhow::Result;
use chrono::Utc;
use ignore::WalkBuilder;
use sha2::{Sha256, Digest};
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::graph::{NodeKind, ContentType};
use crate::graph::store::GraphStore;
use crate::index::fulltext::FulltextIndex;
use crate::index::vector::VectorIndex;
use crate::entity;
use crate::entity::gliner::GlinerModel;

pub struct IngestResult {
    pub files_indexed: u64,
    pub files_skipped: u64,
    pub files_failed: u64,
    pub chunks_embedded: u64,
}

/// Progress callback for reporting ingest status.
pub type ProgressFn = Box<dyn FnMut(ProgressEvent)>;

pub enum ProgressEvent {
    FileStart { path: String, file_num: u64 },
    FileComplete { path: String, chunks: u64, embeddings: u64 },
    FileSkipped { path: String },
    FileFailed { path: String, error: String },
    EmbeddingChunk { chunk_num: u64, total_in_file: u64 },
    Committing,
}

pub fn ingest_paths(
    paths: &[PathBuf],
    exclude: &[String],
    store: &GraphStore,
    fulltext: &FulltextIndex,
    vector: Option<&mut VectorIndex>,
    ner: Option<&mut GlinerModel>,
    config: &Config,
) -> Result<IngestResult> {
    ingest_paths_with_progress(paths, exclude, store, fulltext, vector, ner, config, None)
}

pub fn ingest_paths_with_progress(
    paths: &[PathBuf],
    exclude: &[String],
    store: &GraphStore,
    fulltext: &FulltextIndex,
    mut vector: Option<&mut VectorIndex>,
    mut ner: Option<&mut GlinerModel>,
    config: &Config,
    mut progress: Option<ProgressFn>,
) -> Result<IngestResult> {
    let mut result = IngestResult {
        files_indexed: 0,
        files_skipped: 0,
        files_failed: 0,
        chunks_embedded: 0,
    };

    let mut next_id = store.next_id()?;

    for path in paths {
        let walker = build_walker(path, exclude, config);
        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => {
                    result.files_failed += 1;
                    continue;
                }
            };

            if !entry.file_type().map_or(false, |ft| ft.is_file()) {
                continue;
            }

            let file_path = entry.path();
            let ext = match file_path.extension().and_then(|e| e.to_str()) {
                Some(e) => e.to_lowercase(),
                None => continue,
            };

            let content_type = match ContentType::from_extension(&ext) {
                Some(ct) => ct,
                None => continue,
            };

            // Compute hash
            let contents = match std::fs::read(file_path) {
                Ok(c) => c,
                Err(_) => {
                    result.files_failed += 1;
                    continue;
                }
            };
            let hash = compute_hash(&contents);

            let path_str = file_path.to_string_lossy().to_string();

            // Check if unchanged
            if let Ok(Some(existing_hash)) = store.get_document_hash(&path_str) {
                if existing_hash == hash {
                    result.files_skipped += 1;
                    if let Some(ref mut cb) = progress {
                        cb(ProgressEvent::FileSkipped { path: path_str.clone() });
                    }
                    continue;
                }
                // Changed — delete old entries
                store.delete_by_path(&path_str)?;
                fulltext.delete_by_path(&path_str)?;
                if let Some(v) = vector.as_deref_mut() {
                    v.delete_by_path(&path_str);
                }
            }

            // Extract nodes and edges
            let text_content = String::from_utf8_lossy(&contents).to_string();
            let extract_result = match &content_type {
                ContentType::Markdown => markdown::extract(file_path, &text_content, &mut next_id),
                ContentType::PlainText => plaintext::extract(file_path, &text_content, &mut next_id),
                ContentType::Code { language } => code::extract(file_path, &text_content, language, &mut next_id),
                ContentType::Json | ContentType::Yaml | ContentType::Toml => {
                    structured::extract(file_path, &text_content, &content_type, &mut next_id)
                }
            };

            let (nodes, edges) = match extract_result {
                Ok(r) => r,
                Err(e) => {
                    result.files_failed += 1;
                    if let Some(ref mut cb) = progress {
                        cb(ProgressEvent::FileFailed { path: path_str.clone(), error: e.to_string() });
                    }
                    continue;
                }
            };

            let file_num = result.files_indexed + result.files_skipped + result.files_failed + 1;
            if let Some(ref mut cb) = progress {
                cb(ProgressEvent::FileStart { path: path_str.clone(), file_num });
            }

            // Count chunks for progress
            let total_chunks_in_file = nodes.iter()
                .filter(|n| n.kind == NodeKind::Chunk && n.properties.text.as_ref().map_or(false, |t| !t.is_empty()))
                .count() as u64;
            let mut file_embeddings = 0u64;

            // Collect chunks for batch embedding
            let mut embed_batch: Vec<(u64, String, String)> = Vec::new();

            // Store nodes and edges
            for node in &nodes {
                store.insert_node(node)?;

                // Index text content in Tantivy
                if let Some(text) = &node.properties.text {
                    if !text.is_empty() {
                        fulltext.add_document(
                            node.id,
                            &path_str,
                            text,
                            node.properties.title.as_deref().unwrap_or(&node.name),
                            &content_type.as_str(),
                        )?;
                    }
                }

                // Collect chunk text for batch embedding
                if vector.is_some() {
                    if let Some(text) = &node.properties.text {
                        if !text.is_empty() && node.kind == NodeKind::Chunk {
                            embed_batch.push((node.id, path_str.clone(), text.clone()));
                        }
                    }
                }

                // Also index document titles
                if node.kind == NodeKind::Document {
                    if let Some(title) = &node.properties.title {
                        fulltext.add_document(
                            node.id,
                            &path_str,
                            title,
                            title,
                            &content_type.as_str(),
                        )?;
                    }
                }
            }

            // Batch embed all chunks for this file at once
            if let Some(v) = vector.as_deref_mut() {
                if !embed_batch.is_empty() {
                    if let Some(ref mut cb) = progress {
                        cb(ProgressEvent::EmbeddingChunk {
                            chunk_num: 0,
                            total_in_file: embed_batch.len() as u64,
                        });
                    }
                    match v.add_batch(&embed_batch) {
                        Ok(n) => { file_embeddings = n as u64; }
                        Err(e) => {
                            eprintln!("\nWarning: batch embedding failed: {}", e);
                        }
                    }
                }
            }

            for edge in &edges {
                store.insert_edge(edge)?;
            }

            // Run entity extraction on chunk nodes
            let mut entity_nodes = Vec::new();
            let mut entity_edges = Vec::new();
            for node in &nodes {
                if node.kind == NodeKind::Chunk {
                    if let Some(text) = &node.properties.text {
                        let (mut enodes, mut eedges) = entity::regex::extract_entities(
                            text, node.id, store, &mut next_id,
                        )?;
                        entity_nodes.append(&mut enodes);
                        entity_edges.append(&mut eedges);
                    }
                }
            }

            for node in &entity_nodes {
                store.insert_node(node)?;
            }
            for edge in &entity_edges {
                store.insert_edge(edge)?;
            }

            // Build co-occurrence edges
            entity::regex::build_cooccurrence_edges(store, &nodes)?;

            // Run GLiNER NER if available
            if let Some(ref mut gliner) = ner {
                let ner_labels = &config.ner.labels;
                let ner_threshold = config.ner.threshold;
                let mut chunk_entity_ids: Vec<Vec<i64>> = Vec::new();

                for node in &nodes {
                    if node.kind == NodeKind::Chunk {
                        if let Some(text) = &node.properties.text {
                            if !text.is_empty() {
                                match gliner.extract(text, ner_labels, ner_threshold) {
                                    Ok(entities) => {
                                        let mut ids_in_chunk = Vec::new();
                                        for ent in &entities {
                                            let entity_id = store.upsert_ner_entity(&ent.text, &ent.label)?;
                                            store.insert_ner_mention(
                                                entity_id,
                                                node.id,
                                                &path_str,
                                                ent.score,
                                                ent.start,
                                                ent.end,
                                            )?;
                                            ids_in_chunk.push(entity_id);
                                        }
                                        chunk_entity_ids.push(ids_in_chunk);
                                    }
                                    Err(e) => {
                                        eprintln!("Warning: NER failed on chunk: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }

                // Build NER co-occurrence edges (entities in same chunk)
                for ids in &chunk_entity_ids {
                    for i in 0..ids.len() {
                        for j in (i + 1)..ids.len() {
                            if ids[i] != ids[j] {
                                store.upsert_ner_edge(ids[i], ids[j])?;
                            }
                        }
                    }
                }
            }

            // Store document metadata
            let metadata = std::fs::metadata(file_path)?;
            let modified = metadata.modified()
                .map(|t| chrono::DateTime::<Utc>::from(t))
                .unwrap_or_else(|_| Utc::now());

            let doc_node_id = nodes.first().map(|n| n.id).unwrap_or(0);
            store.insert_document(
                doc_node_id,
                &path_str,
                &content_type.as_str(),
                nodes.first().and_then(|n| n.properties.title.as_deref()),
                &modified.to_rfc3339(),
                metadata.len(),
                &hash,
                &Utc::now().to_rfc3339(),
            )?;

            result.files_indexed += 1;
            result.chunks_embedded += file_embeddings;

            if let Some(ref mut cb) = progress {
                cb(ProgressEvent::FileComplete {
                    path: path_str.clone(),
                    chunks: total_chunks_in_file,
                    embeddings: file_embeddings,
                });
            }
        }
    }

    if let Some(ref mut cb) = progress {
        cb(ProgressEvent::Committing);
    }
    fulltext.commit()?;
    if let Some(v) = vector {
        v.commit()?;
    }
    Ok(result)
}

fn build_walker(path: &Path, exclude: &[String], config: &Config) -> ignore::Walk {
    let mut builder = WalkBuilder::new(path);
    builder.hidden(true);

    // Add exclude patterns from config
    let mut overrides = ignore::overrides::OverrideBuilder::new(path);
    for pattern in &config.index.exclude_patterns {
        let _ = overrides.add(&format!("!{}", pattern));
    }
    for pattern in exclude {
        let _ = overrides.add(&format!("!{}", pattern));
    }
    if let Ok(ov) = overrides.build() {
        builder.overrides(ov);
    }

    builder.build()
}

fn compute_hash(contents: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(contents);
    format!("{:x}", hasher.finalize())
}
