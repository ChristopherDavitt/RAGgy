use anyhow::Result;

use crate::graph::store::GraphStore;
use crate::index::vector::VectorIndex;

/// Result of a resolution pass.
pub struct ResolutionResult {
    pub merges: u64,
    pub details: Vec<(String, String)>, // (alias, canonical)
}

/// Title prefixes to strip when comparing person names.
const TITLE_PREFIXES: &[&str] = &[
    "president ", "vice president ",
    "attorney general ", "general ",
    "senator ", "representative ",
    "secretary ", "minister ", "prime minister ",
    "prince ", "princess ", "king ", "queen ",
    "lord ", "lady ", "sir ", "dame ",
    "dr. ", "dr ", "prof. ", "prof ",
    "mr. ", "mr ", "mrs. ", "mrs ",
    "ms. ", "ms ", "miss ",
    "ceo ", "cfo ", "coo ", "cto ",
    "judge ", "justice ", "chief justice ",
    "director ", "commissioner ",
    "captain ", "colonel ", "major ", "lieutenant ",
    "sergeant ", "officer ",
    "agent ",
];

/// Run entity resolution on the NER entity table.
/// Identifies merge candidates and sets canonical_id.
pub fn resolve_entities(store: &GraphStore) -> Result<ResolutionResult> {
    let all_entities = get_all_entities(store)?;
    let mut result = ResolutionResult { merges: 0, details: vec![] };

    // Group by type — only merge within same type
    let mut by_type: std::collections::HashMap<String, Vec<(i64, String)>> = std::collections::HashMap::new();
    for (id, name, etype, canonical_id) in &all_entities {
        // Skip already-resolved entities
        if canonical_id.is_some() {
            continue;
        }
        by_type.entry(etype.clone()).or_default().push((*id, name.clone()));
    }

    for (etype, entities) in &by_type {
        // Sort by name length descending — longer names are more specific (canonical)
        let mut sorted = entities.clone();
        sorted.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

        for i in 0..sorted.len() {
            let (long_id, long_name) = &sorted[i];

            // Check if this entity is already an alias
            if is_already_merged(store, *long_id)? {
                continue;
            }

            for j in (i + 1)..sorted.len() {
                let (short_id, short_name) = &sorted[j];

                if is_already_merged(store, *short_id)? {
                    continue;
                }

                if should_merge(long_name, short_name, etype) {
                    // short_name is an alias of long_name
                    merge_entities(store, *short_id, *long_id)?;
                    result.merges += 1;
                    result.details.push((short_name.clone(), long_name.clone()));
                }
            }
        }
    }

    Ok(result)
}

/// Determine if two entities of the same type should merge.
/// `longer` is the more specific name, `shorter` is the potential alias.
fn should_merge(longer: &str, shorter: &str, entity_type: &str) -> bool {
    let long_lower = longer.to_lowercase();
    let short_lower = shorter.to_lowercase();

    // Exact match (case-insensitive)
    if long_lower == short_lower {
        return true;
    }

    match entity_type {
        "person" => should_merge_person(longer, shorter),
        "organization" => should_merge_org(longer, shorter),
        "location" => should_merge_location(longer, shorter),
        _ => should_merge_generic(longer, shorter),
    }
}

fn should_merge_person(longer: &str, shorter: &str) -> bool {
    let long_normalized = strip_title(longer);
    let short_normalized = strip_title(shorter);

    let long_lower = long_normalized.to_lowercase();
    let short_lower = short_normalized.to_lowercase();

    // After stripping titles, check exact match
    if long_lower == short_lower {
        return true;
    }

    // Last name match: "Epstein" matches "Jeffrey Epstein"
    let long_parts = split_name(&long_lower);
    let short_parts = split_name(&short_lower);

    // Short name is just a last name that matches the long name's last name
    if short_parts.len() == 1 && long_parts.len() > 1 {
        if let Some(last) = long_parts.last() {
            if *last == short_parts[0] {
                return true;
            }
        }
    }

    // Initial match: "J. Epstein" matches "Jeffrey Epstein"
    if short_parts.len() >= 2 && long_parts.len() >= 2 {
        // Check if last names match
        if short_parts.last() == long_parts.last() {
            // Check if first parts are initials of the long name
            let short_first = &short_parts[0];
            let long_first = &long_parts[0];
            // "j." or "j" matches "jeffrey"
            let initial = short_first.trim_end_matches('.');
            if initial.len() == 1 && long_first.starts_with(initial) {
                return true;
            }
        }
    }

    // Substring: "John Smith" contains "John Smith" (with title stripped)
    // Require at least 2 words to avoid "Andrew" matching "Andrew Brettler"
    if short_parts.len() >= 2 && long_lower.contains(&short_lower) && short_lower.len() >= 4 {
        return true;
    }

    false
}

