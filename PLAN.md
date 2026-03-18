# Raggy - Development Plan

## Vision

The open-source, local-first, single-binary investigative search engine. Index a folder of documents, automatically extract entities and relationships, search with hybrid keyword+semantic, explore connections through a knowledge graph. No cloud, no API keys, no setup.

**Target users**: Journalists, lawyers, analysts, compliance officers, researchers — anyone who needs to find connections across large document collections.

**Positioning**: What Palantir does at the enterprise level, raggy does for individual investigators. What ChromaDB does for vector search, raggy extends with a knowledge graph and hybrid scoring.

## Current State (MVP)

### What Works
- Hybrid search: Tantivy BM25 + HNSW vector (cosine similarity) with dynamic alpha
- Dynamic alpha: auto-tunes keyword vs semantic weight based on query characteristics
- File ingestion: markdown, plaintext, code (Rust/JS/Python), JSON/YAML/TOML
- ONNX Runtime embeddings (all-MiniLM-L6-v2, statically linked via download-binaries)
- Graph store: SQLite with nodes, edges, documents, entities, co-occurrence
- Entity extraction: regex-based (emails, URLs, paths)
- Incremental indexing with SHA256 change detection
- CLI: `index`, `query`, `status`, `reindex`, `config`
- Progress reporting, Ctrl+C handling, stale lock cleanup

### What's Broken
- **Query latency**: ~25s in debug build (model load dominates). Need release build.
- **Empty snippets**: Vector-only results have no text. Need to look up chunk text from SQLite.
- **Weak NER**: Regex only catches emails/URLs/paths. Can't extract people, orgs, locations.

## Architecture

```
CLI (clap)
  ingest/
    mod.rs         — file walker, orchestrates pipeline
    trait.rs       — Extractor trait (planned)
    markdown.rs    — pulldown-cmark parser
    plaintext.rs   — plaintext chunker
    code.rs        — tree-sitter (Rust/JS/Python)
    structured.rs  — JSON/YAML/TOML
    pdf.rs         — PDF extractor (planned)
  graph/
    store.rs       — SQLite knowledge graph
    traverse.rs    — graph traversal
  index/
    fulltext.rs    — Tantivy BM25
    vector.rs      — HNSW (instant-distance)
    embedder.rs    — ONNX sentence embeddings
  query/
    parser.rs      — query parsing + dynamic alpha
    plan.rs        — hybrid search execution + score fusion
  entity/
    regex.rs       — regex NER + co-occurrence
    ner.rs         — ONNX NER model (planned)
    resolution.rs  — entity dedup/merge (planned)
```

## Roadmap

### Tier 1: Without these, it's a toy

#### 1.1 Extractor Trait + Plugin Architecture
Decouple file type parsing from the ingest pipeline. Every parser implements a trait:
```rust
trait Extractor: Send + Sync {
    fn extensions(&self) -> &[&str];
    fn extract(&self, path: &Path, content: &str, next_id: &mut u64)
        -> Result<(Vec<Node>, Vec<Edge>)>;
}
```
Registry auto-discovers extractors. Adding a new format = one file, no pipeline changes.
Existing parsers (markdown, plaintext, code, structured) become trait implementations.

#### 1.2 Real NER (GLiNER)
Zero-shot NER using GLiNER ONNX model (~188MB quantized). Entity types are configurable per project via `config.toml` — no retraining needed.

**Model**: GLiNER small v2.5 (ONNX), using our existing ONNX runtime. Custom `src/entity/gliner.rs` mirrors `embedder.rs` pattern (tokenize → tensors → inference → span extraction).

**Config**:
```toml
[ner]
enabled = true
model = "gliner-small-v2.5"
labels = ["person", "organization", "location", "date", "money"]
```

**Schema**:
```sql
CREATE TABLE entities (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    canonical INTEGER,              -- FK to canonical entity (for resolution)
    UNIQUE(name, entity_type)
);

CREATE TABLE entity_mentions (
    entity_id INTEGER NOT NULL,
    chunk_id INTEGER NOT NULL,
    doc_path TEXT NOT NULL,
    confidence REAL,
    span_start INTEGER,
    span_end INTEGER,
    FOREIGN KEY (entity_id) REFERENCES entities(id)
);

CREATE TABLE entity_edges (
    entity_a INTEGER NOT NULL,
    entity_b INTEGER NOT NULL,
    weight REAL DEFAULT 1.0,
    doc_count INTEGER DEFAULT 1,
    FOREIGN KEY (entity_a) REFERENCES entities(id),
    FOREIGN KEY (entity_b) REFERENCES entities(id),
    UNIQUE(entity_a, entity_b)
);
```

Labels are config (controls extraction), entities are data (what was found). Changing labels requires reindex.

#### 1.3 Entity Resolution
"J. Epstein", "Jeffrey Epstein", "Epstein" must resolve to the same entity. Without this, the graph is fragmented and every co-occurrence is undercounted.

