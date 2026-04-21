# RAGgy

**A private, local search engine for your documents, notes, and code.** Point it at a folder, ask a question, get back the passages that matter — with the people, places, and dates in those passages linked together. Nothing leaves your machine.

RAGgy is built for anyone who works with a personal corpus and needs better than grep: investigative journalists working through FOIA dumps and interview transcripts, researchers cross-referencing sources, engineers searching their own codebase, and AI agents that need durable memory they can trust.

---

## What you get

- **Ask questions in plain English.** "deployment strategies not kubernetes", "reports modified this month", "emails between A and B in 2024".
- **Hybrid retrieval.** Exact keyword matches (BM25), meaning-based matches (vector embeddings), and entity-graph matches — combined into a single ranked result.
- **Entities, linked.** People, organizations, dates, URLs, file paths, and code identifiers are extracted on ingest, deduplicated, and connected. Ask "who is connected to Acme Corp?" and get a real answer.
- **Works with any frontier model.** Runs as an MCP server for Claude Desktop out of the box. Also exposes an HTTP API for custom clients. You can also use it standalone as a CLI.
- **Private by design.** Everything — index, embeddings, documents — lives in `~/.raggy/`. No cloud, no telemetry, no account.

## Who this is for

- **Journalists and researchers** working with sensitive source material who can't upload to a cloud service.
- **Engineers** who want better search over their own codebase and notes than `grep` and fuzzy file finders.
- **AI practitioners** building agents that need persistent, queryable memory of documents, conversations, and artifacts.

## Install

RAGgy is currently installed from source. Binary releases are on the roadmap.

```bash
# Requires Rust 1.75+  (https://rustup.rs)
git clone https://github.com/ChristopherDavitt/RAGgy
cd RAGgy
cargo install --path .

# First-time setup: creates ~/.raggy/ and downloads the embedding + NER models (~200 MB)
raggy init
```

After `raggy init` finishes, you're ready to index and query.

## 60-second hello world

```bash
# Index a folder of documents, code, or notes
raggy index ~/Documents/my-project

# Ask a question
raggy query "authentication middleware"

# Or in plain English
raggy query "pdfs about climate policy from last month"
```

That's it. Results come back ranked by a mix of keyword, semantic, and entity-graph signals.

> **A GIF demo will live here.** If you're seeing the raw repo, you can record one yourself with a real query and open a PR — see `CONTRIBUTING.md`.

## Use it with Claude Desktop (MCP)

RAGgy speaks the Model Context Protocol, so Claude Desktop can search your corpus directly.

Add this to your Claude Desktop MCP config (`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS):

```json
{
  "mcpServers": {
    "raggy": {
      "command": "raggy",
      "args": ["mcp"]
    }
  }
}
```

Restart Claude Desktop and ask it to search your indexed corpus.

## Use it over HTTP

```bash
raggy serve --port 3000
```

Then:

```bash
curl -X POST http://localhost:3000/search \
  -H 'Content-Type: application/json' \
  -d '{"query": "deployment strategies", "limit": 5}'
```

## Common commands

```bash
raggy index <paths>...           # ingest files or directories
raggy query "<question>"         # search
raggy watch <paths>...           # live-index a folder as it changes
raggy reindex                    # update the index (incremental by default)
raggy status                     # show counts and index size
raggy graph <entity>             # show entities connected to a given entity
raggy connect <from> <to>        # find the shortest link between two entities
raggy db list | create | delete  # manage independent databases
raggy serve                      # start the HTTP server
raggy mcp                        # start an MCP server on stdio
```

Run `raggy <cmd> --help` for full options.

## What it can index

| Extension | Type | Notes |
| --- | --- | --- |
| `.md`, `.mdx` | Markdown | Heading structure preserved as sections |
| `.txt`, `.text`, `.log` | Plain text | |
| `.pdf` | PDF | Text extraction via `pdf-extract` |
| `.html`, `.htm` | HTML | |
| `.csv`, `.tsv` | Tabular | |
| `.rs`, `.js`, `.ts`, `.py`, `.go`, `.sol` | Code | Split by top-level definitions |
| `.json`, `.yaml`, `.yml`, `.toml` | Structured | |

RAGgy respects `.gitignore` via the `ignore` crate.

## Query language

Plain English works, but you can also use these operators explicitly:

```bash
raggy query "deployment not kubernetes"              # negation
raggy query "markdown files about testing"           # type filter
raggy query "error handling in ~/projects/api"       # scope filter
raggy query "reports modified this week"             # date filter
raggy query "climate --from 2024-01 --to 2024-06"    # explicit date range
raggy query "climate --recent"                       # boost recent docs
raggy query "auth" --no-vectors                      # keyword-only
raggy query "auth" --alpha 0.8                       # favor semantic over keyword
raggy query "auth" --format json                     # structured output
```

## How it works, briefly

RAGgy maintains three views of your corpus and blends them at query time:

1. **A full-text index** (Tantivy, BM25) for exact and near-exact matches.
2. **A vector index** (HNSW over `all-MiniLM-L6-v2` ONNX embeddings) for meaning-based matches.
3. **An entity graph** (SQLite) where extracted entities — people, organizations, dates, identifiers — are nodes connected through the documents they appear in.

A query hits all three, scores are merged, and the graph boosts documents that mention entities referenced in the query. See **[ARCHITECTURE.md](./ARCHITECTURE.md)** for the full story.

## File layout

```
~/.raggy/
├── raggy.toml                # global config (default db, etc.)
├── models/                   # ONNX embedding + NER models (shared)
└── dbs/
    └── default/
        ├── config.toml       # per-database config
        ├── raggy.db          # SQLite: graph + metadata
        ├── tantivy/          # full-text index
        ├── vectors/          # HNSW vector index
        └── documents/        # processed copies of ingested files
```

Override the home directory with `--home <path>` or `$RAGGY_HOME`.

## Status and stability

RAGgy is **pre-1.0** and under active development. Indexes may need rebuilding across minor versions until we tag 1.0. If you hit a bug, please open an issue with the output of `raggy status` and the steps to reproduce.

## Contributing

Contributions are welcome — see **[CONTRIBUTING.md](./CONTRIBUTING.md)**. All commits must be signed off under the [Developer Certificate of Origin](./CONTRIBUTING.md#developer-certificate-of-origin).

## Security

To report a security vulnerability, please see **[SECURITY.md](./SECURITY.md)**. Do not open a public issue.

## License

MIT — see [LICENSE](./LICENSE).
