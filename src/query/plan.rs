use anyhow::Result;
use std::collections::HashMap;

use chrono::Utc;
use super::parser::QueryPlan;
use crate::graph::store::GraphStore;
use crate::graph::traverse::find_documents_for_entities;
use crate::index::fulltext::{make_snippet, FulltextIndex, SearchResult};
use crate::index::vector::VectorIndex;

/// Half-life for recency decay in days.  A document modified this many days
/// ago receives half the recency boost of one modified today.
const RECENCY_HALF_LIFE_DAYS: f32 = 30.0;

/// Maximum additive score contribution from recency boost.
/// Using multiplicative form: score *= (1 + RECENCY_WEIGHT * decay)
/// so a document from today has its score scaled by (1 + RECENCY_WEIGHT).
const RECENCY_WEIGHT: f32 = 0.4;

pub struct QueryResult {
    pub path: String,
    pub score: f32,
    pub content_type: String,
    pub title: String,
    pub snippet: String,
    /// Surrounding context window: the matched chunk plus up to `window`
    /// sibling chunks on each side within the same section/document.
    /// Empty string when window=0 or context fetch fails.
    pub context: String,
    pub node_id: u64,
    pub entities: Vec<String>,
    pub modified: Option<String>,
}

pub fn execute_plan(
    plan: &QueryPlan,
    fulltext: &FulltextIndex,
    mut vector: Option<&mut VectorIndex>,
    store: &GraphStore,
    limit: usize,
    threshold: f32,
    alpha: f32,
    window: usize,
    recency: bool,
) -> Result<Vec<QueryResult>> {
    // 1. BUILD TANTIVY QUERY
    let query_string = if plan.keywords.is_empty() {
        return Ok(Vec::new());
    } else {
        plan.keywords.join(" ")
    };

    let fetch_count = limit * 3;

    // 2. Get BM25 results from Tantivy
    let bm25_results = fulltext.search(&query_string, fetch_count)?;

    // Keep results at chunk level (by node_id) so multiple chunks from the same file can appear
    let mut bm25_by_node: HashMap<u64, SearchResult> = HashMap::new();
    for result in bm25_results {
        match bm25_by_node.entry(result.node_id) {
            std::collections::hash_map::Entry::Vacant(e) => { e.insert(result); }
            std::collections::hash_map::Entry::Occupied(mut e) => {
                if result.score > e.get().score {
                    e.insert(result);
                }
            }
        }
    }

    // 3. Get vector results (if available)
    let vector_results = if let Some(vec_idx) = vector.as_deref_mut() {
        vec_idx.search(&query_string, fetch_count)?
    } else {
        Vec::new()
    };

    // 4. HYBRID SCORING
    let mut candidates = if !vector_results.is_empty() && alpha > 0.0 {
        hybrid_merge(&bm25_by_node, &vector_results, alpha, store, &query_string)
    } else {
        // Pure BM25 mode
        bm25_by_node.into_values().collect()
    };

    // 5. APPLY METADATA FILTERS
    if let Some(ref type_filter) = plan.type_filter {
        candidates.retain(|r| r.content_type.contains(type_filter));
    }

    if let Some(ref scope) = plan.scope_filter {
        let scope_str = scope.to_string_lossy().to_string();
        candidates.retain(|r| r.doc_path.contains(&scope_str));
    }

    // 5b. DATE FILTER + RECENCY BOOST
    // Batch-fetch modified times once for all candidate paths so we avoid
    // per-candidate DB round-trips.
    let need_modified = plan.date_filter.is_some() || recency;
    let modified_map = if need_modified {
        let paths: Vec<&str> = candidates.iter().map(|r| r.doc_path.as_str()).collect();
        store.get_documents_modified(&paths)?
    } else {
        HashMap::new()
    };

    // Hard date filter — drop candidates outside the requested window.
    if let Some(ref date_range) = plan.date_filter {
        candidates.retain(|r| {
            let Some(modified) = modified_map.get(&r.doc_path) else {
                return true; // unknown modified time — keep rather than silently drop
            };
            if let Some(after) = date_range.after {
                if *modified < after {
                    return false;
                }
            }
            if let Some(before) = date_range.before {
                if *modified > before {
                    return false;
                }
            }
            true
        });
    }

    // Recency boost — multiplicative so relevance ordering is preserved.
    // score *= (1 + RECENCY_WEIGHT * exp(-λ * days_since_modified))
    if recency {
        let now = Utc::now();
        let lambda = std::f32::consts::LN_2 / RECENCY_HALF_LIFE_DAYS;
        for candidate in &mut candidates {
            if let Some(modified) = modified_map.get(&candidate.doc_path) {
                let days = (now - *modified).num_seconds().max(0) as f32 / 86_400.0;
                let decay = (-lambda * days).exp();
                candidate.score *= 1.0 + RECENCY_WEIGHT * decay;
            }
        }
    }

    // 6. GRAPH BOOST
    if !plan.entities.is_empty() {
        let mut entity_ids = Vec::new();
        for entity_name in &plan.entities {
            if let Ok(nodes) = store.find_entity_by_name(entity_name) {
                for node in nodes {
                    entity_ids.push(node.id);
                }
            }
        }

        if !entity_ids.is_empty() {
            if let Ok(graph_doc_ids) = find_documents_for_entities(store, &entity_ids) {
                let total_entities = plan.entities.len() as f32;
                for candidate in &mut candidates {
                    if let Ok(Some(node_id)) = store.get_document_by_path(&candidate.doc_path) {
                        if graph_doc_ids.contains(&node_id) {
                            let connected = 1.0;
                            candidate.score += 0.3 * (connected / total_entities);
                        }
                    }
                }
            }
        }
    }

    // Also boost based on keyword entities extracted from the query
    let query_entity_matches = crate::entity::regex::extract_entities_from_text(&query_string);
    if !query_entity_matches.is_empty() {
        let mut entity_ids = Vec::new();
        for em in &query_entity_matches {
            if let Ok(nodes) = store.find_entity_by_name(&em.value) {
                for node in nodes {
                    entity_ids.push(node.id);
                }
            }
        }
        if !entity_ids.is_empty() {
            if let Ok(graph_doc_ids) = find_documents_for_entities(store, &entity_ids) {
                for candidate in &mut candidates {
                    if let Ok(Some(node_id)) = store.get_document_by_path(&candidate.doc_path) {
                        if graph_doc_ids.contains(&node_id) {
                            candidate.score += 0.15;
                        }
                    }
                }
            }
        }
    }

    // 7. NEGATION EXCLUSION
    if !plan.negations.is_empty() {
        let neg_query = plan.negations.join(" ");
        if let Ok(neg_results) = fulltext.search(&neg_query, 100) {
            let neg_paths: Vec<String> = neg_results.iter()
                .filter(|r| r.score > 0.5)
                .map(|r| r.doc_path.clone())
                .collect();
            candidates.retain(|r| !neg_paths.contains(&r.doc_path));
        }
    }

    // 8. RANK AND RETURN
    candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    candidates.truncate(limit);

    // Filter by threshold
    candidates.retain(|r| r.score >= threshold);

    let results: Vec<QueryResult> = candidates.into_iter().map(|r| {
        let context = if window > 0 {
            store.get_chunk_window(r.node_id, window).unwrap_or_default()
        } else {
            String::new()
        };
        QueryResult {
            path: r.doc_path,
            score: r.score,
            content_type: r.content_type,
            title: r.title,
            snippet: r.snippet,
            context,
            node_id: r.node_id,
            entities: Vec::new(),
            modified: None,
        }
    }).collect();

    Ok(results)
}

