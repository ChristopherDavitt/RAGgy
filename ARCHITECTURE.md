# RAGgy Architecture

This document explains how RAGgy retrieves information. If you just want to use
it, read the [README](./README.md); if you want to contribute, start with
[CONTRIBUTING.md](./CONTRIBUTING.md) and come back here when you need context on
a specific module.

## The core idea: three indexes, merged

Most "RAG" tools give you one retrieval signal: either BM25 (good at exact
terms, blind to synonyms) or dense vectors (good at meaning, blind to rare
proper nouns). Either one alone is missing half the picture. RAGgy runs three
complementary views of the corpus in parallel and blends them at query time.

```
                         ┌──────────────────────────┐
                         │       Your query         │
                         └────────────┬─────────────┘
                                      │
                  ┌───────────────────┼───────────────────┐
                  │                   │                   │
                  ▼                   ▼                   ▼
          ┌──────────────┐    ┌──────────────┐    ┌──────────────┐
          │   BM25       │    │  Vector      │    │  Entity      │
          │   (Tantivy)  │    │  (HNSW+ONNX) │    │  Graph       │
          │              │    │              │    │  (SQLite)    │
          │  exact-term  │    │  meaning /   │    │  co-occurr.  │
          │  matches     │    │  synonyms    │    │  via named   │
          │              │    │              │    │  entities    │
          └──────┬───────┘    └──────┬───────┘    └──────┬───────┘
                 └──────────────┬────┘                    │
                                ▼                         │
                       ┌────────────────┐                 │
                       │ Hybrid merge   │                 │
                       │ (alpha-blend)  │                 │
                       └────────┬───────┘                 │
                                ▼                         │
                       ┌────────────────┐                 │
                       │ Graph boost    │◀────────────────┘
                       │ + filters      │
                       │ + recency      │
                       └────────┬───────┘
                                ▼
                        Ranked results
```

Each of the three is good at something the others aren't, and the merge is what
makes RAGgy search feel different from running a plain vector DB.

## The three indexes

### 1. Full-text (Tantivy, BM25)

Implemented in `src/index/fulltext.rs`. Tantivy is a Rust-native Lucene
equivalent. We store one document per *chunk* (not per file) so long files can
contribute multiple matching passages. BM25 is excellent at rare-term retrieval
— proper nouns, identifiers, exact phrases — which is exactly what dense
vectors tend to miss.

### 2. Vector (HNSW over ONNX embeddings)

