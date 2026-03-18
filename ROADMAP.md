# Raggy - Product & Architecture Roadmap

## Architecture Principle: Library-first, API-second, UI-third

Raggy follows the pattern used by successful open-source tools (Meilisearch, Typesense, Ollama, ChromaDB):

```
UI layers (many)          CLI, Web UI, Desktop app, VS Code extension
        |
   HTTP/gRPC API          raggy serve (the single gateway)
        |
   Core library           raggy::ingest, raggy::query, raggy::graph (Rust crate)
        |
   Storage backends       local disk, S3, SQLite, etc.
```

The core library is the product. Everything else is a client.

### Current state (as of 2026-03-18)

The bottom two layers are in place:
- **Core library** (`lib.rs`) — ingest, query, graph, entity extraction all exposed as public modules
- **CLI binary** (`main.rs`) — consumes the library, thin wrapper
- **Source abstraction** — `DocumentInput` struct decouples "where content comes from" from "how it's processed". `ingest_documents()` accepts pre-built documents from any source; `ingest_paths()` is the filesystem-specific entry point
- **Multi-database** — `Workspace` abstraction with `raggy db` commands, auto-migration
- **Document tags & metadata** — key-value pairs on documents, tag-based filtering

## Phase 1: Core Engine (current)

The investigative search moat. Without this, raggy is just another search tool.

- [x] Hybrid search (BM25 + vector) with dynamic alpha
- [x] Knowledge graph (SQLite nodes/edges/co-occurrence)
- [x] Multi-database support
- [x] Document tags and metadata
- [x] Source-agnostic ingest pipeline (`DocumentInput`)
- [x] GLiNER NER integration (built, needs model download + testing)
- [ ] Entity resolution ("J. Epstein" = "Jeffrey Epstein")
- [ ] Graph traversal queries (`raggy connect`, `raggy graph`)
- [ ] PDF ingestion
- [ ] Extractor trait / plugin architecture
- [ ] Query speed (release build, possible daemon mode)
- [ ] Fix empty snippets for vector-only results

See PLAN.md for technical details on each item.

## Phase 2: API Server (`raggy serve`)

The single gateway that all clients talk to. This is what makes raggy hostable.

### Why API-first

- Users shouldn't need terminal expertise to ingest files
- AI agents, web UIs, desktop apps, SDKs all talk to the same API
- CLI becomes a client too (`raggy index ./docs` calls `POST /documents` on the server)
- Hosting = running `raggy serve` behind a reverse proxy

### Endpoints

```
POST   /api/v1/documents          upload/ingest files (multipart)
GET    /api/v1/documents          list documents (with tag/metadata filters)
DELETE /api/v1/documents/:id      remove a document
POST   /api/v1/query              search (returns JSON)
GET    /api/v1/graph/:entity      entity connections
POST   /api/v1/connect            find paths between two entities
GET    /api/v1/status             index stats
POST   /api/v1/db                 create database
GET    /api/v1/db                 list databases
```

### Implementation notes

- Framework: axum (Rust, async, lightweight, same ecosystem)
- Models stay loaded in memory (solves query latency — no reload per request)
- Multi-database maps to multi-tenant: each tenant/project gets a database
- Auth: token-based for hosted, optional for local
- The `DocumentInput` abstraction means upload handlers are trivial:
  ```rust
  let doc = DocumentInput::from_bytes(uuid, filename, &body, content_type, None);
  ingest_documents(&[doc], ...)?;
  ```

### Developer experience priorities

- `raggy serve` should just work with zero config (like Ollama)
- File upload via drag-and-drop or API, not "install raggy in the right folder"
- Clear feedback: what was indexed, what entities were found, what's searchable
- API docs auto-generated (utoipa/swagger)

## Phase 3: Web UI

A bundled web dashboard served by `raggy serve`. Not a separate project — ships with the binary.

### Core views

- **Search** — search box, results with snippets, tag filters
- **Upload** — drag-and-drop file upload, bulk import
- **Graph explorer** — visual entity relationship graph (d3.js or similar)
- **Documents** — browse indexed documents, view tags/metadata
- **Database switcher** — manage multiple databases
- **Status** — index stats, NER entity counts, storage usage

### Implementation notes

- Single-page app, embedded in the binary via `include_dir!` or `rust-embed`
- Lightweight: vanilla JS + a small framework (Preact, Svelte, or htmx)
- Served at `http://localhost:9200/` alongside the API
- Graph visualization is the killer feature for investigators

## Phase 4: Storage Backends

Once the API server exists, storage backends become pluggable:

- **Local disk** — current default, always available
- **S3/GCS/Azure Blob** — for hosted deployments, configured via `config.toml`
- **Database blobs** — store document content in SQLite for fully self-contained deployments

Each backend implements the source side of `DocumentInput`. The processing pipeline doesn't change.

## Phase 5: Ecosystem

- **Python SDK** — `pip install raggy-client`, wraps the HTTP API
- **JavaScript SDK** — `npm install @raggy/client`
- **MCP server** — Model Context Protocol for AI agent integration
- **VS Code extension** — search your codebase/docs from the editor
- **Docker image** — `docker run raggy` for instant deployment
- **Helm chart** — Kubernetes deployment

## Design Principles

1. **Single binary** — raggy ships as one executable. No runtime dependencies, no docker required, no cloud accounts. Download and run.
2. **Library-first** — the Rust crate is the source of truth. Every feature works as a library call before it gets an API endpoint or UI button.
3. **Local-first** — works offline, data stays on your machine by default. Hosting is opt-in.
4. **Source-agnostic** — content comes from `DocumentInput`, not filesystem paths. The processing pipeline doesn't care where bytes came from.
5. **Investigative-first** — optimize for entity extraction, graph traversal, and connection discovery. That's the moat. Don't try to be a better vector DB than ChromaDB.
