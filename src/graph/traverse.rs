use anyhow::Result;
use std::collections::{HashSet, HashMap, VecDeque};

use super::store::GraphStore;

/// Given entity node IDs, traverse Mentions edges backwards to find chunk nodes,
/// then traverse Contains edges backwards to find document node IDs.
pub fn find_documents_for_entities(store: &GraphStore, entity_ids: &[u64]) -> Result<Vec<u64>> {
    let mut doc_ids = HashSet::new();

    for &entity_id in entity_ids {
        // Entity <- Mentions - Chunk
        let mention_edges = store.get_edges_to(entity_id, "mentions")?;
        for edge in &mention_edges {
            let chunk_id = edge.from;
            // Chunk <- Contains - Document
            let contains_edges = store.get_edges_to(chunk_id, "contains")?;
            for ce in &contains_edges {
                let parent_id = ce.from;
                // Check if parent is a document or section; walk up to document
                if let Some(node) = store.get_node(parent_id)? {
                    if node.kind == super::NodeKind::Document {
                        doc_ids.insert(parent_id);
                    } else {
                        // It's a section, walk up one more level
                        let parent_edges = store.get_edges_to(parent_id, "contains")?;
                        for pe in &parent_edges {
                            doc_ids.insert(pe.from);
                        }
                    }
                }
            }
        }
    }

    Ok(doc_ids.into_iter().collect())
}

/// A hop in a path between two entities.
#[derive(Debug, Clone)]
pub struct PathHop {
    pub entity_id: i64,
    pub entity_name: String,
    pub entity_type: String,
    pub edge_weight: f32,
    pub shared_docs: Vec<String>,
}

/// Find shortest path(s) between two NER entities via co-occurrence edges (BFS).
/// Returns the path as a sequence of hops, or None if no path exists.
/// max_depth limits how many hops to search.
pub fn find_shortest_path(
    store: &GraphStore,
    start_id: i64,
    end_id: i64,
    max_depth: usize,
) -> Result<Option<Vec<PathHop>>> {
    if start_id == end_id {
        return Ok(Some(vec![]));
    }

    // BFS with parent tracking
    let mut visited: HashSet<i64> = HashSet::new();
    let mut parent: HashMap<i64, (i64, f32)> = HashMap::new(); // child -> (parent, edge_weight)
    let mut queue: VecDeque<(i64, usize)> = VecDeque::new();

    visited.insert(start_id);
    queue.push_back((start_id, 0));

    let mut found = false;

    while let Some((current, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }

        let neighbors = store.get_ner_connections(current)?;
        for (neighbor_id, _name, _etype, weight, _doc_count) in &neighbors {
            if !visited.contains(neighbor_id) {
                visited.insert(*neighbor_id);
                parent.insert(*neighbor_id, (current, *weight));
                queue.push_back((*neighbor_id, depth + 1));

                if *neighbor_id == end_id {
                    found = true;
                    break;
                }
            }
        }

        if found {
            break;
        }
    }

    if !found {
        return Ok(None);
    }

    // Reconstruct path from end to start
    let mut path_ids: Vec<(i64, f32)> = Vec::new();
    let mut current = end_id;
    while current != start_id {
        let (prev, weight) = parent[&current];
        path_ids.push((current, weight));
        current = prev;
    }
    path_ids.push((start_id, 0.0));
    path_ids.reverse();

    // Build PathHop structs
    let mut hops = Vec::new();
    for (i, (entity_id, edge_weight)) in path_ids.iter().enumerate() {
        let (name, etype) = get_entity_info(store, *entity_id)?;

        // Find shared documents between this entity and the next one
        let shared_docs = if i + 1 < path_ids.len() {
            let next_id = path_ids[i + 1].0;
            find_shared_documents(store, *entity_id, next_id)?
        } else {
            vec![]
        };

        hops.push(PathHop {
            entity_id: *entity_id,
            entity_name: name,
            entity_type: etype,
            edge_weight: *edge_weight,
            shared_docs,
        });
    }

    Ok(Some(hops))
}

/// Get entity name and type by ID.
fn get_entity_info(store: &GraphStore, entity_id: i64) -> Result<(String, String)> {
    let conn = store.conn();
    let result = conn.query_row(
        "SELECT name, entity_type FROM ner_entities WHERE id = ?1",
        rusqlite::params![entity_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    Ok(result)
}

/// Find documents where both entities are mentioned.
fn find_shared_documents(store: &GraphStore, entity_a: i64, entity_b: i64) -> Result<Vec<String>> {
    let conn = store.conn();
    let mut stmt = conn.prepare(
        "SELECT DISTINCT a.doc_path FROM ner_mentions a
         INNER JOIN ner_mentions b ON a.doc_path = b.doc_path
         WHERE a.entity_id = ?1 AND b.entity_id = ?2"
    )?;
    let rows = stmt.query_map(rusqlite::params![entity_a, entity_b], |row| row.get(0))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}
