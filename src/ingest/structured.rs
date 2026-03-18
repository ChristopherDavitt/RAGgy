use anyhow::Result;
use std::path::Path;

use crate::graph::{Node, Edge, NodeKind, NodeProperties, EdgeKind, EdgeProperties, ContentType};

pub fn extract(
    path: &Path,
    content: &str,
    content_type: &ContentType,
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
            content_type: Some(content_type.clone()),
            title: Some(file_name),
            text: Some(content.to_string()),
            ..Default::default()
        },
    });

    // Parse the structured data and extract top-level keys as sections
    let keys = extract_top_level_keys(content, content_type);

    for (key, value_text) in keys {
        let section_id = *next_id;
        *next_id += 1;

        nodes.push(Node {
            id: section_id,
            kind: NodeKind::Section,
            name: key.clone(),
            properties: NodeProperties {
                section_kind: Some("key".to_string()),
                title: Some(key),
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

        let chunk_id = *next_id;
        *next_id += 1;

        nodes.push(Node {
            id: chunk_id,
            kind: NodeKind::Chunk,
            name: format!("chunk_{}", chunk_id),
            properties: NodeProperties {
                text: Some(value_text),
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
    }

    // If no keys were extracted, put the whole content as a chunk
    if nodes.len() == 1 && !content.trim().is_empty() {
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

fn extract_top_level_keys(content: &str, content_type: &ContentType) -> Vec<(String, String)> {
    match content_type {
        ContentType::Json => extract_json_keys(content),
        ContentType::Yaml => extract_yaml_keys(content),
        ContentType::Toml => extract_toml_keys(content),
        _ => Vec::new(),
    }
}

fn extract_json_keys(content: &str) -> Vec<(String, String)> {
    let value: Result<serde_json::Value, _> = serde_json::from_str(content);
    match value {
        Ok(serde_json::Value::Object(map)) => {
            map.into_iter()
                .map(|(k, v)| {
                    let text = serde_json::to_string_pretty(&v).unwrap_or_default();
                    (k, text)
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

fn extract_yaml_keys(content: &str) -> Vec<(String, String)> {
    let value: Result<serde_yaml::Value, _> = serde_yaml::from_str(content);
    match value {
        Ok(serde_yaml::Value::Mapping(map)) => {
            map.into_iter()
                .filter_map(|(k, v)| {
                    let key = match k {
                        serde_yaml::Value::String(s) => s,
                        _ => return None,
                    };
                    let text = serde_yaml::to_string(&v).unwrap_or_default();
                    Some((key, text))
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

fn extract_toml_keys(content: &str) -> Vec<(String, String)> {
    let value: Result<toml::Value, _> = toml::from_str(content);
    match value {
        Ok(toml::Value::Table(map)) => {
            map.into_iter()
                .map(|(k, v)| {
                    let text = toml::to_string_pretty(&v).unwrap_or_default();
                    (k, text)
                })
                .collect()
        }
        _ => Vec::new(),
    }
}
