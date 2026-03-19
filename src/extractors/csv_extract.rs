use anyhow::Result;
use std::io::Cursor;

/// Convert CSV/TSV rows into readable text for indexing.
/// Format: "Column1: value1 | Column2: value2" per row.
pub fn extract(bytes: &[u8], extension: &str) -> Result<String> {
    let cursor = Cursor::new(bytes);

    let delimiter = match extension {
        "tsv" => b'\t',
        _ => b',',
    };

    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .has_headers(true)
        .from_reader(cursor);

    let headers: Vec<String> = reader
        .headers()
        .map(|h| h.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();

    let mut output = String::new();

    if !headers.is_empty() {
        output.push_str("Columns: ");
        output.push_str(&headers.join(", "));
        output.push_str("\n\n");
    }

    for record in reader.records() {
        let record = match record {
            Ok(r) => r,
            Err(_) => continue,
        };

        let row_text: Vec<String> = record
            .iter()
            .enumerate()
            .map(|(i, val)| {
                if i < headers.len() && !headers[i].is_empty() {
                    format!("{}: {}", headers[i], val)
                } else {
                    val.to_string()
                }
            })
            .collect();

        output.push_str(&row_text.join(" | "));
        output.push('\n');
    }

    Ok(output)
}
