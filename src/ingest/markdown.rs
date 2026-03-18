use anyhow::Result;
use pulldown_cmark::{Event, Tag, TagEnd, Parser, HeadingLevel};
use std::path::Path;

use crate::graph::{Node, Edge, NodeKind, NodeProperties, EdgeKind, EdgeProperties, ContentType};

pub fn extract(path: &Path, content: &str, next_id: &mut u64) -> Result<(Vec<Node>, Vec<Edge>)> {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    // Create document node
    let doc_id = *next_id;
    *next_id += 1;

    let title = extract_title(content);

    nodes.push(Node {
        id: doc_id,
        kind: NodeKind::Document,
        name: path.file_name().unwrap_or_default().to_string_lossy().to_string(),
        properties: NodeProperties {
            path: Some(path.to_path_buf()),
            content_type: Some(ContentType::Markdown),
            title: title.clone(),
            text: Some(content.to_string()),
            ..Default::default()
        },
    });

    let parser = Parser::new(content);

    let mut section_stack: Vec<(u64, u32)> = Vec::new(); // (node_id, depth)
    let mut current_heading_text = String::new();
    let mut current_heading_level: u32 = 0;
    let mut in_heading = false;
    let mut chunk_buffer = String::new();
    let mut in_code_block = false;
    let mut code_buffer = String::new();
    let mut last_section_at_depth: Vec<Option<u64>> = vec![None; 7]; // h1-h6
    let mut line_counter: u32 = 1;

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                // Flush any pending chunk
                if !chunk_buffer.trim().is_empty() {
                    let chunk_id = *next_id;
                    *next_id += 1;
                    let parent = section_stack.last().map(|(id, _)| *id).unwrap_or(doc_id);
                    nodes.push(Node {
                        id: chunk_id,
                        kind: NodeKind::Chunk,
                        name: format!("chunk_{}", chunk_id),
                        properties: NodeProperties {
                            text: Some(chunk_buffer.trim().to_string()),
                            parent_section_id: Some(parent),
                            ..Default::default()
                        },
                    });
                    edges.push(Edge {
                        from: parent,
                        to: chunk_id,
                        kind: EdgeKind::Contains,
                        weight: 1.0,
                        properties: EdgeProperties::default(),
                    });
                    chunk_buffer.clear();
                }

                in_heading = true;
                current_heading_text.clear();
                current_heading_level = heading_level_to_u32(level);
            }
            Event::End(TagEnd::Heading(_)) => {
                in_heading = false;
                let section_id = *next_id;
                *next_id += 1;

                // Pop sections at same or deeper level
                while let Some(&(_, depth)) = section_stack.last() {
                    if depth >= current_heading_level {
                        section_stack.pop();
                    } else {
                        break;
                    }
                }

                let parent = section_stack.last().map(|(id, _)| *id).unwrap_or(doc_id);

                nodes.push(Node {
                    id: section_id,
                    kind: NodeKind::Section,
                    name: current_heading_text.trim().to_string(),
                    properties: NodeProperties {
                        depth: Some(current_heading_level),
                        section_kind: Some("heading".to_string()),
                        title: Some(current_heading_text.trim().to_string()),
                        text: Some(current_heading_text.trim().to_string()),
                        start_line: Some(line_counter),
                        ..Default::default()
                    },
                });

                edges.push(Edge {
                    from: parent,
                    to: section_id,
                    kind: EdgeKind::Contains,
                    weight: 1.0,
                    properties: EdgeProperties::default(),
                });

                // Create Follows edge from previous section at same depth
                let depth_idx = current_heading_level as usize;
                if depth_idx < last_section_at_depth.len() {
                    if let Some(prev_id) = last_section_at_depth[depth_idx] {
                        edges.push(Edge {
                            from: prev_id,
                            to: section_id,
                            kind: EdgeKind::Follows,
                            weight: 1.0,
                            properties: EdgeProperties {
                                order: Some(depth_idx as u32),
                                ..Default::default()
                            },
                        });
                    }
                    last_section_at_depth[depth_idx] = Some(section_id);
                }

                section_stack.push((section_id, current_heading_level));
            }
            Event::Start(Tag::CodeBlock(_)) => {
                in_code_block = true;
                code_buffer.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                if !code_buffer.trim().is_empty() {
                    let chunk_id = *next_id;
                    *next_id += 1;
                    let parent = section_stack.last().map(|(id, _)| *id).unwrap_or(doc_id);
                    nodes.push(Node {
                        id: chunk_id,
                        kind: NodeKind::Chunk,
                        name: format!("code_chunk_{}", chunk_id),
                        properties: NodeProperties {
                            text: Some(code_buffer.trim().to_string()),
                            parent_section_id: Some(parent),
                            section_kind: Some("code".to_string()),
                            ..Default::default()
                        },
                    });
                    edges.push(Edge {
                        from: parent,
                        to: chunk_id,
                        kind: EdgeKind::Contains,
                        weight: 1.0,
                        properties: EdgeProperties::default(),
                    });
                }
                code_buffer.clear();
            }
            Event::Text(text) => {
                if in_heading {
                    current_heading_text.push_str(&text);
                } else if in_code_block {
                    code_buffer.push_str(&text);
                } else {
                    chunk_buffer.push_str(&text);
                }
                line_counter += text.matches('\n').count() as u32;
            }
            Event::SoftBreak | Event::HardBreak => {
                if !in_heading {
                    chunk_buffer.push('\n');
                }
                line_counter += 1;
            }
            Event::End(TagEnd::Paragraph) => {
                if !chunk_buffer.trim().is_empty() {
                    let chunk_id = *next_id;
                    *next_id += 1;
                    let parent = section_stack.last().map(|(id, _)| *id).unwrap_or(doc_id);
                    nodes.push(Node {
                        id: chunk_id,
                        kind: NodeKind::Chunk,
                        name: format!("chunk_{}", chunk_id),
                        properties: NodeProperties {
                            text: Some(chunk_buffer.trim().to_string()),
                            parent_section_id: Some(parent),
                            ..Default::default()
                        },
                    });
                    edges.push(Edge {
                        from: parent,
                        to: chunk_id,
                        kind: EdgeKind::Contains,
                        weight: 1.0,
                        properties: EdgeProperties::default(),
                    });
                    chunk_buffer.clear();
                }
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                let url = dest_url.to_string();
                // If it's a relative path, create a References edge
                if !url.starts_with("http://") && !url.starts_with("https://") && !url.starts_with('#') {
                    // Store as a reference - will be resolved during graph building
                    chunk_buffer.push_str(&format!("[link:{}]", url));
                }
            }
            _ => {}
        }
    }

    // Flush remaining chunk buffer
    if !chunk_buffer.trim().is_empty() {
        let chunk_id = *next_id;
        *next_id += 1;
        let parent = section_stack.last().map(|(id, _)| *id).unwrap_or(doc_id);
        nodes.push(Node {
            id: chunk_id,
            kind: NodeKind::Chunk,
            name: format!("chunk_{}", chunk_id),
            properties: NodeProperties {
                text: Some(chunk_buffer.trim().to_string()),
                parent_section_id: Some(parent),
                ..Default::default()
            },
        });
        edges.push(Edge {
            from: parent,
            to: chunk_id,
            kind: EdgeKind::Contains,
            weight: 1.0,
            properties: EdgeProperties::default(),
        });
    }

    Ok((nodes, edges))
}

fn extract_title(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(title) = trimmed.strip_prefix("# ") {
            return Some(title.trim().to_string());
        }
    }
    None
}

fn heading_level_to_u32(level: HeadingLevel) -> u32 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}
