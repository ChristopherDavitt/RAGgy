pub mod plaintext;
pub mod markdown;
pub mod pdf;
pub mod html;
pub mod csv_extract;

use anyhow::Result;

/// Extract text content from raw file bytes based on file extension.
/// For text-based formats, this is a UTF-8 decode.
/// For binary formats (PDF), this performs actual content extraction.
pub fn extract(bytes: &[u8], extension: &str) -> Result<String> {
    match extension {
        "pdf" => pdf::extract(bytes),
        "html" | "htm" => html::extract(bytes),
        "csv" | "tsv" => csv_extract::extract(bytes, extension),
        // All text-based formats pass through as UTF-8
        _ => plaintext::extract(bytes),
    }
}