Implemented in `src/index/vector.rs`. We use
[`instant-distance`](https://crates.io/crates/instant-distance) for a pure-Rust
HNSW implementation and the
[`all-MiniLM-L6-v2`](https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2)
ONNX model via [`ort`](https://ort.pyke.io/) for embeddings. This catches
semantic matches — a query for "authentication" will find passages about
"login flow" even with no keyword overlap.

The embedding model is intentionally small (~23 MB, 384 dimensions). It runs
fast on CPU and keeps first-time install to a reasonable download.

### 3. Entity graph (SQLite)

Implemented in `src/entity/` and `src/graph/`. On ingest, every chunk is
scanned for named entities via two extractors:

- **Regex extractors** (`src/entity/regex.rs`) for URLs, emails, file paths,
  dates, versions, and code identifiers. Deterministic, fast, zero false
  positives.
- **[GLiNER](https://github.com/urchade/GLiNER)** (`src/entity/gliner.rs`), a
  small ONNX NER model, for people, organizations, and locations.

Each entity becomes a node; each "entity appears in chunk X" becomes an edge.
After ingestion, an entity-resolution pass (`src/entity/resolution.rs`) merges
aliases — "John Smith" and "J. Smith" in the same document; "acme.com" and
"Acme Corp" linked by email domain. An optional second pass uses embedding
similarity ("gradient resolution") for softer matches.

The graph is stored in SQLite and queryable directly via `raggy graph <entity>`
and `raggy connect <a> <b>`.

## The query pipeline

End-to-end, from query string to ranked results:

### 1. Parse (`src/query/parser.rs`)

The query is decomposed into a structured `QueryPlan`:

- **Keywords** — what goes into BM25 and vector search.
- **Negations** — `"deployment not kubernetes"` → exclude `kubernetes`.
- **Entities** — proper nouns detected in the query itself, for the graph
  boost.
- **Type filter** — `"markdown files about X"` → only markdown.
- **Scope filter** — `"X in ~/projects/api"` → only files under that path.
- **Date filter** — `"last week"`, `"this month"`, or explicit `--from` /
  `--to` flags.
- **Suggested alpha** — a heuristic for how much to weight keyword vs.
  semantic, based on the shape of the query.

This is deterministic, not LLM-driven. The parser is small enough to read in a
sitting.

### 2. Retrieve (`src/query/plan.rs`, `execute_plan`)

- Run BM25 over Tantivy, keep top `limit * 3` chunks.
- If a vector index is loaded and `alpha > 0`, run a vector search with the
  same fetch depth.
- **Hybrid merge**: normalize each side's scores to [0,1] and combine as
  `alpha * vector_score + (1 - alpha) * bm25_score`. Chunks matched by only
  one side are still kept, with the missing side scored at 0.

### 3. Graph boost

Entities extracted from the query text become graph probes. Documents
connected to those entities get a small multiplicative boost on top of their
hybrid score. This is what makes "reports about Acme Corp" return documents
that *mention* Acme Corp even when "Acme" isn't a strong keyword/vector hit.

### 4. Filter and rerank

- Apply metadata filters (type, scope, date) and negations.
- Apply recency boost when `--recent` is set: exponential decay with a
  30-day half-life, contributing up to ~40% uplift for same-day documents
  (see `RECENCY_HALF_LIFE_DAYS` and `RECENCY_WEIGHT` in `plan.rs`).
- Drop results below `--threshold`, truncate to `--limit`.

### 5. Enrich for output

Each surviving result is hydrated with:

- A snippet (Tantivy-extracted) or a wider context window of surrounding
  chunks (`--window`).
- The path to the processed copy in `~/.raggy/dbs/<db>/documents/` so a
  downstream UI can render the whole file.
- The modified date and entity list.

## The ingest pipeline

`src/ingest/mod.rs` owns the flow. Briefly:

1. **Walk** the input paths respecting `.gitignore` (via the `ignore` crate).
2. **Hash** each file (SHA-256). If the hash matches an existing document,
   skip — this is how `raggy reindex` stays incremental.
3. **Extract** content via the right handler
   (`src/extractors/` + `src/ingest/<type>.rs`): Markdown via
   [`pulldown-cmark`](https://crates.io/crates/pulldown-cmark), PDF via
   [`pdf-extract`](https://crates.io/crates/pdf-extract), HTML via
   [`html2text`](https://crates.io/crates/html2text), code via a
   language-aware line splitter (tree-sitter is a dependency; the current
   extractor is heuristic for portability, see
   [`src/ingest/code.rs`](./src/ingest/code.rs)), and so on.
4. **Chunk** into document → section → chunk nodes with edges. Chunks are the
   retrieval unit everywhere downstream.
5. **Entity-extract** each chunk (regex + GLiNER), add entity nodes and
   edges.
6. **Embed** each chunk (when vectors are enabled) and add to HNSW.
7. **Index** each chunk into Tantivy.
8. **Store** a processed copy of the source file under
   `~/.raggy/dbs/<db>/documents/<hash-prefix>.<ext>` so downstream UIs can
   load it directly. Non-code files are wrapped in YAML frontmatter
   (`source`, `title`, `type`, `ingested`, `modified`).
9. **Resolve entities** across the corpus: merge aliases and canonicalize.
10. **Commit** Tantivy, flush HNSW, done.

## Workspace layout

Everything RAGgy writes lives under a single home directory — by default
`~/.raggy/`, overridable with `--home` or `$RAGGY_HOME`:

```
~/.raggy/
├── raggy.toml                # global config (default db, version, etc.)
├── models/                   # ONNX embedding + NER models (shared across dbs)
│   ├── all-MiniLM-L6-v2/
│   └── gliner-small-v2.1/
└── dbs/
    └── default/              # one folder per independent database
        ├── config.toml
        ├── raggy.db          # SQLite: graph, metadata, entities
        ├── tantivy/          # full-text index
        ├── vectors/          # HNSW vector index
        └── documents/        # processed copies, content-hash named
```

Multiple databases are first-class — one per project, one per client, one per
sensitivity tier. `raggy db list | create | delete | use` manages them.

## Surfaces (how you talk to RAGgy)

The same core engine is exposed through three interfaces:

- **CLI** (`src/main.rs`) — direct, scriptable, JSON-output-friendly.
- **HTTP** (`src/serve.rs`, axum) — REST endpoints for `/search`, `/ingest`,
  `/graph`, and friends. Intended for local-only use; see below.
- **MCP** (`src/mcp.rs`) — a stdio JSON-RPC server speaking the
  [Model Context Protocol](https://modelcontextprotocol.io), so Claude Desktop
  and other MCP clients can call RAGgy as a tool.

All three share the same `Workspace` abstraction (`src/workspace.rs`) so they
see a consistent view of the index.

## Security posture

RAGgy is designed as a **single-user, local-only tool**. The HTTP server is
unauthenticated by design and should not be exposed beyond `localhost`. If you
need multi-user access, put it behind a proper auth proxy — don't patch auth
into RAGgy. See [SECURITY.md](./SECURITY.md) for vulnerability reporting.

## Non-goals (for now)

- **Distributed / cloud-hosted operation.** RAGgy is a local engine. A hosted
  flavor may ship later as a separate product; it won't change the local story.
- **Multi-tenant auth.** See above.
- **Training or fine-tuning embeddings.** We use a pre-trained small model and
  blend its signal with BM25 + graph. That combo outperforms larger models
  used in isolation on the workloads RAGgy targets.
- **A proprietary query DSL.** The query language is intentionally small and
  English-shaped — plain queries work, operators are a thin layer on top.

## Where to start reading the code

If you want to follow an end-to-end flow, this is a sensible reading order:

1. `src/main.rs` — CLI entry, dispatch.
2. `src/workspace.rs` — how paths resolve.
3. `src/ingest/mod.rs` — ingest loop, `ingest_document` in particular.
4. `src/query/parser.rs` → `src/query/plan.rs` — query → plan → results.
5. `src/index/fulltext.rs`, `src/index/vector.rs` — the two index backends.
6. `src/graph/store.rs`, `src/entity/resolution.rs` — the graph side.
7. `src/mcp.rs`, `src/serve.rs` — the two non-CLI surfaces.

Everything else (extractors, config, init, watch) plugs into that spine.
