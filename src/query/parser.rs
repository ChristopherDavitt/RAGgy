use chrono::{DateTime, Datelike, Utc, Duration, TimeZone};
use serde::Serialize;
use std::path::PathBuf;

use crate::entity::regex::extract_entities_from_text;

#[derive(Debug, Clone, Serialize)]
pub struct QueryPlan {
    pub keywords: Vec<String>,
    pub negations: Vec<String>,
    pub entities: Vec<String>,
    pub type_filter: Option<String>,
    pub scope_filter: Option<PathBuf>,
    pub date_filter: Option<DateRange>,
    pub confidence: f32,
    pub suggested_alpha: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DateRange {
    pub after: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
}

const QUERY_STOPWORDS: &[&str] = &[
    "the", "a", "an", "about", "that", "which", "where",
    "files", "documents", "find", "show", "get", "me", "all",
    "related", "to", "of", "for", "with",
];

pub fn parse_query(query: &str) -> QueryPlan {
    let mut remaining = query.to_string();
    let mut negations = Vec::new();
    let mut type_filter = None;
    let mut scope_filter = None;
    let mut date_filter = None;

    // 1. NEGATION DETECTION
    let negation_keywords = ["not ", "except ", "excluding ", "without ", "but not "];
    for kw in &negation_keywords {
        if let Some(idx) = remaining.to_lowercase().find(kw) {
            let after = &remaining[idx + kw.len()..];
            negations.extend(
                after.split_whitespace()
                    .map(|s| s.to_string())
            );
            remaining = remaining[..idx].to_string();
            break;
        }
    }

    // 2. SCOPE DETECTION
    let scope_patterns = ["in ~/", "in /", "under ~/", "under /", "in ./"];
    for pattern in &scope_patterns {
        if let Some(idx) = remaining.to_lowercase().find(pattern) {
            let path_start = idx + pattern.len() - if pattern.starts_with("in ~/") || pattern.starts_with("under ~/") { 2 } else { 1 };
            let path_str: String = remaining[path_start..].split_whitespace().next().unwrap_or("").to_string();
            if !path_str.is_empty() {
                scope_filter = Some(PathBuf::from(&remaining[path_start..path_start + path_str.len()]));
                let end = path_start + path_str.len();
                remaining = format!("{}{}", &remaining[..idx], &remaining[end..]);
            }
            break;
        }
    }

    // 3. TYPE DETECTION
    let type_keywords: &[(&[&str], &str)] = &[
        (&["blog post", "blog posts", "markdown", " md "], "markdown"),
        (&["code", "script", "function", "source"], "code"),
        (&["config", "configuration", " json ", " yaml ", " yml "], "yaml"),
        (&["notes", "text", "plain text"], "plaintext"),
    ];

    let lower = format!(" {} ", remaining.to_lowercase());
    for (keywords, ct) in type_keywords {
        for kw in *keywords {
            if lower.contains(kw) {
                type_filter = Some(ct.to_string());
                remaining = remaining.replace(kw.trim(), "").trim().to_string();
                break;
            }
        }
        if type_filter.is_some() {
            break;
        }
    }

    // 4. DATE DETECTION
    let now = Utc::now();
    let lower_rem = remaining.to_lowercase();
    if lower_rem.contains("modified today") || lower_rem.contains("from today") {
        let start_of_today = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
        date_filter = Some(DateRange {
            after: Some(Utc.from_utc_datetime(&start_of_today)),
            before: None,
        });
        remaining = remaining.replace("modified today", "").replace("from today", "").trim().to_string();
    } else if lower_rem.contains("this week") || lower_rem.contains("modified this week") {
        let days_since_monday = now.date_naive().weekday().num_days_from_monday();
        let start_of_week = now - Duration::days(days_since_monday as i64);
        date_filter = Some(DateRange {
            after: Some(start_of_week),
            before: None,
        });
        remaining = remaining.replace("modified this week", "").replace("this week", "").trim().to_string();
    } else if lower_rem.contains("last month") || lower_rem.contains("modified last month") {
        let last_month = now - Duration::days(30);
        date_filter = Some(DateRange {
            after: Some(last_month),
            before: Some(now),
        });
        remaining = remaining.replace("modified last month", "").replace("last month", "").trim().to_string();
    } else if lower_rem.contains("recent") || lower_rem.contains("recently") {
        let week_ago = now - Duration::days(7);
        date_filter = Some(DateRange {
            after: Some(week_ago),
            before: None,
        });
        remaining = remaining.replace("recently", "").replace("recent", "").trim().to_string();
    }

    // 5. ENTITY EXTRACTION
    let entity_matches = extract_entities_from_text(&remaining);
    let entities: Vec<String> = entity_matches.iter()
        .map(|e| e.value.clone())
        .collect();

    // 6. KEYWORD EXTRACTION
    let keywords: Vec<String> = remaining.split_whitespace()
        .map(|s| s.to_lowercase())
        .filter(|s| !QUERY_STOPWORDS.contains(&s.as_str()))
        .filter(|s| s.len() > 1)
        .collect();

    // 7. CONFIDENCE SCORING
    let mut confidence: f32 = 0.5;
    if keywords.len() >= 2 { confidence += 0.2; }
    if type_filter.is_some() { confidence += 0.1; }
    if !entities.is_empty() { confidence += 0.1; }
    if scope_filter.is_some() { confidence += 0.1; }
    if confidence > 1.0 { confidence = 1.0; }

    let suggested_alpha = compute_dynamic_alpha(query, &keywords);

    QueryPlan {
        keywords,
        negations,
        entities,
        type_filter,
        scope_filter,
        date_filter,
        confidence,
        suggested_alpha: Some(suggested_alpha),
    }
}

/// Compute an ideal alpha based on query characteristics.
/// Lower alpha = favor keyword/BM25, higher = favor semantic/vector.
fn compute_dynamic_alpha(query: &str, keywords: &[String]) -> f32 {
    let words: Vec<&str> = query.split_whitespace().collect();
    let mut alpha: f32 = 0.5; // neutral starting point

    // Question words → push semantic
    let question_words = ["what", "how", "why", "when", "where", "who", "which", "does", "can", "should"];
    if let Some(first) = words.first() {
        if question_words.iter().any(|q| first.eq_ignore_ascii_case(q)) {
            alpha += 0.15;
        }
    }

    // Short query (1-2 keywords) → push keyword
    if keywords.len() <= 2 {
        alpha -= 0.15;
    }

    // Long natural language (5+ words) → push semantic
    if words.len() >= 5 {
        alpha += 0.1;
    }

    // Quoted exact phrase → push keyword heavily
    if query.contains('"') {
        alpha -= 0.25;
    }

    // Technical identifiers (snake_case, CamelCase, namespaces) → push keyword
    if words.iter().any(|w| {
        w.contains('_') || w.contains("::") || w.contains('.') || has_mixed_case(w)
    }) {
        alpha -= 0.2;
    }

    // High stop-word ratio → more natural language → push semantic
    let stop_ratio = if !words.is_empty() {
        let stop_count = words.iter()
            .filter(|w| QUERY_STOPWORDS.contains(&w.to_lowercase().as_str()))
            .count();
        stop_count as f32 / words.len() as f32
    } else {
        0.0
    };
    if stop_ratio > 0.4 {
        alpha += 0.1;
    }

    alpha.clamp(0.05, 0.95)
}

/// Check if a word has mixed case (e.g., CamelCase, getElementById)
fn has_mixed_case(word: &str) -> bool {
    let has_upper = word.chars().any(|c| c.is_uppercase());
    let has_lower = word.chars().any(|c| c.is_lowercase());
    // Must have both, and not just a capitalized first letter
    has_upper && has_lower && !word[1..].chars().all(|c| c.is_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_query() {
        let plan = parse_query("deployment strategies");
        assert!(plan.keywords.contains(&"deployment".to_string()));
        assert!(plan.keywords.contains(&"strategies".to_string()));
        assert!(plan.negations.is_empty());
    }

    #[test]
    fn test_negation() {
        let plan = parse_query("deployment strategies not CI/CD");
        assert!(plan.keywords.contains(&"deployment".to_string()));
        assert!(plan.negations.contains(&"CI/CD".to_string()));
    }

    #[test]
    fn test_type_filter() {
        let plan = parse_query("blog post about deployment");
        assert_eq!(plan.type_filter.as_deref(), Some("markdown"));
        assert!(plan.keywords.contains(&"deployment".to_string()));
    }
}
