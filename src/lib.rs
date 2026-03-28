pub mod config;
pub mod entity;
pub mod extractors;
pub mod graph;
pub mod index;
pub mod ingest;
pub mod query;
pub mod retrieval;
pub mod serve;
pub mod workspace;

#[derive(thiserror::Error, Debug)]
pub enum RaggyError {
    #[error("No index found. Run `raggy index <path>` first.")]
    NoIndex,
    #[error("Unsupported file type: {0}")]
    UnsupportedFileType(String),
    #[error("Index corrupted: {0}")]
    IndexCorrupted(String),
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("Search error: {0}")]
    Search(#[from] tantivy::TantivyError),
    #[error("Embedding model not found at {0}. Run with --no-vectors or download the model.")]
    ModelNotFound(String),
}
