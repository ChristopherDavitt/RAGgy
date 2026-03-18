// Metadata operations are handled through GraphStore (graph/store.rs)
// which manages the SQLite database including the documents table.
// This module is kept as a namespace for any future metadata-specific operations.

use anyhow::Result;
use crate::graph::store::GraphStore;

pub fn get_content_type_counts(store: &GraphStore) -> Result<Vec<(String, u64)>> {
    let mut stmt = store.conn().prepare(
        "SELECT content_type, COUNT(*) FROM documents GROUP BY content_type ORDER BY COUNT(*) DESC"
    )?;
    let rows = stmt.query_map([], |row| {
        let ct: String = row.get(0)?;
        let count: u64 = row.get(1)?;
        Ok((ct, count))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}
