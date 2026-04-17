/// `raggy init` — set up the RAGgy home directory and download required models.
///
/// Run once after installing RAGgy:
///   raggy init
///
/// This creates ~/.raggy/ (or $RAGGY_HOME), writes default configs, and
/// downloads the two ONNX model files needed for embedding and NER.
/// Re-running is safe — already-present files are skipped.

use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result};
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};

use crate::config::{Config, GlobalConfig};

const EMBEDDING_MODEL: &str = "all-MiniLM-L6-v2";
const NER_MODEL: &str = "gliner-small-v2.1";

struct ModelFile {
    url: &'static str,
    dest: &'static str,
    label: &'static str,
}

const EMBEDDING_FILES: &[ModelFile] = &[
    ModelFile {
        url: "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx",
        dest: "model.onnx",
        label: "all-MiniLM-L6-v2 model.onnx (~23 MB)",
    },
    ModelFile {
        url: "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json",
        dest: "tokenizer.json",
        label: "all-MiniLM-L6-v2 tokenizer.json",
    },
];

const NER_FILES: &[ModelFile] = &[
    ModelFile {
        url: "https://huggingface.co/onnx-community/gliner_small-v2.1/resolve/main/model.onnx",
        dest: "model.onnx",
        label: "gliner-small-v2.1 model.onnx (~166 MB)",
    },
    ModelFile {
        url: "https://huggingface.co/urchade/gliner_small-v2.1/resolve/main/tokenizer.json",
        dest: "tokenizer.json",
        label: "gliner-small-v2.1 tokenizer.json",
    },
];

pub fn run(raggy_home: &Path) -> Result<()> {
    println!("{} Initializing RAGgy at {}", "→".blue().bold(), raggy_home.display());

    // Create directory structure
    let models_dir = raggy_home.join("models");
    let default_db_dir = raggy_home.join("dbs").join("default");
    let default_docs_dir = default_db_dir.join("documents");

    for dir in &[&models_dir, &default_db_dir, &default_docs_dir] {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create {}", dir.display()))?;
    }

    // Write global config if missing
    let global_config_path = raggy_home.join("raggy.toml");
    if !global_config_path.exists() {
        GlobalConfig::default().save(raggy_home)?;
    }

    // Write default db config if missing
    let db_config_path = default_db_dir.join("config.toml");
    if !db_config_path.exists() {
        Config::default().save(&default_db_dir)?;
    }

    println!("{} Directory structure ready\n", "✓".green().bold());

    // Download embedding model
    let embedding_dir = models_dir.join(EMBEDDING_MODEL);
    std::fs::create_dir_all(&embedding_dir)?;
    println!("{} Embedding model ({})", "↓".blue().bold(), EMBEDDING_MODEL);
    for f in EMBEDDING_FILES {
        download_file(f.url, &embedding_dir.join(f.dest), f.label)?;
    }

    // Download NER model
    let ner_dir = models_dir.join(NER_MODEL);
    std::fs::create_dir_all(&ner_dir)?;
    println!("\n{} NER model ({})", "↓".blue().bold(), NER_MODEL);
    for f in NER_FILES {
        download_file(f.url, &ner_dir.join(f.dest), f.label)?;
    }

    println!("\n{} RAGgy is ready!", "✓".green().bold());
    println!("  Home:    {}", raggy_home.display());
    println!("  Index:   {}", "raggy index <path>".bold());
    println!("  Search:  {}", "raggy query \"...\"".bold());
    println!("  Serve:   {}", "raggy serve".bold());
    println!("  MCP:     {}", "raggy mcp".bold());

    Ok(())
}

fn download_file(url: &str, dest: &Path, label: &str) -> Result<()> {
    if dest.exists() {
        println!("  {} {} (already present)", "✓".green(), label);
        return Ok(());
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .context("Failed to create HTTP client")?;

    let mut response = client
        .get(url)
        .send()
        .with_context(|| format!("Failed to connect to {}", url))?
        .error_for_status()
        .with_context(|| format!("HTTP error downloading {}", label))?;

    let total_size = response.content_length().unwrap_or(0);

    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("  {msg:45} [{bar:30}] {bytes:>10}/{total_bytes:10}  {eta}")
            .unwrap()
            .progress_chars("=> "),
    );
    pb.set_message(label.to_string());

    // Write to a .tmp file first to avoid leaving partial downloads
    let tmp = dest.with_extension("onnx.tmp");
    let mut file = std::fs::File::create(&tmp)
        .with_context(|| format!("Cannot create {}", tmp.display()))?;

    let mut buf = vec![0u8; 65_536];
    let mut downloaded = 0u64;
    loop {
        let n = response
            .read(&mut buf)
            .with_context(|| format!("Read error while downloading {}", label))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .with_context(|| format!("Write error for {}", dest.display()))?;
        downloaded += n as u64;
        pb.set_position(downloaded);
    }

    drop(file);
    std::fs::rename(&tmp, dest)
        .with_context(|| format!("Failed to finalize {}", dest.display()))?;

    pb.finish_with_message(format!("{} {}", "✓".green(), label));
    Ok(())
}
