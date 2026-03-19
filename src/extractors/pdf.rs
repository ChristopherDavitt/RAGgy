use anyhow::{Result, Context};

/// Extract text from a PDF file.
/// Uses pdf-extract for the text layer. Falls back to OCR if tesseract
/// is available on PATH and the text layer is empty/minimal.
pub fn extract(bytes: &[u8]) -> Result<String> {
    // Catch panics from pdf-extract on malformed PDFs
    let text = std::panic::catch_unwind(|| {
        pdf_extract::extract_text_from_mem(bytes)
    })
    .map_err(|_| anyhow::anyhow!("PDF parser panicked on malformed input"))?
    .context("Failed to extract text from PDF")?;

    let trimmed = text.trim();

    // If we got meaningful text, return it
    if !trimmed.is_empty() && trimmed.len() > 50 {
        return Ok(clean_pdf_text(&text));
    }

    // Text layer is empty/too short — try OCR if tesseract is on PATH
    if let Some(ocr_text) = try_ocr(bytes) {
        if !ocr_text.trim().is_empty() {
            return Ok(ocr_text);
        }
    }

    // Return whatever we got (may be empty/short)
    Ok(clean_pdf_text(&text))
}

/// Clean up common PDF text extraction artifacts.
fn clean_pdf_text(text: &str) -> String {
    text.replace('\0', "")
        .replace('\x0C', "\n\n") // form feed → paragraph break
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Attempt OCR via tesseract CLI.
/// Returns None if tesseract is not installed or OCR fails.
fn try_ocr(bytes: &[u8]) -> Option<String> {
    use std::process::Command;
    use std::io::Write;

    // Check if tesseract is available
    if Command::new("tesseract")
        .arg("--version")
        .output()
        .is_err()
    {
        return None;
    }

    // Write PDF to a temp file for tesseract
    let temp_dir = std::env::temp_dir();
    let pdf_path = temp_dir.join(format!("raggy_ocr_{}.pdf", std::process::id()));
    let output_base = temp_dir.join(format!("raggy_ocr_{}_out", std::process::id()));
    let output_path = temp_dir.join(format!("raggy_ocr_{}_out.txt", std::process::id()));

    // Write PDF bytes to temp file
    let mut file = std::fs::File::create(&pdf_path).ok()?;
    file.write_all(bytes).ok()?;
    drop(file);

    // Run tesseract (it can handle PDFs directly with the pdf input handler)
    let result = Command::new("tesseract")
        .arg(&pdf_path)
        .arg(&output_base)
        .arg("-l").arg("eng")
        .output();

    // Clean up input
    let _ = std::fs::remove_file(&pdf_path);

    match result {
        Ok(output) if output.status.success() => {
            let text = std::fs::read_to_string(&output_path).ok();
            let _ = std::fs::remove_file(&output_path);
            text
        }
        _ => {
            let _ = std::fs::remove_file(&output_path);
            None
        }
    }
}
