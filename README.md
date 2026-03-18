# raggy

A structured search engine CLI tool for AI agents. Indexes local files using deterministic methods (no LLM required) and exposes natural language queries that decompose into multi-signal structured retrieval.

## Features

- **BM25 full-text search** powered by Tantivy (Rust-native Lucene equivalent)
- **Entity graph** with regex-based extraction (URLs, emails, file paths, dates, versions, code identifiers)
- **Graph-boosted ranking** — documents connected to query entities get boosted in results
- **Query planner** with support for negation, type filtering, scope filtering, and date ranges
- **Incremental indexing** — SHA-256 content hashing skips unchanged files on reindex
- **Multi-format support** — Markdown, plain text, code (Rust, Python, JS/TS, Go, Solidity), JSON, YAML, TOML
- **JSON output mode** for programmatic consumption by AI agents
- **SQLite-backed graph** for document relationships, entities, and metadata
- **Respects .gitignore** via the `ignore` crate

## Install

Requires [Rust](https://rustup.rs/) 1.75+.

```bash
git clone <repo-url>
cd raggy
cargo build --release
```

The binary is at `target/release/raggy`. Add it to your PATH:

```bash
# Windows
setx PATH "%PATH%;C:\path\to\raggy\target\release"

# macOS / Linux
export PATH="$PATH:/path/to/raggy/target/release"
```

## Quick Start

```bash
# Index a directory
raggy index ~/my-project

# Search
raggy query "authentication middleware"

# Check what's indexed
raggy status
```

## Usage

### Index files

```bash
# Index one or more directories
raggy index ~/projects/api ~/docs

# Exclude patterns
raggy index ~/projects --exclude "*.log" --exclude "vendor"
```

Supported file types:

| Extension | Type |
|---|---|
| `.md`, `.mdx` | Markdown |
| `.txt`, `.text` | Plain text |
| `.rs`, `.js`, `.ts`, `.py`, `.go`, `.sol` | Code |
| `.json` | JSON |
| `.yaml`, `.yml` | YAML |
| `.toml` | TOML |

### Search

```bash
# Basic keyword search
raggy query "deployment strategies"

# Negation — exclude results containing certain terms
raggy query "deployment not kubernetes"

# Type filtering
raggy query "blog post about testing"

# Scope to a directory
raggy query "error handling in ~/projects/api"

# Date filtering
raggy query "config files modified this week"

# JSON output (for AI agents)
raggy query "auth" --format json

# Limit results and set minimum score
raggy query "database" --limit 5 --threshold 0.5
```

### JSON Output

The `--format json` flag returns structured output suitable for AI agent consumption:

```json
{
  "query": "deployment strategies",
  "plan": {
    "keywords": ["deployment", "strategies"],
    "negations": [],
    "type_filter": null,
    "scope_filter": null,
    "entities": [],
    "confidence": 0.7
  },
  "results": [
    {
      "path": "/home/user/docs/deployment.md",
      "score": 3.65,
      "content_type": "markdown",
      "title": "Deployment Strategies",
      "snippet": "When choosing a deployment strategy...",
      "entities": [],
      "modified": null
    }
  ],
  "meta": {
    "elapsed_ms": 35,
    "total_candidates": 2,
    "index_version": "0.1.0"
  }
}
```

### Status

```bash
raggy status
```

```
raggy index status
  Documents:    42
  Nodes:        387
  Edges:        512
  Entities:     96
  Last indexed: 2026-03-15T18:08:43Z

  Content types:
    markdown: 15
    code:python: 12
    code:rust: 8
    yaml: 4
    plaintext: 3

  Index size:   1.2 MB
```

### Reindex

```bash
# Incremental — only reprocesses changed/new/deleted files
raggy reindex

# Full rebuild
raggy reindex --full
```

### Configuration

```bash
# View all config
raggy config

# Get a value
raggy config query.default_limit

# Set a value
raggy config query.default_limit 20
```

Config is stored in `.raggy/config.toml`:

```toml
[index]
exclude_patterns = ["node_modules", ".git", "target", "__pycache__", ".raggy"]

[query]
default_format = "human"
default_limit = 10

[llm]
enabled = false
```

## How It Works

### Indexing Pipeline

1. Walk directory tree (respecting `.gitignore`)
2. Compute SHA-256 hash per file — skip unchanged files
3. Select extractor based on file extension
4. Extract document graph: **Document** -> **Section** -> **Chunk** nodes with edges
5. Run regex entity extractors on chunk text (URLs, emails, dates, identifiers, etc.)
6. Build co-occurrence edges between entities in the same chunk
7. Index text content in Tantivy for BM25 search
8. Store graph + metadata in SQLite

### Query Pipeline

1. Parse query into a **QueryPlan** (keywords, negations, type/scope/date filters, entities)
2. Run BM25 search via Tantivy
3. Apply metadata filters (type, scope, date)
4. Graph boost — entities found in query text boost connected documents
5. Negation exclusion — remove documents matching negated terms
6. Rank and return results

### Architecture

```
raggy/
├── src/
│   ├── main.rs              # CLI entry point (clap)
│   ├── lib.rs               # Public API re-exports
│   ├── config.rs            # Configuration loading/saving
│   ├── ingest/
│   │   ├── mod.rs           # Ingestion orchestrator
│   │   ├── markdown.rs      # Markdown extractor (pulldown-cmark)
│   │   ├── plaintext.rs     # Plain text extractor
│   │   ├── code.rs          # Code extractor
│   │   └── structured.rs    # JSON/YAML/TOML extractor
│   ├── entity/
│   │   ├── mod.rs           # Entity extraction orchestrator
│   │   └── regex.rs         # Pattern-based entity extractors
│   ├── graph/
│   │   ├── mod.rs           # Core data types (Node, Edge, etc.)
│   │   ├── store.rs         # SQLite persistence
│   │   └── traverse.rs      # Graph query operations
│   ├── index/
│   │   ├── mod.rs           # Index manager
│   │   ├── fulltext.rs      # Tantivy full-text index
│   │   ├── metadata.rs      # SQLite metadata queries
│   │   └── tfidf.rs         # TF-IDF (reserved)
│   ├── query/
│   │   ├── mod.rs           # Query module
│   │   ├── parser.rs        # Deterministic query decomposition
│   │   └── plan.rs          # QueryPlan execution
│   └── retrieval/
│       ├── mod.rs           # Retrieval module
│       └── ranker.rs        # Multi-signal ranking (reserved)
└── .raggy/                  # Created at index root
    ├── config.toml
    ├── tantivy/             # Full-text index files
    └── raggy.db             # SQLite graph + metadata
```

## Performance

| Operation | Target |
|---|---|
| Index 1,000 files | < 10s |
| Index 10,000 files | < 60s |
| Query | < 100ms p95 |
| Incremental reindex (10 changed files) | < 2s |

## Tech Stack

- **Rust** — systems language for performance
- **Tantivy** — full-text search with BM25 scoring
- **SQLite** (rusqlite) — graph and metadata storage with WAL mode
- **pulldown-cmark** — Markdown parsing
- **clap** — CLI argument parsing
- **serde** — serialization for JSON/YAML/TOML

## License

MIT
