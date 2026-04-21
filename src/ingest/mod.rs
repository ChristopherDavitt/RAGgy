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
    EntitiesResolved { merges: u64 },
    Committing,
}

/// A document from any source, ready for ingestion.
/// This is the boundary between "where content comes from" and "how it's processed."
pub struct DocumentInput {
    /// Unique identifier for this document (filesystem path, URL, upload ID, etc.)
    pub doc_id: String,
    /// Human-readable name (filename, title, etc.)
    pub name: String,
    /// Raw content as string
    pub content: String,
    /// Content type determines which extractor to use
    pub content_type: ContentType,
    /// Content hash for change detection (skip if unchanged)
    pub content_hash: String,
    /// File size in bytes
    pub size: u64,
    /// Last modified timestamp (RFC3339)
    pub modified: String,
}

impl DocumentInput {
    /// Create a DocumentInput from raw content, auto-computing hash.
    pub fn new(doc_id: String, name: String, content: String, content_type: ContentType, size: u64, modified: String) -> Self {
        let content_hash = compute_hash(content.as_bytes());
        DocumentInput { doc_id, name, content, content_type, content_hash, size, modified }
    }

    /// Create from in-memory bytes (e.g. HTTP upload, S3 object).
    /// Uses the appropriate extractor based on file extension.
    pub fn from_bytes(doc_id: String, name: String, bytes: &[u8], content_type: ContentType, extension: &str, modified: Option<String>) -> Result<Self> {
        let content_hash = compute_hash(bytes);
        let content = crate::extractors::extract(bytes, extension)?;
        let size = bytes.len() as u64;
        let modified = modified.unwrap_or_else(|| Utc::now().to_rfc3339());
        Ok(DocumentInput { doc_id, name, content, content_type, content_hash, size, modified })
    }
}

// ── Filesystem source ──

/// Walk local directories and ingest files. This is the filesystem-specific entry point.
pub fn ingest_paths(
    paths: &[PathBuf],
    exclude: &[String],
    store: &GraphStore,
    fulltext: &FulltextIndex,
    vector: Option<&mut VectorIndex>,
    ner: Option<&mut GlinerModel>,
    config: &Config,
    documents_dir: Option<&std::path::Path>,
) -> Result<IngestResult> {
    ingest_paths_with_progress(paths, exclude, store, fulltext, vector, ner, config, None, documents_dir)
}

