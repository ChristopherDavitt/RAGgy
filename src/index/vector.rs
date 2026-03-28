use anyhow::{Result, Context};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::embedder::Embedder;

/// Metadata associated with each vector in the HNSW index.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VectorMeta {
    pub node_id: u64,
    pub doc_path: String,
}

/// A point in embedding space. Wraps a Vec<f32> and implements instant_distance::Point.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EmbeddingPoint(pub Vec<f32>);

impl instant_distance::Point for EmbeddingPoint {
    fn distance(&self, other: &Self) -> f32 {
        // Cosine distance = 1 - dot_product (vectors are L2-normalized)
        let dot: f32 = self.0.iter().zip(other.0.iter()).map(|(a, b)| a * b).sum();
        1.0 - dot
    }
}

/// Result from a vector similarity search.
pub struct VectorSearchResult {
    pub node_id: u64,
    pub doc_path: String,
    pub score: f32, // cosine similarity (0.0 to 1.0)
}

/// The HNSW-based vector index, backed by instant-distance.
/// Points and metadata are persisted via bincode. The HNSW graph is rebuilt on load.
pub struct VectorIndex {
    embedder: Embedder,
    hnsw: Option<instant_distance::HnswMap<EmbeddingPoint, usize>>,
    meta: Vec<VectorMeta>,
    points: Vec<EmbeddingPoint>,
    // Pending additions (accumulated before commit)
    pending_points: Vec<EmbeddingPoint>,
    pending_meta: Vec<VectorMeta>,
    // Paths to delete on next commit
    deleted_paths: HashSet<String>,
    index_dir: PathBuf,
}

impl VectorIndex {
    /// Open or create a vector index. Rebuilds HNSW graph from stored points.
    pub fn open(index_dir: &Path, model_dir: &Path) -> Result<Self> {
        Self::open_inner(index_dir, model_dir, false)
    }

    /// Open with verbose progress output (for indexing).
    pub fn open_verbose(index_dir: &Path, model_dir: &Path) -> Result<Self> {
        Self::open_inner(index_dir, model_dir, true)
    }

    fn open_inner(index_dir: &Path, model_dir: &Path, verbose: bool) -> Result<Self> {
        std::fs::create_dir_all(index_dir)?;

        let embedder = if verbose {
            Embedder::load_verbose(model_dir)?
        } else {
            Embedder::load(model_dir)?
        };

        let meta_path = index_dir.join("meta.bin");
        let points_path = index_dir.join("points.bin");

        let (meta, points) = if meta_path.exists() && points_path.exists() {
            let meta_bytes = std::fs::read(&meta_path)?;
            let meta: Vec<VectorMeta> = bincode::deserialize(&meta_bytes)
                .context("Failed to deserialize vector metadata")?;

            let points_bytes = std::fs::read(&points_path)?;
            let points: Vec<EmbeddingPoint> = bincode::deserialize(&points_bytes)
                .context("Failed to deserialize vector points")?;

            (meta, points)
        } else {
            (Vec::new(), Vec::new())
        };

        // Rebuild HNSW from stored points
        let hnsw = if !points.is_empty() {
            let values: Vec<usize> = (0..points.len()).collect();
            Some(instant_distance::Builder::default().build(points.clone(), values))
        } else {
            None
        };

        Ok(VectorIndex {
            embedder,
            hnsw,
            meta,
            points,
            pending_points: Vec::new(),
            pending_meta: Vec::new(),
            deleted_paths: HashSet::new(),
            index_dir: index_dir.to_path_buf(),
        })
    }

    /// Embed and stage a single chunk. Call `commit()` to finalize.
    pub fn add_document(&mut self, node_id: u64, doc_path: &str, text: &str) -> Result<()> {
        let embedding = self.embedder.embed(text)?;

        self.pending_points.push(EmbeddingPoint(embedding));
        self.pending_meta.push(VectorMeta {
            node_id,
            doc_path: doc_path.to_string(),
        });

        Ok(())
    }

