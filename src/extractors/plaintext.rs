use anyhow::Result;

/// Pass-through: decode bytes as UTF-8 (lossy).
pub fn extract(bytes: &[u8]) -> Result<String> {
    Ok(String::from_utf8_lossy(bytes).to_string())
}
