use anyhow::Result;

/// Extract readable text from HTML content.
/// Strips tags, scripts, and styles while preserving text structure.
pub fn extract(bytes: &[u8]) -> Result<String> {
    let html_str = String::from_utf8_lossy(bytes);
    Ok(strip_tags(&html_str))
}

/// Simple HTML tag stripper that preserves text content.
/// Removes script/style blocks entirely, converts block elements to newlines.
fn strip_tags(html: &str) -> String {
    let mut output = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_skip_block = false;
    let mut tag_name = String::new();
    let mut collecting_tag_name = false;

    let skip_tags = ["script", "style", "noscript"];
    let block_tags = [
        "p", "div", "br", "h1", "h2", "h3", "h4", "h5", "h6",
        "li", "tr", "blockquote", "pre", "section", "article",
        "header", "footer", "nav", "table", "ul", "ol",
    ];

    let chars: Vec<char> = html.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];

        if ch == '<' {
            in_tag = true;
            tag_name.clear();
            collecting_tag_name = true;
            i += 1;
            continue;
        }

        if in_tag {
            if ch == '>' {
                in_tag = false;
                collecting_tag_name = false;

                let tag_lower = tag_name.to_lowercase();
                let is_closing = tag_lower.starts_with('/');
                let clean_tag = tag_lower.trim_start_matches('/').to_string();
                // Strip attributes from tag name
                let clean_tag = clean_tag.split_whitespace().next().unwrap_or("");

                // Handle skip blocks (script, style)
                if skip_tags.contains(&clean_tag) {
                    in_skip_block = !is_closing;
                }

                // Add newlines for block elements
                if block_tags.contains(&clean_tag) {
                    output.push('\n');
                }
            } else if collecting_tag_name {
                if ch == ' ' || ch == '\t' || ch == '\n' {
                    // Keep collecting to capture attributes but stop tag name
                    collecting_tag_name = false;
                }
                tag_name.push(ch);
            }
            i += 1;
            continue;
        }

        if in_skip_block {
            i += 1;
            continue;
        }

        // Decode common HTML entities
        if ch == '&' {
            let remaining: String = chars[i..].iter().take(10).collect();
            if remaining.starts_with("&amp;") {
                output.push('&');
                i += 5;
            } else if remaining.starts_with("&lt;") {
                output.push('<');
                i += 4;
            } else if remaining.starts_with("&gt;") {
                output.push('>');
                i += 4;
            } else if remaining.starts_with("&quot;") {
                output.push('"');
                i += 6;
            } else if remaining.starts_with("&apos;") {
                output.push('\'');
                i += 6;
            } else if remaining.starts_with("&nbsp;") {
                output.push(' ');
                i += 6;
            } else {
                output.push(ch);
                i += 1;
            }
            continue;
        }

        output.push(ch);
        i += 1;
    }

    // Collapse excessive whitespace
    collapse_whitespace(&output)
}

fn collapse_whitespace(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut blank_count = 0;

    for line in s.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_count += 1;
            if blank_count <= 1 {
                result.push('\n');
            }
        } else {
            if blank_count > 0 && !result.is_empty() {
                result.push('\n');
            }
            blank_count = 0;
            result.push_str(trimmed);
            result.push('\n');
        }
    }

    result.trim().to_string()
}
