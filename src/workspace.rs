use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::config::{Config, GlobalConfig};

/// Resolved workspace paths for a specific database.
pub struct Workspace {
    pub raggy_dir: PathBuf,   // ~/.raggy/ (or RAGGY_HOME / --home override)
    pub db_name: String,      // e.g. "default"
    pub db_dir: PathBuf,      // ~/.raggy/dbs/default/
    pub models_dir: PathBuf,  // ~/.raggy/models/ (shared across databases)
}

impl Workspace {
    /// Resolve which database to use and return all paths.
    ///
    /// Resolution order:
    ///   home_override  (--home CLI flag)
    ///   RAGGY_HOME     (env var)
    ///   ~/.raggy/      (global default)
    ///
    /// If the resolved home does not exist, an error is returned asking
    /// the user to run `raggy init`.
    pub fn resolve(home_override: Option<&Path>, db_override: Option<&str>) -> Result<Self> {
        let raggy_dir = home_override
            .map(|p| p.to_path_buf())
            .unwrap_or_else(default_home);

        if !raggy_dir.exists() {
            anyhow::bail!(
                "RAGgy home not found at {}.\nRun `raggy init` to set up RAGgy.",
                raggy_dir.display()
            );
        }

        let global = GlobalConfig::load(&raggy_dir)?;

        let db_name = db_override
            .map(|s| s.to_string())
            .or_else(|| std::env::var("RAGGY_DB").ok())
            .unwrap_or(global.default_db);

        let db_dir = raggy_dir.join("dbs").join(&db_name);
        let models_dir = raggy_dir.join("models");

        Ok(Workspace {
            raggy_dir,
            db_name,
            db_dir,
            models_dir,
        })
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.db_dir)?;
        std::fs::create_dir_all(&self.models_dir)?;
        std::fs::create_dir_all(&self.documents_dir())?;
        Ok(())
    }

    pub fn db_path(&self) -> PathBuf {
        self.db_dir.join("raggy.db")
    }

    pub fn tantivy_dir(&self) -> PathBuf {
        self.db_dir.join("tantivy")
    }

    pub fn vector_dir(&self) -> PathBuf {
        self.db_dir.join("vectors")
    }

    /// Directory where processed document copies are stored.
    pub fn documents_dir(&self) -> PathBuf {
        self.db_dir.join("documents")
    }

    pub fn config(&self) -> Result<Config> {
        Config::load(&self.db_dir)
    }

    pub fn save_config(&self, config: &Config) -> Result<()> {
        config.save(&self.db_dir)
    }

    pub fn embedding_model_dir(&self, config: &Config) -> PathBuf {
        self.models_dir.join(&config.embedding.model)
    }

    pub fn ner_model_dir(&self, config: &Config) -> PathBuf {
        self.models_dir.join(&config.ner.model)
    }

    pub fn exists(&self) -> bool {
        self.db_path().exists()
    }

    pub fn list_databases(&self) -> Result<Vec<String>> {
        let dbs_dir = self.raggy_dir.join("dbs");
        if !dbs_dir.exists() {
            return Ok(vec![]);
        }
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&dbs_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    names.push(name.to_string());
                }
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn create_database(raggy_dir: &Path, name: &str) -> Result<PathBuf> {
        let db_dir = raggy_dir.join("dbs").join(name);
        if db_dir.exists() {
            anyhow::bail!("Database '{}' already exists", name);
        }
        std::fs::create_dir_all(&db_dir)?;
        let config = Config::default();
        config.save(&db_dir)?;
        Ok(db_dir)
    }

    pub fn delete_database(raggy_dir: &Path, name: &str) -> Result<()> {
        if name == "default" {
            anyhow::bail!("Cannot delete the default database");
        }
        let db_dir = raggy_dir.join("dbs").join(name);
        if !db_dir.exists() {
            anyhow::bail!("Database '{}' does not exist", name);
        }
        std::fs::remove_dir_all(&db_dir)?;
        Ok(())
    }
}

/// Return the default RAGgy home directory.
///
/// Priority: RAGGY_HOME env var → ~/.raggy/
pub fn default_home() -> PathBuf {
    if let Ok(h) = std::env::var("RAGGY_HOME") {
        return PathBuf::from(h);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".raggy")
}
