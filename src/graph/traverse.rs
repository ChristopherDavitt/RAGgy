use anyhow::Result;
use std::collections::HashSet;

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