Approach:
- Substring/alias matching (longest common subsequence)
- Merge entities with same type + high string similarity
- Store canonical name + aliases list
- Resolve at index time, with a manual override command (`raggy merge-entity "J. Epstein" "Jeffrey Epstein"`)

#### 1.4 Graph Traversal Queries
The whole differentiator. Must support:
- `raggy connect "Epstein" "JPMorgan"` — find shortest path(s) between two entities through co-occurrence edges
- `raggy graph "Epstein"` — show all entities connected to this one, ranked by edge weight
- Multi-hop: A→B in doc1, B→C in doc2, C→D in doc3 — show the full chain with source documents at each hop
- Output as human-readable text and JSON for programmatic use

#### 1.5 PDF Ingestion
90% of investigative documents are PDFs. Use the extractor trait. Crate options: `pdf-extract`, `lopdf`, or shell out to `pdftotext`. Must handle:
- Text extraction with page numbers
- Chunking by paragraph/page
- Preserve page-level provenance (chunk → page number)

#### 1.6 Query Speed
Target: sub-2-second queries.
- Release build (now possible with MSVC 14.44)
- If still slow: daemon mode that keeps ONNX model loaded, accepts queries over a socket

### Tier 2: Without these, it's a CLI tool, not a product

#### 2.1 Provenance
Every result traces to exact source: file, page/line, paragraph. Every entity connection shows which documents and which passages established the link. Investigators must cite sources.

Store line offsets per chunk during extraction. Return them in query results.

#### 2.2 Programmatic API
For AI agent integration. Options:
- **MCP server**: Model Context Protocol — agents call raggy as a tool
- **HTTP server**: `raggy serve` starts a local API. REST endpoints for query, graph, index.
- **Both**: MCP wraps the HTTP API

Endpoints: `/query`, `/connect`, `/graph/{entity}`, `/index`, `/status`

#### 2.3 Temporal Analysis
"When did X and Y start appearing together?" / "Show all mentions of X over time."

- Track entity mention timestamps (from document modified dates)
- `raggy timeline "Epstein"` — chronological view of entity appearances
- `raggy timeline "Epstein" "JPMorgan"` — when did these two start co-occurring?

#### 2.4 Weighted Entity Edges
Not just "A and B co-occur" but "A and B co-occur in 47 documents with high contextual proximity." Edge weight = f(frequency, proximity, recency). This is the difference between a graph and a useful graph.

#### 2.5 Scale
Current: HNSW rebuilds from scratch on load, bincode serialization.
Target: 100K+ documents.
- Persistent ANN index (switch from instant-distance to usearch or hnswlib with mmap)
- Lazy HNSW loading
- SQLite indexes on hot query paths
- Benchmark at 10K, 50K, 100K documents

### Tier 3: Competitive advantages

#### 3.1 Multi-hop Discovery
`raggy discover "Person A" --depth 3` — find all entities within 3 hops. Surface non-obvious connections. This is what Palantir charges millions for.

#### 3.2 Document Clustering
Auto-group documents by entity/topic overlap. "These 200 documents form 4 clusters: financial transfers, travel records, communications, legal proceedings." Use entity co-occurrence vectors + community detection on the graph.

#### 3.3 Anomaly Detection
"This entity suddenly appears in 50 documents after being absent" or "These two entities that never co-occurred before now appear together." Pattern breaks are where investigations find leads.

#### 3.4 Watch Mode
`raggy watch ~/investigation/` — monitor directories, auto-index new files. Ongoing investigations receive documents continuously.

#### 3.5 Export
- Graph export: Gephi (.gexf), Neo4j (Cypher), GraphML
- Tabular: CSV of entity connections with source citations
- Report: Markdown summary of entity relationships

#### 3.6 Deduplication
Same content across multiple files (forwarded emails, copied reports). Detect via content hash or embedding similarity. Don't double-count co-occurrences.

#### 3.7 Additional Extractors
Using the trait/plugin architecture:
- DOCX/ODT (office documents)
- HTML (web pages, saved articles)
- CSV/Excel (tabular data with entity columns)
- Email (mbox/eml — sender/recipient as entities)
- Each is a single file implementing the Extractor trait

## Config

Default config stored in `.raggy/config.toml`:
- `embedding.enabled = true`
- `embedding.model = "all-MiniLM-L6-v2"`
- `embedding.alpha = 0.5` (default hybrid weight, overridden by dynamic alpha)
- `index.exclude_patterns` (glob patterns to skip)

## Dependencies

Key crates: `clap`, `tantivy`, `rusqlite` (bundled), `petgraph`, `ort` (download-binaries), `tokenizers`, `instant-distance`, `tree-sitter`, `pulldown-cmark`, `serde`, `walkdir`, `ignore`, `sha2`, `chrono`, `colored`, `ctrlc`, `anyhow`, `thiserror`
