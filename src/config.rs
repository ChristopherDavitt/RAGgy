use anyhow::Result;
use serde::{Serialize, Deserialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub index: IndexConfig,
    pub query: QueryConfig,
    pub llm: LlmConfig,
    #[serde(default)]
    pub embedding: EmbeddingConfig,
    #[serde(default)]
    pub ner: NerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexConfig {
    pub exclude_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryConfig {
    pub default_format: String,
    pub default_limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub enabled: bool,
    pub model: String,
    pub alpha: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NerConfig {
    pub enabled: bool,
    pub model: String,
    pub labels: Vec<String>,
    pub threshold: f32,
}

impl Default for NerConfig {
    fn default() -> Self {
        NerConfig {
            enabled: true,
            model: "gliner-small-v2.1".to_string(),
            labels: vec![
                "person".to_string(),
                "organization".to_string(),
                "location".to_string(),
                "date".to_string(),
                "money".to_string(),
            ],
            threshold: 0.5,
        }
    }
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        EmbeddingConfig {
            enabled: true,
            model: "all-MiniLM-L6-v2".to_string(),
            alpha: 0.5,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            index: IndexConfig {
                exclude_patterns: vec![
                    "node_modules".to_string(),
                    ".git".to_string(),
                    "target".to_string(),
                    "__pycache__".to_string(),
                    ".raggy".to_string(),
                ],
            },
            query: QueryConfig {
                default_format: "human".to_string(),
                default_limit: 10,
            },
            llm: LlmConfig {
                enabled: false,
            },
            embedding: EmbeddingConfig::default(),
            ner: NerConfig::default(),
        }
    }
}

impl Config {
    pub fn load(raggy_dir: &Path) -> Result<Self> {
        let config_path = raggy_dir.join("config.toml");
        if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path)?;
            let config: Config = toml::from_str(&contents)?;
            Ok(config)
        } else {
            Ok(Config::default())
        }
    }

    pub fn save(&self, raggy_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(raggy_dir)?;
        let config_path = raggy_dir.join("config.toml");
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(config_path, contents)?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<String> {
        match key {
            "index.exclude_patterns" => Some(format!("{:?}", self.index.exclude_patterns)),
            "query.default_format" => Some(self.query.default_format.clone()),
            "query.default_limit" => Some(self.query.default_limit.to_string()),
            "llm.enabled" => Some(self.llm.enabled.to_string()),
            "embedding.enabled" => Some(self.embedding.enabled.to_string()),
            "embedding.model" => Some(self.embedding.model.clone()),
            "embedding.alpha" => Some(self.embedding.alpha.to_string()),
            "ner.enabled" => Some(self.ner.enabled.to_string()),
            "ner.model" => Some(self.ner.model.clone()),
            "ner.labels" => Some(format!("{:?}", self.ner.labels)),
            "ner.threshold" => Some(self.ner.threshold.to_string()),
            _ => None,
        }
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "query.default_format" => self.query.default_format = value.to_string(),
            "query.default_limit" => self.query.default_limit = value.parse()?,
            "llm.enabled" => self.llm.enabled = value.parse()?,
            "embedding.enabled" => self.embedding.enabled = value.parse()?,
            "embedding.model" => self.embedding.model = value.to_string(),
            "embedding.alpha" => self.embedding.alpha = value.parse()?,
            "ner.enabled" => self.ner.enabled = value.parse()?,
            "ner.model" => self.ner.model = value.to_string(),
            "ner.threshold" => self.ner.threshold = value.parse()?,
            "ner.labels" => {
                self.ner.labels = value.split(',').map(|s| s.trim().to_string()).collect();
            }
            _ => anyhow::bail!("Unknown config key: {}", key),
        }
        Ok(())
    }
}

/// Global config stored at .raggy/raggy.toml (not per-database).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    pub default_db: String,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        GlobalConfig {
            default_db: "default".to_string(),
        }
    }
}

impl GlobalConfig {
    pub fn load(raggy_dir: &Path) -> Result<Self> {
        let path = raggy_dir.join("raggy.toml");
        if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            let config: GlobalConfig = toml::from_str(&contents)?;
            Ok(config)
        } else {
            Ok(GlobalConfig::default())
        }
    }

    pub fn save(&self, raggy_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(raggy_dir)?;
        let path = raggy_dir.join("raggy.toml");
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(path, contents)?;
        Ok(())
    }
}

/// Find the .raggy directory, starting from current dir and walking up.
/// If not found, returns .raggy in the current directory.
pub fn find_raggy_dir() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut dir = cwd.as_path();
    loop {
        let raggy_dir = dir.join(".raggy");
        if raggy_dir.exists() {
            return raggy_dir;
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return cwd.join(".raggy"),
        }
    }
}
