//! End-to-end regression fixtures for RAGgy's core query pipeline.
//!
//! These tests ingest a small in-memory corpus via the public API and run
//! real BM25 queries through `execute_plan`. They deliberately disable the
//! embedding and NER paths so they run without ONNX model files.
//!
//! What they guard against:
//!   - Ingestion or fulltext indexing silently dropping documents.
//!   - Content-hash dedup regressing into re-ingest-every-time.
//!   - Recency boost losing its effect on ranking.

use chrono::{Duration, Utc};
use raggy::config::Config;
use raggy::graph::store::GraphStore;
use raggy::graph::ContentType;
use raggy::index::fulltext::FulltextIndex;
use raggy::ingest::{ingest_documents, DocumentInput};
use raggy::query::parser::parse_query;
use raggy::query::plan::execute_plan;

struct Fixture {
    _dir: tempfile::TempDir,
    store: GraphStore,
    fulltext: FulltextIndex,
    config: Config,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = GraphStore::open(&dir.path().join("db.sqlite")).unwrap();
        let fulltext = FulltextIndex::open(&dir.path().join("tantivy")).unwrap();
        let mut config = Config::default();
        // Tests must not require ONNX models.
        config.embedding.enabled = false;
        config.ner.enabled = false;
        Fixture { _dir: dir, store, fulltext, config }
    }
}

fn doc(id: &str, content: &str, modified: &str) -> DocumentInput {
    DocumentInput::new(
        id.to_string(),
        id.to_string(),
        content.to_string(),
        ContentType::Markdown,
        content.len() as u64,
        modified.to_string(),
    )
}

#[test]
fn bm25_query_surfaces_the_matching_document() {
    let fx = Fixture::new();
    let now = Utc::now().to_rfc3339();
    let docs = vec![
        doc("a.md", "# Kubernetes deployment strategies for zero-downtime rollouts", &now),
        doc("b.md", "# Baking sourdough bread at home", &now),
        doc("c.md", "# Home office lighting tips", &now),
    ];

    ingest_documents(&docs, &fx.store, &fx.fulltext, None, None, &fx.config, None).unwrap();

    let plan = parse_query("kubernetes deployment");
    let results = execute_plan(&plan, &fx.fulltext, None, &fx.store, 10, 0.0, 0.0, 0, false).unwrap();

    assert!(!results.is_empty(), "query must return at least one result");
    assert_eq!(results[0].path, "a.md", "kubernetes query must rank a.md first");
}

#[test]
fn reingesting_unchanged_content_is_skipped() {
    let fx = Fixture::new();
    let now = Utc::now().to_rfc3339();
    let docs = vec![doc("a.md", "unchanged content about rust programming", &now)];

    let first = ingest_documents(&docs, &fx.store, &fx.fulltext, None, None, &fx.config, None).unwrap();
    assert_eq!(first.files_indexed, 1);
    assert_eq!(first.files_skipped, 0);

    let second = ingest_documents(&docs, &fx.store, &fx.fulltext, None, None, &fx.config, None).unwrap();
    assert_eq!(second.files_indexed, 0, "unchanged doc must not be re-indexed");
    assert_eq!(second.files_skipped, 1, "unchanged doc must be reported as skipped");
}

#[test]
fn recency_boost_promotes_newer_document_over_older() {
    // Two documents with near-identical BM25 scores (same matching tokens,
    // different doc_ids). With recency=false their scores should be roughly
    // equal; with recency=true the newer doc must score strictly higher.
    let fx = Fixture::new();
    let recent = Utc::now().to_rfc3339();
    let ancient = (Utc::now() - Duration::days(365)).to_rfc3339();

    let docs = vec![
        doc("old.md",    "distinctive token corpusfixturexyz appears here", &ancient),
        doc("recent.md", "distinctive token corpusfixturexyz appears here", &recent),
    ];
    ingest_documents(&docs, &fx.store, &fx.fulltext, None, None, &fx.config, None).unwrap();

    let plan = parse_query("corpusfixturexyz");

    let without = execute_plan(&plan, &fx.fulltext, None, &fx.store, 10, 0.0, 0.0, 0, false).unwrap();
    let with    = execute_plan(&plan, &fx.fulltext, None, &fx.store, 10, 0.0, 0.0, 0, true ).unwrap();

    // Results are chunk-level, so reduce to each doc's best-scoring chunk.
    let best = |rs: &[raggy::query::plan::QueryResult], path: &str| -> f32 {
        rs.iter().filter(|r| r.path == path).map(|r| r.score).fold(f32::MIN, f32::max)
    };

    assert!(best(&without, "recent.md") > f32::MIN, "recent.md must be retrieved");
    assert!(best(&without, "old.md")    > f32::MIN, "old.md must be retrieved");

    // With recency on, the newer doc must outrank the ancient one.
    assert!(
        best(&with, "recent.md") > best(&with, "old.md"),
        "recency boost must raise newer score above older"
    );

    // And the boost must actually lift the recent doc's score — otherwise
    // it's a no-op that happens to leave ordering intact by accident.
    assert!(
        best(&with, "recent.md") > best(&without, "recent.md"),
        "recency=true must increase the newer doc's score vs recency=false"
    );
}
