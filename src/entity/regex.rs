use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;

use crate::graph::{Node, Edge, NodeKind, NodeProperties, EdgeKind, EdgeProperties, EntityType};
use crate::graph::store::GraphStore;

lazy_static! {
    static ref URL_RE: Regex = Regex::new(r"https?://[^\s)>\]]+").unwrap();
    static ref FILEPATH_RE: Regex = Regex::new(r"(?:~/|/|\./)[\\w./-]+").unwrap();
    static ref EMAIL_RE: Regex = Regex::new(r"[\w.+-]+@[\w.-]+\.\w+").unwrap();
    static ref DATE_ISO_RE: Regex = Regex::new(r"\d{4}-\d{2}-\d{2}").unwrap();
    static ref DATE_WRITTEN_RE: Regex = Regex::new(
        r"(?:January|February|March|April|May|June|July|August|September|October|November|December)\s+\d{1,2},?\s+\d{4}"
    ).unwrap();
    static ref DATE_SLASH_RE: Regex = Regex::new(r"\d{2}/\d{2}/\d{4}").unwrap();
    static ref VERSION_RE: Regex = Regex::new(r"v?\d+\.\d+(?:\.\d+)?(?:-[\w.]+)?").unwrap();
    static ref IDENTIFIER_RE: Regex = Regex::new(
        r"\b(?:[A-Z][a-z]+(?:[A-Z][a-z]+)+|[a-z]+(?:_[a-z]+)+|[A-Z]+(?:_[A-Z]+)+)\b"
    ).unwrap();
}

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "in", "on", "at", "to", "for",
    "of", "with", "by", "from", "is", "are", "was", "were", "be", "been",
    "being", "have", "has", "had", "do", "does", "did", "will", "would",
    "could", "should", "may", "might", "can", "shall", "not", "no", "nor",
    "this", "that", "these", "those", "it", "its", "they", "them", "their",
    "we", "our", "you", "your", "he", "she", "his", "her", "if", "then",
    "else", "when", "where", "how", "what", "which", "who", "whom",
    "all", "each", "every", "both", "few", "more", "most", "other",
    "some", "such", "than", "too", "very", "just", "about",
];

pub struct EntityMatch {
    pub entity_type: EntityType,
    pub value: String,
    pub offset: usize,
}

pub fn extract_entities_from_text(text: &str) -> Vec<EntityMatch> {
    let mut matches = Vec::new();

    // URLs
    for m in URL_RE.find_iter(text) {
        matches.push(EntityMatch {
            entity_type: EntityType::Url,
            value: m.as_str().to_string(),
            offset: m.start(),
        });
    }

    // File paths
    for m in FILEPATH_RE.find_iter(text) {
        let val = m.as_str();
        if val.contains('/') && val.len() > 2 {
            matches.push(EntityMatch {
                entity_type: EntityType::FilePath,
                value: val.to_string(),
                offset: m.start(),
            });
        }
    }

    // Emails
    for m in EMAIL_RE.find_iter(text) {
        matches.push(EntityMatch {
            entity_type: EntityType::Email,
            value: m.as_str().to_string(),
            offset: m.start(),
        });
    }

    // Dates
    for m in DATE_ISO_RE.find_iter(text) {
        matches.push(EntityMatch {
            entity_type: EntityType::Date,
            value: m.as_str().to_string(),
            offset: m.start(),
        });
    }
    for m in DATE_WRITTEN_RE.find_iter(text) {
        matches.push(EntityMatch {
            entity_type: EntityType::Date,
            value: m.as_str().to_string(),
            offset: m.start(),
        });
    }
    for m in DATE_SLASH_RE.find_iter(text) {
        matches.push(EntityMatch {
            entity_type: EntityType::Date,
            value: m.as_str().to_string(),
            offset: m.start(),
        });
    }

    // Versions
    for m in VERSION_RE.find_iter(text) {
        let val = m.as_str();
        // Filter out things that are likely just numbers (e.g., "1.0")
        if val.len() >= 3 {
            matches.push(EntityMatch {
                entity_type: EntityType::Version,
                value: val.to_string(),
                offset: m.start(),
            });
        }
    }

    // Code identifiers (PascalCase, snake_case, SCREAMING_SNAKE_CASE)
    for m in IDENTIFIER_RE.find_iter(text) {
        let val = m.as_str();
        if val.len() >= 3 && !STOPWORDS.contains(&val.to_lowercase().as_str()) {
            matches.push(EntityMatch {
                entity_type: EntityType::Identifier,
                value: val.to_string(),
                offset: m.start(),
            });
        }
    }

    matches
}

/// Extract entities from chunk text, creating Entity nodes and Mentions edges.
/// Deduplicates entities by name + type.
pub fn extract_entities(
    text: &str,
    chunk_id: u64,
    store: &GraphStore,
    next_id: &mut u64,
) -> Result<(Vec<Node>, Vec<Edge>)> {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    let entity_matches = extract_entities_from_text(text);

    for em in &entity_matches {
        // Check if entity already exists
        let existing = store.find_entity_by_name(&em.value)?;
        let entity_id = if let Some(existing_node) = existing.first() {
            existing_node.id
        } else {
            let id = *next_id;
            *next_id += 1;
            nodes.push(Node {
                id,
                kind: NodeKind::Entity,
                name: em.value.clone(),
                properties: NodeProperties {
                    entity_type: Some(em.entity_type.clone()),
                    ..Default::default()
                },
            });
            id
        };

        // Create Mentions edge from chunk to entity
        edges.push(Edge {
            from: chunk_id,
            to: entity_id,
            kind: EdgeKind::Mentions,
            weight: 1.0,
            properties: EdgeProperties::default(),
        });
    }

    Ok((nodes, edges))
}

/// Build co-occurrence edges between entities that appear in the same chunk.
pub fn build_cooccurrence_edges(store: &GraphStore, nodes: &[Node]) -> Result<()> {
    for node in nodes {
        if node.kind != NodeKind::Chunk {
            continue;
        }

        // Get all entities mentioned by this chunk
        let mention_edges = store.get_edges_from(node.id, "mentions")?;
        let entity_ids: Vec<u64> = mention_edges.iter().map(|e| e.to).collect();

        // Create co-occurrence edges for each pair
        for i in 0..entity_ids.len() {
            for j in (i + 1)..entity_ids.len() {
                let a = entity_ids[i];
                let b = entity_ids[j];

                // Check if edge already exists
                if let Some(existing) = store.get_edge(a, b, "co_occurs")? {
                    store.update_edge_weight(a, b, "co_occurs", existing.weight + 1.0)?;
                } else {
                    store.insert_edge(&Edge {
                        from: a,
                        to: b,
                        kind: EdgeKind::CoOccurs,
                        weight: 1.0,
                        properties: EdgeProperties::default(),
                    })?;
                }
            }
        }
    }

    Ok(())
}