/// Merge BM25 and vector results using weighted linear combination with min-max normalization.
/// Keyed by node_id so multiple chunks from the same file can appear.
fn hybrid_merge(
    bm25_by_node: &HashMap<u64, SearchResult>,
    vector_results: &[crate::index::vector::VectorSearchResult],
    alpha: f32, // 0.0 = pure BM25, 1.0 = pure vector
    store: &GraphStore,
    query: &str,
) -> Vec<SearchResult> {
    // Normalize BM25 scores to [0, 1]
    let bm25_scores: Vec<f32> = bm25_by_node.values().map(|r| r.score).collect();
    let bm25_min = bm25_scores.iter().cloned().fold(f32::INFINITY, f32::min);
    let bm25_max = bm25_scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let bm25_range = bm25_max - bm25_min;

    // Normalize vector scores to [0, 1]
    let vec_scores: Vec<f32> = vector_results.iter().map(|r| r.score).collect();
    let vec_min = vec_scores.iter().cloned().fold(f32::INFINITY, f32::min);
    let vec_max = vec_scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let vec_range = vec_max - vec_min;

    // Build a map of node_id -> normalized vector score
    let mut vec_score_map: HashMap<u64, (f32, String)> = HashMap::new();
    for vr in vector_results {
        let norm = if vec_range > 0.0 {
            (vr.score - vec_min) / vec_range
        } else {
            1.0
        };
        vec_score_map
            .entry(vr.node_id)
            .and_modify(|(s, _)| { if norm > *s { *s = norm; } })
            .or_insert((norm, vr.doc_path.clone()));
    }

    // Merge: start with BM25 results, add hybrid scores
    let mut merged: HashMap<u64, SearchResult> = HashMap::new();

    for (&node_id, result) in bm25_by_node {
        let bm25_norm = if bm25_range > 0.0 {
            (result.score - bm25_min) / bm25_range
        } else {
            1.0
        };
        let vec_norm = vec_score_map.get(&node_id).map(|(s, _)| *s).unwrap_or(0.0);
        let hybrid = (1.0 - alpha) * bm25_norm + alpha * vec_norm;

        merged.insert(node_id, SearchResult {
            node_id: result.node_id,
            doc_path: result.doc_path.clone(),
            score: hybrid,
            snippet: result.snippet.clone(),
            title: result.title.clone(),
            content_type: result.content_type.clone(),
        });
    }

    // Add vector-only results (not in BM25)
    for vr in vector_results {
        if !merged.contains_key(&vr.node_id) {
            let vec_norm = if vec_range > 0.0 {
                (vr.score - vec_min) / vec_range
            } else {
                1.0
            };
            let hybrid = alpha * vec_norm; // bm25 component is 0

            // Look up chunk text from the graph store to build a snippet
            let (snippet, title, content_type) = if let Ok(Some(node)) = store.get_node(vr.node_id) {
                let text = node.properties.text.as_deref().unwrap_or("");
                let snippet = if !text.is_empty() {
                    make_snippet(text, query)
                } else {
                    String::new()
                };
                let title = node.properties.title.unwrap_or_default();
                let ct = node.properties.content_type
                    .map(|c| c.as_str())
                    .unwrap_or_default();
                (snippet, title, ct)
            } else {
                (String::new(), String::new(), String::new())
            };

            merged.insert(vr.node_id, SearchResult {
                node_id: vr.node_id,
                doc_path: vr.doc_path.clone(),
                score: hybrid,
                snippet,
                title,
                content_type,
            });
        }
    }

    merged.into_values().collect()
}
