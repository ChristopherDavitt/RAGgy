use anyhow::Result;
use std::path::Path;

use crate::graph::{Node, Edge, NodeKind, NodeProperties, EdgeKind, EdgeProperties, ContentType};

/// Extract code files into document + chunk nodes.
/// Uses a simple heuristic approach: split by top-level definitions.
/// Tree-sitter integration is available but we use a simpler line-based approach
/// for reliability across platforms.
pub fn extract(
    path: &Path,
    content: &str,
    language: &str,
    next_id: &mut u64,
) -> Result<(Vec<Node>, Vec<Edge>)> {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    let doc_id = *next_id;
    *next_id += 1;

    let file_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();

    nodes.push(Node {
        id: doc_id,
        kind: NodeKind::Document,
        name: file_name.clone(),
        properties: NodeProperties {
            path: Some(path.to_path_buf()),
            content_type: Some(ContentType::Code { language: language.to_string() }),
            title: Some(file_name),
            text: Some(content.to_string()),
            ..Default::default()
        },
    });

    // Split code into logical sections based on language patterns
    let sections = split_code_sections(content, language);

    let mut prev_section_id: Option<u64> = None;

    for (name, text, start_line, end_line) in sections {
        let section_id = *next_id;
        *next_id += 1;

        nodes.push(Node {
            id: section_id,
            kind: NodeKind::Section,
            name: name.clone(),
            properties: NodeProperties {
                section_kind: Some("function".to_string()),
                start_line: Some(start_line),
                end_line: Some(end_line),
                title: Some(name),
                ..Default::default()
            },
        });

        edges.push(Edge {
            from: doc_id,
            to: section_id,
            kind: EdgeKind::Contains,
            weight: 1.0,
            properties: EdgeProperties::default(),
        });

        // Create chunk with the actual code text
        let chunk_id = *next_id;
        *next_id += 1;

        nodes.push(Node {
            id: chunk_id,
            kind: NodeKind::Chunk,
            name: format!("code_chunk_{}", chunk_id),
            properties: NodeProperties {
                text: Some(text),
                parent_section_id: Some(section_id),
                ..Default::default()
            },
        });

        edges.push(Edge {
            from: section_id,
            to: chunk_id,
            kind: EdgeKind::Contains,
            weight: 1.0,
            properties: EdgeProperties::default(),
        });

        if let Some(prev_id) = prev_section_id {
            edges.push(Edge {
                from: prev_id,
                to: section_id,
                kind: EdgeKind::Follows,
                weight: 1.0,
                properties: EdgeProperties::default(),
            });
        }

        prev_section_id = Some(section_id);
    }

    // If no sections were found, create a single chunk with all content
    if prev_section_id.is_none() && !content.trim().is_empty() {
        let chunk_id = *next_id;
        *next_id += 1;

        nodes.push(Node {
            id: chunk_id,
            kind: NodeKind::Chunk,
            name: format!("chunk_{}", chunk_id),
            properties: NodeProperties {
                text: Some(content.to_string()),
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
    }

    Ok((nodes, edges))
}

fn split_code_sections(content: &str, language: &str) -> Vec<(String, String, u32, u32)> {
    let mut sections = Vec::new();
    let lines: Vec<&str> = content.lines().collect();

    let definition_patterns: Vec<&str> = match language {
        "rust" => vec!["fn ", "pub fn ", "struct ", "pub struct ", "enum ", "pub enum ", "impl ", "trait ", "pub trait ", "mod ", "pub mod "],
        "python" => vec!["def ", "class ", "async def "],
        "javascript" | "typescript" => vec!["function ", "class ", "const ", "export function ", "export class ", "export const ", "export default "],
        "go" => vec!["func ", "type "],
        "solidity" => vec!["function ", "contract ", "library ", "interface "],
        _ => vec!["fn ", "def ", "function ", "class "],
    };

    let mut current_section_start: Option<(String, usize)> = None;
    let mut current_lines = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        let is_definition = definition_patterns.iter().any(|p| trimmed.starts_with(p));

        if is_definition {
            // Flush previous section
            if let Some((name, start)) = current_section_start.take() {
                if !current_lines.is_empty() {
                    sections.push((
                        name,
                        current_lines.join("\n"),
                        start as u32 + 1,
                        i as u32,
                    ));
                }
            }

            let name = extract_definition_name(trimmed, language);
            current_section_start = Some((name, i));
            current_lines = vec![*line];
        } else if current_section_start.is_some() {
            current_lines.push(*line);
        }
    }

    // Flush last section
    if let Some((name, start)) = current_section_start {
        if !current_lines.is_empty() {
            sections.push((
                name,
                current_lines.join("\n"),
                start as u32 + 1,
                lines.len() as u32,
            ));
        }
    }

    sections
}

fn extract_definition_name(line: &str, language: &str) -> String {
    let cleaned = line.trim()
        .trim_start_matches("pub ")
        .trim_start_matches("export ")
        .trim_start_matches("default ")
        .trim_start_matches("async ");

    // Extract the name token after the keyword
    let after_keyword = if let Some(rest) = cleaned.strip_prefix("fn ") {
        rest
    } else if let Some(rest) = cleaned.strip_prefix("def ") {
        rest
    } else if let Some(rest) = cleaned.strip_prefix("function ") {
        rest
    } else if let Some(rest) = cleaned.strip_prefix("class ") {
        rest
    } else if let Some(rest) = cleaned.strip_prefix("struct ") {
        rest
    } else if let Some(rest) = cleaned.strip_prefix("enum ") {
        rest
    } else if let Some(rest) = cleaned.strip_prefix("impl ") {
        rest
    } else if let Some(rest) = cleaned.strip_prefix("trait ") {
        rest
    } else if let Some(rest) = cleaned.strip_prefix("mod ") {
        rest
    } else if let Some(rest) = cleaned.strip_prefix("type ") {
        rest
    } else if let Some(rest) = cleaned.strip_prefix("func ") {
        rest
    } else if let Some(rest) = cleaned.strip_prefix("const ") {
        rest
    } else if let Some(rest) = cleaned.strip_prefix("contract ") {
        rest
    } else if let Some(rest) = cleaned.strip_prefix("library ") {
        rest
    } else if let Some(rest) = cleaned.strip_prefix("interface ") {
        rest
    } else {
        cleaned
    };

    // Take the name up to the first non-identifier char
    let name: String = after_keyword.chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();

    if name.is_empty() {
        format!("anonymous_{}", language)
    } else {
        name
    }
}