    /// Embed a batch of chunks at once (much faster than one-at-a-time).
    /// Returns the number of successfully embedded chunks.
    pub fn add_batch(
        &mut self,
        chunks: &[(u64, String, String)], // (node_id, doc_path, text)
    ) -> Result<usize> {
        if chunks.is_empty() {
            return Ok(0);
        }

        // Process in sub-batches of 32 to manage memory
        let batch_size = 32;
        let mut total = 0;

        for batch in chunks.chunks(batch_size) {
            let texts: Vec<&str> = batch.iter().map(|(_, _, t)| t.as_str()).collect();
            let embeddings = self.embedder.embed_batch(&texts)?;

            for (i, embedding) in embeddings.into_iter().enumerate() {
                let (node_id, ref doc_path, _) = batch[i];
                self.pending_points.push(EmbeddingPoint(embedding));
                self.pending_meta.push(VectorMeta {
                    node_id,
                    doc_path: doc_path.clone(),
                });
                total += 1;
            }
        }

        Ok(total)
    }

    /// Mark all vectors from a given doc_path for deletion on next commit.
    pub fn delete_by_path(&mut self, doc_path: &str) {
        self.deleted_paths.insert(doc_path.to_string());
    }

    /// Rebuild the HNSW index from (existing - deleted + pending) and persist to disk.
    pub fn commit(&mut self) -> Result<()> {
        // Merge existing (minus deleted) with pending
        let mut all_points = Vec::new();
        let mut all_meta = Vec::new();

        // Keep existing that aren't deleted
        for (i, m) in self.meta.iter().enumerate() {
            if !self.deleted_paths.contains(&m.doc_path) {
                all_points.push(self.points[i].clone());
                all_meta.push(m.clone());
            }
        }

        // Add pending
        all_points.extend(self.pending_points.drain(..));
        all_meta.extend(self.pending_meta.drain(..));
        self.deleted_paths.clear();

        if all_points.is_empty() {
            self.hnsw = None;
            self.meta = Vec::new();
            self.points = Vec::new();
            let _ = std::fs::remove_file(self.index_dir.join("meta.bin"));
            let _ = std::fs::remove_file(self.index_dir.join("points.bin"));
            return Ok(());
        }

        // Build HNSW index
        let values: Vec<usize> = (0..all_points.len()).collect();
        let hnsw = instant_distance::Builder::default().build(all_points.clone(), values);

        // Persist points and metadata (HNSW is rebuilt on load)
        let meta_bytes = bincode::serialize(&all_meta)
            .context("Failed to serialize vector metadata")?;
        std::fs::write(self.index_dir.join("meta.bin"), meta_bytes)?;

        let points_bytes = bincode::serialize(&all_points)
            .context("Failed to serialize vector points")?;
        std::fs::write(self.index_dir.join("points.bin"), points_bytes)?;

        self.hnsw = Some(hnsw);
        self.meta = all_meta;
        self.points = all_points;

        Ok(())
    }

    /// Search for the nearest neighbors to a query string.
    pub fn search(&mut self, query: &str, limit: usize) -> Result<Vec<VectorSearchResult>> {
        let hnsw = match &self.hnsw {
            Some(h) => h,
            None => return Ok(Vec::new()),
        };

        let query_embedding = self.embedder.embed(query)?;
        let query_point = EmbeddingPoint(query_embedding);

        let mut search = instant_distance::Search::default();
        let results: Vec<_> = hnsw.search(&query_point, &mut search)
            .take(limit)
            .collect();

        let mut out = Vec::with_capacity(results.len());
        for item in results {
            let idx = *item.value;
            if idx < self.meta.len() {
                let meta = &self.meta[idx];
                // Convert distance to similarity: sim = 1.0 - distance
                let similarity = 1.0 - item.distance;
                out.push(VectorSearchResult {
                    node_id: meta.node_id,
                    doc_path: meta.doc_path.clone(),
                    score: similarity,
                });
            }
        }

        Ok(out)
    }

    /// Number of vectors in the index (committed + pending).
    pub fn len(&self) -> usize {
        self.meta.len() + self.pending_meta.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Embed a single text string. Exposes the internal embedder for external use.
    pub fn embed(&mut self, text: &str) -> Result<Vec<f32>> {
        self.embedder.embed(text)
    }

    /// Embed a batch of text strings. Exposes the internal embedder for external use.
    pub fn embed_batch(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.embedder.embed_batch(texts)
    }
}
