pub mod store;
pub mod traverse;

use std::path::PathBuf;
use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};

// ── Node Types ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum NodeKind {
    Document,
    Section,
    Chunk,
    Entity,
}

impl NodeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeKind::Document => "document",
            NodeKind::Section => "section",
            NodeKind::Chunk => "chunk",
            NodeKind::Entity => "entity",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "document" => Some(NodeKind::Document),
            "section" => Some(NodeKind::Section),
            "chunk" => Some(NodeKind::Chunk),
            "entity" => Some(NodeKind::Entity),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: u64,
    pub kind: NodeKind,
    pub name: String,
    pub properties: NodeProperties,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NodeProperties {
    // Document properties
    pub path: Option<PathBuf>,
    pub content_type: Option<ContentType>,
    pub title: Option<String>,
    pub modified: Option<DateTime<Utc>>,
    pub size: Option<u64>,
    pub content_hash: Option<String>,

    // Section properties
    pub depth: Option<u32>,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub section_kind: Option<String>,

    // Chunk properties
    pub text: Option<String>,
    pub parent_section_id: Option<u64>,

    // Entity properties
    pub entity_type: Option<EntityType>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ContentType {
    Markdown,
    PlainText,
    Code { language: String },
    Json,
    Yaml,
    Toml,
}

impl ContentType {
    pub fn as_str(&self) -> String {
        match self {
            ContentType::Markdown => "markdown".to_string(),
            ContentType::PlainText => "plaintext".to_string(),
            ContentType::Code { language } => format!("code:{}", language),
            ContentType::Json => "json".to_string(),
            ContentType::Yaml => "yaml".to_string(),
            ContentType::Toml => "toml".to_string(),
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        if let Some(lang) = s.strip_prefix("code:") {
            return Some(ContentType::Code { language: lang.to_string() });
        }
        match s {
            "markdown" => Some(ContentType::Markdown),
            "plaintext" => Some(ContentType::PlainText),
            "json" => Some(ContentType::Json),
            "yaml" => Some(ContentType::Yaml),
            "toml" => Some(ContentType::Toml),
            _ => None,
        }
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "md" | "mdx" => Some(ContentType::Markdown),
            "txt" | "text" => Some(ContentType::PlainText),
            "rs" => Some(ContentType::Code { language: "rust".to_string() }),
            "js" => Some(ContentType::Code { language: "javascript".to_string() }),
            "ts" => Some(ContentType::Code { language: "typescript".to_string() }),
            "py" => Some(ContentType::Code { language: "python".to_string() }),
            "go" => Some(ContentType::Code { language: "go".to_string() }),
            "sol" => Some(ContentType::Code { language: "solidity".to_string() }),
            "json" => Some(ContentType::Json),
            "yaml" | "yml" => Some(ContentType::Yaml),
            "toml" => Some(ContentType::Toml),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum EntityType {
    Person,
    Organization,
    Location,
    Url,
    FilePath,
    Email,
    Date,
    Version,
    Identifier,
}

impl EntityType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EntityType::Person => "person",
            EntityType::Organization => "organization",
            EntityType::Location => "location",
            EntityType::Url => "url",
            EntityType::FilePath => "filepath",
            EntityType::Email => "email",
            EntityType::Date => "date",
            EntityType::Version => "version",
            EntityType::Identifier => "identifier",
        }
    }
}

// ── Edge Types ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EdgeKind {
    Contains,
    References,
    Mentions,
    CoOccurs,
    Follows,
}

impl EdgeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EdgeKind::Contains => "contains",
            EdgeKind::References => "references",
            EdgeKind::Mentions => "mentions",
            EdgeKind::CoOccurs => "co_occurs",
            EdgeKind::Follows => "follows",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "contains" => Some(EdgeKind::Contains),
            "references" => Some(EdgeKind::References),
            "mentions" => Some(EdgeKind::Mentions),
            "co_occurs" => Some(EdgeKind::CoOccurs),
            "follows" => Some(EdgeKind::Follows),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub from: u64,
    pub to: u64,
    pub kind: EdgeKind,
    pub weight: f32,
    pub properties: EdgeProperties,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EdgeProperties {
    pub ref_type: Option<String>,
    pub count: Option<u32>,
    pub order: Option<u32>,
}