fn should_merge_org(longer: &str, shorter: &str) -> bool {
    let long_lower = longer.to_lowercase();
    let short_lower = shorter.to_lowercase();

    // Substring match with minimum length
    if long_lower.contains(&short_lower) && short_lower.len() >= 4 {
        return true;
    }
    if short_lower.contains(&long_lower) && long_lower.len() >= 4 {
        return true;
    }

    // Strip common suffixes: Inc, Corp, LLC, Ltd, etc.
    let long_stripped = strip_org_suffix(&long_lower);
    let short_stripped = strip_org_suffix(&short_lower);
    if long_stripped == short_stripped && !long_stripped.is_empty() {
        return true;
    }

    false
}

fn should_merge_location(longer: &str, shorter: &str) -> bool {
    let long_lower = longer.to_lowercase();
    let short_lower = shorter.to_lowercase();

    // Substring match: "Virgin Islands" is in "US Virgin Islands"
    if long_lower.contains(&short_lower) && short_lower.len() >= 4 {
        return true;
    }

    false
}

fn should_merge_generic(longer: &str, shorter: &str) -> bool {
    let long_lower = longer.to_lowercase();
    let short_lower = shorter.to_lowercase();

    // Only merge on exact case-insensitive match for unknown types
    long_lower == short_lower
}

/// Strip title prefixes from a person's name.
fn strip_title(name: &str) -> String {
    let lower = name.to_lowercase();
    for prefix in TITLE_PREFIXES {
        if lower.starts_with(prefix) {
            return name[prefix.len()..].to_string();
        }
    }
    name.to_string()
}

/// Split a name into parts, handling "." in initials.
fn split_name(name: &str) -> Vec<String> {
    name.split_whitespace()
        .map(|s| s.to_string())
        .collect()
}

/// Strip common organization suffixes.
fn strip_org_suffix(name: &str) -> String {
    let suffixes = [
        ", inc.", ", inc", " inc.", " inc",
        ", corp.", ", corp", " corp.", " corp",
        ", llc", " llc",
        ", ltd.", ", ltd", " ltd.", " ltd",
        ", co.", ", co", " co.", " co",
        " corporation", " incorporated",
        " company", " group", " holdings",
    ];
    let mut result = name.to_string();
    for suffix in &suffixes {
        if result.ends_with(suffix) {
            result = result[..result.len() - suffix.len()].to_string();
            break;
        }
    }
    result
}

// ── Store operations ──