/// Walk local directories with progress reporting.
pub fn ingest_paths_with_progress(
    paths: &[PathBuf],
    exclude: &[String],
    store: &GraphStore,
    fulltext: &FulltextIndex,
    mut vector: Option<&mut VectorIndex>,
    mut ner: Option<&mut GlinerModel>,
    config: &Config,
    mut progress: Option<ProgressFn>,
    documents_dir: Option<&std::path::Path>,
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

            // Read file and build DocumentInput
            let bytes = match std::fs::read(file_path) {
                Ok(c) => c,
                Err(_) => {
                    result.files_failed += 1;
                    continue;
                }
            };

            let metadata = match std::fs::metadata(file_path) {
                Ok(m) => m,
                Err(_) => {
                    result.files_failed += 1;
                    continue;
                }
            };

            let modified = metadata.modified()
                .map(|t| chrono::DateTime::<Utc>::from(t))
                .unwrap_or_else(|_| Utc::now())
                .to_rfc3339();

            let path_str = file_path.to_string_lossy().to_string();
            let name = file_path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            // Extract text from bytes using the appropriate extractor
            let content = match crate::extractors::extract(&bytes, &ext) {
                Ok(c) => c,
                Err(e) => {
                    if let Some(ref mut cb) = progress {
                        cb(ProgressEvent::FileFailed {
                            path: path_str,
                            error: format!("Extraction failed: {}", e),
                        });
                    }
                    result.files_failed += 1;
                    continue;
                }
            };

            let doc = DocumentInput::new(
                path_str, name, content, content_type, metadata.len() as u64, modified,
            );

            // Delegate to the source-agnostic processor
            let doc_result = ingest_document(
                &doc, store, fulltext, vector.as_deref_mut(), ner.as_deref_mut(),
                config, &mut next_id, &mut progress,
                result.files_indexed + result.files_skipped + result.files_failed + 1,
                documents_dir,
            )?;

            match doc_result {
                DocOutcome::Indexed { chunks_embedded } => {
                    result.files_indexed += 1;
                    result.chunks_embedded += chunks_embedded;
                }
                DocOutcome::Skipped => {
                    result.files_skipped += 1;
                }
                DocOutcome::Failed => {
                    result.files_failed += 1;
                }
            }
        }
    }

    // Run entity resolution after all documents are processed
    if config.ner.enabled {
        let res = entity::resolution::resolve_entities(store)?;
        let mut total_merges = res.merges;

        // Gradient-based (embedding similarity) resolution pass
        if config.ner.gradient_resolution {
            if let Some(ref mut v) = vector {
                let gres = entity::resolution::gradient_resolve_entities(
                    store, *v, config.ner.gradient_threshold,
                )?;
                total_merges += gres.merges;
            }
        }

        if total_merges > 0 {
            if let Some(ref mut cb) = progress {
                cb(ProgressEvent::EntitiesResolved { merges: total_merges });
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

// ── Source-agnostic document processing ──

enum DocOutcome {
    Indexed { chunks_embedded: u64 },
    Skipped,
    Failed,
}

/// Process a single document from any source. This is the core ingestion logic
/// that knows nothing about where the content came from.
fn ingest_document(
    doc: &DocumentInput,
    store: &GraphStore,
    fulltext: &FulltextIndex,
    mut vector: Option<&mut VectorIndex>,
    mut ner: Option<&mut GlinerModel>,
    config: &Config,
    next_id: &mut u64,
    progress: &mut Option<ProgressFn>,
    file_num: u64,
    documents_dir: Option<&std::path::Path>,
) -> Result<DocOutcome> {
    // Check if unchanged
    if let Ok(Some(existing_hash)) = store.get_document_hash(&doc.doc_id) {
        if existing_hash == doc.content_hash {
            if let Some(ref mut cb) = progress {
                cb(ProgressEvent::FileSkipped { path: doc.doc_id.clone() });
            }
            return Ok(DocOutcome::Skipped);
        }
        // Changed — delete old entries
        store.delete_by_path(&doc.doc_id)?;
        fulltext.delete_by_path(&doc.doc_id)?;
        if let Some(v) = vector.as_deref_mut() {
            v.delete_by_path(&doc.doc_id);
        }
    }

    // Use a synthetic Path for extractors (they only use it for naming)
    let synthetic_path = PathBuf::from(&doc.doc_id);

    // Extract nodes and edges
    let extract_result = match &doc.content_type {
        ContentType::Markdown => markdown::extract(&synthetic_path, &doc.content, next_id),
        ContentType::PlainText => plaintext::extract(&synthetic_path, &doc.content, next_id),
        ContentType::Code { language } => code::extract(&synthetic_path, &doc.content, language, next_id),
        ContentType::Json | ContentType::Yaml | ContentType::Toml => {
            structured::extract(&synthetic_path, &doc.content, &doc.content_type, next_id)
        }
        // PDF, HTML, CSV are already extracted to plain text by the extractors module
        ContentType::Pdf | ContentType::Html | ContentType::Csv => {
            plaintext::extract(&synthetic_path, &doc.content, next_id)
        }
    };

    let (nodes, edges) = match extract_result {
        Ok(r) => r,
        Err(e) => {
            if let Some(ref mut cb) = progress {
                cb(ProgressEvent::FileFailed { path: doc.doc_id.clone(), error: e.to_string() });
            }
            return Ok(DocOutcome::Failed);
        }
    };

    if let Some(ref mut cb) = progress {
        cb(ProgressEvent::FileStart { path: doc.doc_id.clone(), file_num });
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
                    &doc.doc_id,
                    text,
                    node.properties.title.as_deref().unwrap_or(&node.name),
                    &doc.content_type.as_str(),
                )?;
            }
        }

        // Collect chunk text for batch embedding
        if vector.is_some() {
            if let Some(text) = &node.properties.text {
                if !text.is_empty() && node.kind == NodeKind::Chunk {
                    embed_batch.push((node.id, doc.doc_id.clone(), text.clone()));
                }
            }
        }

        // Also index document titles
        if node.kind == NodeKind::Document {
            if let Some(title) = &node.properties.title {
                fulltext.add_document(
                    node.id,
                    &doc.doc_id,
                    title,
                    title,
                    &doc.content_type.as_str(),
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
                    text, node.id, store, next_id,
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
                                        &doc.doc_id,
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

    // Write a processed copy of the document to the documents_dir
    let stored_path = documents_dir.and_then(|dir| write_stored_copy(doc, dir));

    // Store document metadata
    let doc_node_id = nodes.first().map(|n| n.id).unwrap_or(0);
    store.insert_document(
        doc_node_id,
        &doc.doc_id,
        &doc.content_type.as_str(),
        nodes.first().and_then(|n| n.properties.title.as_deref()),
        &doc.modified,
        doc.size,
        &doc.content_hash,
        &Utc::now().to_rfc3339(),
        stored_path.as_deref(),
    )?;

    if let Some(ref mut cb) = progress {
        cb(ProgressEvent::FileComplete {
            path: doc.doc_id.clone(),
            chunks: total_chunks_in_file,
            embeddings: file_embeddings,
        });
    }

    Ok(DocOutcome::Indexed { chunks_embedded: file_embeddings })
}

// ── Public API for non-filesystem sources ──

/// Ingest a batch of documents from any source (HTTP uploads, S3, etc.).
/// Call `commit()` on fulltext/vector after this returns.
pub fn ingest_documents(
    docs: &[DocumentInput],
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

    for (i, doc) in docs.iter().enumerate() {
        let doc_result = ingest_document(
            doc, store, fulltext, vector.as_deref_mut(), ner.as_deref_mut(),
            config, &mut next_id, &mut progress,
            (i + 1) as u64,
            None, // HTTP-uploaded documents are not stored to disk
        )?;

        match doc_result {
            DocOutcome::Indexed { chunks_embedded } => {
                result.files_indexed += 1;
                result.chunks_embedded += chunks_embedded;
            }
            DocOutcome::Skipped => {
                result.files_skipped += 1;
            }
            DocOutcome::Failed => {
                result.files_failed += 1;
            }
        }
    }

    // Run entity resolution
    if config.ner.enabled {
        let res = entity::resolution::resolve_entities(store)?;
        let mut total_merges = res.merges;

        // Gradient-based (embedding similarity) resolution pass
        if config.ner.gradient_resolution {
            if let Some(ref mut v) = vector {
                let gres = entity::resolution::gradient_resolve_entities(
                    store, *v, config.ner.gradient_threshold,
                )?;
                total_merges += gres.merges;
            }
        }

        if total_merges > 0 {
            if let Some(ref mut cb) = progress {
                cb(ProgressEvent::EntitiesResolved { merges: total_merges });
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

// ── Helpers ──

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

/// Write a processed copy of a document to the documents directory.
///
/// Code files keep their original extension so syntax highlighting works.
/// All other content types are stored as `.md` with YAML frontmatter so
/// a frontend or Obsidian vault can render them directly.
///
/// Returns the absolute path of the stored file, or None on failure.
fn write_stored_copy(doc: &DocumentInput, documents_dir: &std::path::Path) -> Option<String> {
    let hash_prefix = &doc.content_hash[..16];

    let is_code = matches!(&doc.content_type, ContentType::Code { .. });

    let ext = if is_code {
        std::path::Path::new(&doc.doc_id)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("txt")
            .to_string()
    } else {
        "md".to_string()
    };

    let dest = documents_dir.join(format!("{}.{}", hash_prefix, ext));

    // Same content → same hash → same dest: skip if already written
    if dest.exists() {
        return Some(dest.to_string_lossy().into_owned());
    }

    let body = if is_code {
        doc.content.clone()
    } else {
        format!(
            "---\nsource: {}\ntitle: {}\ntype: {}\ningested: {}\nmodified: {}\n---\n\n{}",
            doc.doc_id,
            doc.name,
            doc.content_type.as_str(),
            Utc::now().to_rfc3339(),
            doc.modified,
            doc.content,
        )
    };

    match std::fs::write(&dest, &body) {
        Ok(_) => Some(dest.to_string_lossy().into_owned()),
        Err(e) => {
            eprintln!("Warning: could not write stored copy for {}: {}", doc.doc_id, e);
            None
        }
    }
}
