use anyhow::Result;
use std::path::Path;

use crate::graph::{Node, Edge, NodeKind, NodeProperties, EdgeKind, EdgeProperties, ContentType};

/// Split plain text into chunks of ~500-800 chars using a multi-strategy approach:
/// 1. Split on paragraph boundaries (\n\n)
/// 2. Split large chunks on sentence boundaries (. ! ?)
/// 3. Merge adjacent tiny chunks (<200 chars)
pub fn chunk_text(content: &str) -> Vec<String> {
    const MAX_CHUNK: usize = 1000;
    const MIN_CHUNK: usize = 200;

    // Step 1: split on paragraph boundaries
    let paragraphs: Vec<&str> = content.split("\n\n")
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();

    // Step 2: split oversized paragraphs on sentence boundaries
    let mut pieces: Vec<String> = Vec::new();
    for para in paragraphs {
        if para.len() <= MAX_CHUNK {
            pieces.push(para.to_string());
        } else {
            // Split on sentence endings followed by a space
            let mut remaining = para;
            while remaining.len() > MAX_CHUNK {
                // Find the last sentence boundary within MAX_CHUNK
                let search_region = &remaining[..MAX_CHUNK];
                let split_pos = search_region.rmatch_indices(". ")
                    .chain(search_region.rmatch_indices("! "))
                    .chain(search_region.rmatch_indices("? "))
                    .map(|(i, s)| i + s.len())
                    .max();

                if let Some(pos) = split_pos {
                    pieces.push(remaining[..pos].trim().to_string());
                    remaining = remaining[pos..].trim();
                } else {
                    // No sentence boundary found; split at word boundary near MAX_CHUNK
                    let split_at = remaining[..MAX_CHUNK]
                        .rfind(' ')
                        .unwrap_or(MAX_CHUNK);
                    pieces.push(remaining[..split_at].trim().to_string());
                    remaining = remaining[split_at..].trim();
                }
            }
            if !remaining.is_empty() {
                pieces.push(remaining.to_string());
            }
        }
    }

    // Step 3: merge adjacent tiny chunks
    let mut merged: Vec<String> = Vec::new();
    for piece in pieces {
        if let Some(last) = merged.last_mut() {
            if last.len() < MIN_CHUNK || piece.len() < MIN_CHUNK {
                if last.len() + piece.len() + 1 <= MAX_CHUNK {
                    last.push(' ');
                    last.push_str(&piece);
                    continue;
                }
            }
        }
        merged.push(piece);
    }

    merged.into_iter().filter(|c| !c.is_empty()).collect()
}

pub fn extract(path: &Path, content: &str, next_id: &mut u64) -> Result<(Vec<Node>, Vec<Edge>)> {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    // Create document node
    let doc_id = *next_id;
    *next_id += 1;

    let file_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();

    nodes.push(Node {
        id: doc_id,
        kind: NodeKind::Document,
        name: file_name.clone(),
        properties: NodeProperties {
            path: Some(path.to_path_buf()),
            content_type: Some(ContentType::PlainText),
            title: Some(file_name),
            text: Some(content.to_string()),
            ..Default::default()
        },
    });

    let chunks = chunk_text(content);

    let mut prev_chunk_id: Option<u64> = None;

    for chunk in &chunks {
        let chunk_id = *next_id;
        *next_id += 1;

        nodes.push(Node {
            id: chunk_id,
            kind: NodeKind::Chunk,
            name: format!("chunk_{}", chunk_id),
            properties: NodeProperties {
                text: Some(chunk.to_string()),
                parent_section_id: Some(doc_id),
                ..Default::default()
            },
        });

        edges.push(Edge {
            from: doc_id,
            to: chunk_id,
            kind: EdgeKind::Contains,
            weight: 1.0,
            properties: EdgeProperties::default(),
        });

        if let Some(prev_id) = prev_chunk_id {
            edges.push(Edge {
                from: prev_id,
                to: chunk_id,
                kind: EdgeKind::Follows,
                weight: 1.0,
                properties: EdgeProperties::default(),
            });
        }

        prev_chunk_id = Some(chunk_id);
    }

    Ok((nodes, edges))
}