fn get_all_entities(store: &GraphStore) -> Result<Vec<(i64, String, String, Option<i64>)>> {
    let conn = store.conn();
    let mut stmt = conn.prepare(
        "SELECT id, name, entity_type, canonical_id FROM ner_entities ORDER BY entity_type, name"
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn is_already_merged(store: &GraphStore, entity_id: i64) -> Result<bool> {
    let conn = store.conn();
    let canonical: Option<i64> = conn.query_row(
        "SELECT canonical_id FROM ner_entities WHERE id = ?1",
        rusqlite::params![entity_id],
        |row| row.get(0),
    )?;
    Ok(canonical.is_some())
}

/// Minimum entity name length for gradient resolution.  Very short names
/// (e.g. "US", "UK") produce noisy embeddings that match too broadly.
const MIN_GRADIENT_NAME_LEN: usize = 3;

/// Run gradient-based entity resolution using embedding cosine similarity.
///
/// For each type group, embeds all entity names and merges any pair whose cosine
/// similarity exceeds `threshold`. This complements rule-based resolution by
/// catching semantic aliases that string heuristics miss (e.g. "FBI" and
/// "Federal Bureau of Investigation").
///
/// Should be called *after* `resolve_entities` so that already-merged entities
/// are skipped and not re-processed.
pub fn gradient_resolve_entities(
    store: &GraphStore,
    vector: &mut VectorIndex,
    threshold: f32,
) -> Result<ResolutionResult> {
    let all_entities = get_all_entities(store)?;
    let mut result = ResolutionResult { merges: 0, details: vec![] };

    // Group unresolved entities by type, filtering out very short names
    // that produce unreliable embeddings.
    let mut by_type: std::collections::HashMap<String, Vec<(i64, String)>> =
        std::collections::HashMap::new();
    for (id, name, etype, canonical_id) in &all_entities {
        if canonical_id.is_some() {
            continue; // already merged by rule-based pass
        }
        if name.len() < MIN_GRADIENT_NAME_LEN {
            continue; // too short for reliable embedding similarity
        }
        by_type.entry(etype.clone()).or_default().push((*id, name.clone()));
    }

    // Track merges locally to avoid per-pair DB round-trips.
    let mut merged: std::collections::HashSet<i64> = std::collections::HashSet::new();

    for (_etype, entities) in &by_type {
        if entities.len() < 2 {
            continue;
        }

        // Embed all entity names in one batch
        let names: Vec<&str> = entities.iter().map(|(_, n)| n.as_str()).collect();
        let embeddings = match vector.embed_batch(&names) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("  warning: gradient resolution skipped type group: {}", e);
                continue;
            }
        };

        // Sort candidates by name length descending so longer (more specific) names
        // become canonical when merging.
        let mut indexed: Vec<(usize, i64, &str)> = entities
            .iter()
            .enumerate()
            .map(|(i, (id, name))| (i, *id, name.as_str()))
            .collect();
        indexed.sort_by(|a, b| b.2.len().cmp(&a.2.len()));

        // Pairwise cosine similarity — vectors are already L2-normalised, so
        // dot product == cosine similarity.
        let n = indexed.len();
        for i in 0..n {
            let (ei, long_id, _long_name) = indexed[i];

            if merged.contains(&long_id) {
                continue;
            }

            for j in (i + 1)..n {
                let (ej, short_id, _short_name) = indexed[j];

                if merged.contains(&short_id) {
                    continue;
                }

                let sim: f32 = embeddings[ei]
                    .iter()
                    .zip(embeddings[ej].iter())
                    .map(|(a, b)| a * b)
                    .sum();

                if sim >= threshold {
                    merge_entities(store, short_id, long_id)?;
                    merged.insert(short_id);
                    result.merges += 1;
                    result.details.push((
                        entities[ej].1.clone(),
                        entities[ei].1.clone(),
                    ));
                }
            }
        }
    }

    Ok(result)
}

fn merge_entities(store: &GraphStore, alias_id: i64, canonical_id: i64) -> Result<()> {
    let conn = store.conn();

    // Set canonical_id on the alias
    conn.execute(
        "UPDATE ner_entities SET canonical_id = ?1 WHERE id = ?2",
        rusqlite::params![canonical_id, alias_id],
    )?;

    // Also update any entities that pointed to alias_id as their canonical
    conn.execute(
        "UPDATE ner_entities SET canonical_id = ?1 WHERE canonical_id = ?2",
        rusqlite::params![canonical_id, alias_id],
    )?;

    // Merge co-occurrence edges: rewire alias edges to canonical
    // Get all edges involving the alias
    let edges: Vec<(i64, i64, f64, i64)> = {
        let mut stmt = conn.prepare(
            "SELECT entity_a, entity_b, weight, doc_count FROM ner_edges
             WHERE entity_a = ?1 OR entity_b = ?1"
        )?;
        let rows = stmt.query_map(rusqlite::params![alias_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        rows.filter_map(|r| r.ok()).collect()
    };

    for (a, b, weight, doc_count) in edges {
        // Determine the "other" entity
        let other = if a == alias_id { b } else { a };
        if other == canonical_id {
            // Edge between alias and canonical — just delete, they're the same entity now
            continue;
        }

        // Canonical ordering
        let (new_a, new_b) = if canonical_id < other {
            (canonical_id, other)
        } else {
            (other, canonical_id)
        };

        // Check if canonical already has an edge to this other entity
        let existing: Option<(f64, i64)> = conn.query_row(
            "SELECT weight, doc_count FROM ner_edges WHERE entity_a = ?1 AND entity_b = ?2",
            rusqlite::params![new_a, new_b],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).ok();

        if let Some((existing_weight, existing_doc_count)) = existing {
            // Merge: add weights and doc counts
            conn.execute(
                "UPDATE ner_edges SET weight = ?1, doc_count = ?2
                 WHERE entity_a = ?3 AND entity_b = ?4",
                rusqlite::params![
                    existing_weight + weight,
                    existing_doc_count + doc_count,
                    new_a, new_b
                ],
            )?;
        } else {
            // Create new edge from canonical to other
            conn.execute(
                "INSERT OR IGNORE INTO ner_edges (entity_a, entity_b, weight, doc_count)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![new_a, new_b, weight, doc_count],
            )?;
        }
    }

    // Delete old edges involving the alias
    conn.execute(
        "DELETE FROM ner_edges WHERE entity_a = ?1 OR entity_b = ?1",
        rusqlite::params![alias_id],
    )?;

    Ok(())
}
