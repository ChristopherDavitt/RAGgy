use anyhow::Result;

/// Markdown is already text — pass through as UTF-8.
/// The ingest::markdown extractor handles structural parsing downstream.
pub fn extract(bytes: &[u8]) -> Result<String> {
    Ok(String::from_utf8_lossy(bytes).to_string())
}
