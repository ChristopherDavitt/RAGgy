use anyhow::{Result, Context};
use std::path::{Path, PathBuf};

use crate::config::{Config, GlobalConfig};

/// Resolved workspace paths for a specific database.
pub struct Workspace {
    pub raggy_dir: PathBuf,   // .raggy/
    pub db_name: String,      // e.g. "default"
    pub db_dir: PathBuf,      // .raggy/dbs/default/
    pub models_dir: PathBuf,  // .raggy/models/ (shared)
}

impl Workspace {
    /// Resolve which database to use and return all paths.
    /// Priority: db_override > RAGGY_DB env > global config > "default"
    pub fn resolve(db_override: Option<&str>) -> Result<Self> {
        let raggy_dir = find_raggy_root();

        // Auto-migrate from flat layout if needed
        maybe_migrate(&raggy_dir)?;

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
        // Write default config
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

/// Find the .raggy directory, starting from current dir and walking up.
fn find_raggy_root() -> PathBuf {
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

/// Auto-migrate from flat .raggy/ layout to .raggy/dbs/default/.
fn maybe_migrate(raggy_dir: &Path) -> Result<()> {
    let old_db = raggy_dir.join("raggy.db");
    let dbs_dir = raggy_dir.join("dbs");

    // Only migrate if old layout exists and new layout doesn't
    if !old_db.exists() || dbs_dir.exists() {
        return Ok(());
    }

    eprintln!("Migrating index to multi-database layout...");

    let default_dir = dbs_dir.join("default");
    std::fs::create_dir_all(&default_dir)
        .context("Failed to create dbs/default directory")?;

    // Move database files
    let files_to_move = ["raggy.db", "raggy.db-wal", "raggy.db-shm", "config.toml"];
    for file in &files_to_move {
        let src = raggy_dir.join(file);
        if src.exists() {
            let dst = default_dir.join(file);
            std::fs::rename(&src, &dst)
                .with_context(|| format!("Failed to move {}", file))?;
        }
    }

    // Move directories
    let dirs_to_move = ["tantivy", "vectors"];
    for dir in &dirs_to_move {
        let src = raggy_dir.join(dir);
        if src.exists() {
            let dst = default_dir.join(dir);
            std::fs::rename(&src, &dst)
                .with_context(|| format!("Failed to move {}/", dir))?;
        }
    }

    // models/ stays in place (shared)

    // Write global config
    let global = GlobalConfig::default();
    global.save(raggy_dir)?;

    eprintln!("Migrated existing index to database 'default'.");
    eprintln!("Use `raggy db list` to see databases.\n");

    Ok(())
}
