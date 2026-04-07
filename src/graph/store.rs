use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::Path;

use super::{Node, NodeKind, NodeProperties, Edge, EdgeKind, EdgeProperties};

pub struct GraphStore {
    conn: Connection,
}

impl GraphStore {
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        let store = GraphStore { conn };
        store.create_tables()?;
        Ok(store)
    }

    fn create_tables(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS documents (
                node_id INTEGER PRIMARY KEY,
                path TEXT UNIQUE NOT NULL,
                content_type TEXT NOT NULL,
                title TEXT,
                modified TEXT NOT NULL,
                size INTEGER NOT NULL,
                content_hash TEXT NOT NULL,
                indexed_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS nodes (
                id INTEGER PRIMARY KEY,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                properties TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS edges (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                from_node INTEGER NOT NULL REFERENCES nodes(id),
                to_node INTEGER NOT NULL REFERENCES nodes(id),
                kind TEXT NOT NULL,
                weight REAL NOT NULL DEFAULT 1.0,
                properties TEXT NOT NULL DEFAULT '{}',
                UNIQUE(from_node, to_node, kind)
            );

            CREATE INDEX IF NOT EXISTS idx_edges_from ON edges(from_node);
            CREATE INDEX IF NOT EXISTS idx_edges_to ON edges(to_node);
            CREATE INDEX IF NOT EXISTS idx_edges_kind ON edges(kind);
            CREATE INDEX IF NOT EXISTS idx_documents_path ON documents(path);
            CREATE INDEX IF NOT EXISTS idx_documents_content_type ON documents(content_type);
            CREATE INDEX IF NOT EXISTS idx_nodes_kind ON nodes(kind);

            CREATE TABLE IF NOT EXISTS ner_entities (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                entity_type TEXT NOT NULL,
                canonical_id INTEGER,
                UNIQUE(name, entity_type)
            );

            CREATE TABLE IF NOT EXISTS ner_mentions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entity_id INTEGER NOT NULL REFERENCES ner_entities(id),
                chunk_id INTEGER NOT NULL,
                doc_path TEXT NOT NULL,
                confidence REAL,
                span_start INTEGER,
                span_end INTEGER
            );

            CREATE TABLE IF NOT EXISTS ner_edges (
                entity_a INTEGER NOT NULL REFERENCES ner_entities(id),
                entity_b INTEGER NOT NULL REFERENCES ner_entities(id),
                weight REAL NOT NULL DEFAULT 1.0,
                doc_count INTEGER NOT NULL DEFAULT 1,
                UNIQUE(entity_a, entity_b)
            );

            CREATE INDEX IF NOT EXISTS idx_ner_entities_name ON ner_entities(name);
            CREATE INDEX IF NOT EXISTS idx_ner_entities_type ON ner_entities(entity_type);
            CREATE INDEX IF NOT EXISTS idx_ner_mentions_entity ON ner_mentions(entity_id);
            CREATE INDEX IF NOT EXISTS idx_ner_mentions_chunk ON ner_mentions(chunk_id);
            CREATE INDEX IF NOT EXISTS idx_ner_mentions_doc ON ner_mentions(doc_path);
            CREATE INDEX IF NOT EXISTS idx_ner_edges_a ON ner_edges(entity_a);
            CREATE INDEX IF NOT EXISTS idx_ner_edges_b ON ner_edges(entity_b);

            CREATE TABLE IF NOT EXISTS document_tags (
                doc_path TEXT NOT NULL,
                tag TEXT NOT NULL,
                PRIMARY KEY (doc_path, tag)
            );
            CREATE INDEX IF NOT EXISTS idx_document_tags_tag ON document_tags(tag);

            CREATE TABLE IF NOT EXISTS document_metadata (
                doc_path TEXT NOT NULL,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                PRIMARY KEY (doc_path, key)
            );
            CREATE INDEX IF NOT EXISTS idx_document_metadata_key ON document_metadata(key);
            "
        )?;
        Ok(())
    }

    pub fn next_id(&self) -> Result<u64> {
        let max_id: Option<u64> = self.conn.query_row(
            "SELECT MAX(id) FROM nodes",
            [],
            |row| row.get(0),
        )?;
        Ok(max_id.unwrap_or(0) + 1)
    }

    pub fn insert_node(&self, node: &Node) -> Result<()> {
        let props_json = serde_json::to_string(&node.properties)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO nodes (id, kind, name, properties) VALUES (?1, ?2, ?3, ?4)",
            params![node.id, node.kind.as_str(), node.name, props_json],
        )?;
        Ok(())
    }

    pub fn insert_edge(&self, edge: &Edge) -> Result<()> {
        let props_json = serde_json::to_string(&edge.properties)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO edges (from_node, to_node, kind, weight, properties) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![edge.from, edge.to, edge.kind.as_str(), edge.weight, props_json],
        )?;
        Ok(())
    }

    pub fn insert_document(
        &self,
        node_id: u64,
        path: &str,
        content_type: &str,
        title: Option<&str>,
        modified: &str,
        size: u64,
        content_hash: &str,
        indexed_at: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO documents (node_id, path, content_type, title, modified, size, content_hash, indexed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![node_id, path, content_type, title, modified, size, content_hash, indexed_at],
        )?;
        Ok(())
    }

    pub fn get_document_hash(&self, path: &str) -> Result<Option<String>> {
        let result = self.conn.query_row(
            "SELECT content_hash FROM documents WHERE path = ?1",
            params![path],
            |row| row.get(0),
        );
        match result {
            Ok(hash) => Ok(Some(hash)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn delete_by_path(&self, path: &str) -> Result<()> {
        // Get the document node_id
        let node_id: Option<u64> = match self.conn.query_row(
            "SELECT node_id FROM documents WHERE path = ?1",
            params![path],
            |row| row.get(0),
        ) {
            Ok(id) => Some(id),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e.into()),
        };

        if let Some(doc_id) = node_id {
            // Find all child nodes (chunks, sections) via Contains edges
            let child_ids: Vec<u64> = {
                let mut stmt = self.conn.prepare(
                    "SELECT to_node FROM edges WHERE from_node = ?1 AND kind = 'contains'"
                )?;
                let rows = stmt.query_map(params![doc_id], |row| row.get(0))?;
                rows.filter_map(|r| r.ok()).collect()
            };

            // Delete edges involving child nodes
            for child_id in &child_ids {
                self.conn.execute("DELETE FROM edges WHERE from_node = ?1 OR to_node = ?1", params![child_id])?;
                self.conn.execute("DELETE FROM nodes WHERE id = ?1", params![child_id])?;
            }

            // Delete edges involving the document node
            self.conn.execute("DELETE FROM edges WHERE from_node = ?1 OR to_node = ?1", params![doc_id])?;
            self.conn.execute("DELETE FROM nodes WHERE id = ?1", params![doc_id])?;
            self.conn.execute("DELETE FROM documents WHERE node_id = ?1", params![doc_id])?;
        }

        Ok(())
    }

    pub fn get_all_document_paths(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT path FROM documents")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn get_document_count(&self) -> Result<u64> {
        let count: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM documents",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    pub fn get_node_count(&self) -> Result<u64> {
        let count: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM nodes",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    pub fn get_edge_count(&self) -> Result<u64> {
        let count: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM edges",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    pub fn get_entity_count(&self) -> Result<u64> {
        let count: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE kind = 'entity'",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    pub fn get_last_indexed_time(&self) -> Result<Option<String>> {
        let result = self.conn.query_row(
            "SELECT MAX(indexed_at) FROM documents",
            [],
            |row| row.get(0),
        );
        match result {
            Ok(time) => Ok(time),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn get_nodes_by_kind(&self, kind: &str) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, name, properties FROM nodes WHERE kind = ?1"
        )?;
        let rows = stmt.query_map(params![kind], |row| {
            let id: u64 = row.get(0)?;
            let kind_str: String = row.get(1)?;
            let name: String = row.get(2)?;
            let props_json: String = row.get(3)?;
            Ok((id, kind_str, name, props_json))
        })?;

        let mut nodes = Vec::new();
        for row in rows {
            let (id, kind_str, name, props_json) = row?;
            let kind = NodeKind::from_str(&kind_str).unwrap_or(NodeKind::Chunk);
            let properties: NodeProperties = serde_json::from_str(&props_json).unwrap_or_default();
            nodes.push(Node { id, kind, name, properties });
        }
        Ok(nodes)
    }

    pub fn get_edges_from(&self, node_id: u64, kind: &str) -> Result<Vec<Edge>> {
        let mut stmt = self.conn.prepare(
            "SELECT from_node, to_node, kind, weight, properties FROM edges WHERE from_node = ?1 AND kind = ?2"
        )?;
        let rows = stmt.query_map(params![node_id, kind], |row| {
            let from: u64 = row.get(0)?;
            let to: u64 = row.get(1)?;
            let kind_str: String = row.get(2)?;
            let weight: f32 = row.get(3)?;
            let props_json: String = row.get(4)?;
            Ok((from, to, kind_str, weight, props_json))
        })?;

        let mut edges = Vec::new();
        for row in rows {
            let (from, to, kind_str, weight, props_json) = row?;
            let kind = EdgeKind::from_str(&kind_str).unwrap_or(EdgeKind::Contains);
            let properties: EdgeProperties = serde_json::from_str(&props_json).unwrap_or_default();
            edges.push(Edge { from, to, kind, weight, properties });
        }
        Ok(edges)
    }

    pub fn get_edges_to(&self, node_id: u64, kind: &str) -> Result<Vec<Edge>> {
        let mut stmt = self.conn.prepare(
            "SELECT from_node, to_node, kind, weight, properties FROM edges WHERE to_node = ?1 AND kind = ?2"
        )?;
        let rows = stmt.query_map(params![node_id, kind], |row| {
            let from: u64 = row.get(0)?;
            let to: u64 = row.get(1)?;
            let kind_str: String = row.get(2)?;
            let weight: f32 = row.get(3)?;
            let props_json: String = row.get(4)?;
            Ok((from, to, kind_str, weight, props_json))
        })?;

        let mut edges = Vec::new();
        for row in rows {
            let (from, to, kind_str, weight, props_json) = row?;
            let kind = EdgeKind::from_str(&kind_str).unwrap_or(EdgeKind::Contains);
            let properties: EdgeProperties = serde_json::from_str(&props_json).unwrap_or_default();
            edges.push(Edge { from, to, kind, weight, properties });
        }
        Ok(edges)
    }

    pub fn get_node(&self, id: u64) -> Result<Option<Node>> {
        let result = self.conn.query_row(
            "SELECT id, kind, name, properties FROM nodes WHERE id = ?1",
            params![id],
            |row| {
                let id: u64 = row.get(0)?;
                let kind_str: String = row.get(1)?;
                let name: String = row.get(2)?;
                let props_json: String = row.get(3)?;
                Ok((id, kind_str, name, props_json))
            },
        );
        match result {
            Ok((id, kind_str, name, props_json)) => {
                let kind = NodeKind::from_str(&kind_str).unwrap_or(NodeKind::Chunk);
                let properties: NodeProperties = serde_json::from_str(&props_json).unwrap_or_default();
                Ok(Some(Node { id, kind, name, properties }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn find_entity_by_name(&self, name: &str) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, name, properties FROM nodes WHERE kind = 'entity' AND name = ?1"
        )?;
        let rows = stmt.query_map(params![name], |row| {
            let id: u64 = row.get(0)?;
            let kind_str: String = row.get(1)?;
            let name: String = row.get(2)?;
            let props_json: String = row.get(3)?;
            Ok((id, kind_str, name, props_json))
        })?;

        let mut nodes = Vec::new();
        for row in rows {
            let (id, kind_str, name, props_json) = row?;
            let kind = NodeKind::from_str(&kind_str).unwrap_or(NodeKind::Entity);
            let properties: NodeProperties = serde_json::from_str(&props_json).unwrap_or_default();
            nodes.push(Node { id, kind, name, properties });
        }
        Ok(nodes)
    }

    pub fn update_edge_weight(&self, from: u64, to: u64, kind: &str, weight: f32) -> Result<()> {
        self.conn.execute(
            "UPDATE edges SET weight = ?1 WHERE from_node = ?2 AND to_node = ?3 AND kind = ?4",
            params![weight, from, to, kind],
        )?;
        Ok(())
    }

    pub fn get_edge(&self, from: u64, to: u64, kind: &str) -> Result<Option<Edge>> {
        let result = self.conn.query_row(
            "SELECT from_node, to_node, kind, weight, properties FROM edges WHERE from_node = ?1 AND to_node = ?2 AND kind = ?3",
            params![from, to, kind],
            |row| {
                let from: u64 = row.get(0)?;
                let to: u64 = row.get(1)?;
                let kind_str: String = row.get(2)?;
                let weight: f32 = row.get(3)?;
                let props_json: String = row.get(4)?;
                Ok((from, to, kind_str, weight, props_json))
            },
        );
        match result {
            Ok((from, to, kind_str, weight, props_json)) => {
                let kind = EdgeKind::from_str(&kind_str).unwrap_or(EdgeKind::Contains);
                let properties: EdgeProperties = serde_json::from_str(&props_json).unwrap_or_default();
                Ok(Some(Edge { from, to, kind, weight, properties }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn get_document_by_path(&self, path: &str) -> Result<Option<u64>> {
        let result = self.conn.query_row(
            "SELECT node_id FROM documents WHERE path = ?1",
            params![path],
            |row| row.get(0),
        );
        match result {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Batch-fetch modified timestamps for a set of document paths.
    /// Returns a map of path → DateTime<Utc>.  Paths not found are absent.
    pub fn get_documents_modified(
        &self,
        paths: &[&str],
    ) -> Result<std::collections::HashMap<String, chrono::DateTime<chrono::Utc>>> {
        if paths.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let placeholders: Vec<String> = (1..=paths.len()).map(|i| format!("?{}", i)).collect();
        let sql = format!(
            "SELECT path, modified FROM documents WHERE path IN ({})",
            placeholders.join(", ")
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut map = std::collections::HashMap::new();
        let rows = stmt.query_map(rusqlite::params_from_iter(paths.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows.filter_map(|r| r.ok()) {
            let (path, modified_str) = row;
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&modified_str) {
                map.insert(path, dt.with_timezone(&chrono::Utc));
            }
        }
        Ok(map)
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    // ── NER Entity Methods ──

    /// Insert or get an NER entity. Returns the entity ID.
    pub fn upsert_ner_entity(&self, name: &str, entity_type: &str) -> Result<i64> {
        // Try insert, ignore if exists
        self.conn.execute(
            "INSERT OR IGNORE INTO ner_entities (name, entity_type) VALUES (?1, ?2)",
            params![name, entity_type],
        )?;
        // Get the ID
        let id: i64 = self.conn.query_row(
            "SELECT id FROM ner_entities WHERE name = ?1 AND entity_type = ?2",
            params![name, entity_type],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    /// Record an entity mention in a chunk.
    pub fn insert_ner_mention(
        &self,
        entity_id: i64,
        chunk_id: u64,
        doc_path: &str,
        confidence: f32,
        span_start: usize,
        span_end: usize,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO ner_mentions (entity_id, chunk_id, doc_path, confidence, span_start, span_end)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![entity_id, chunk_id as i64, doc_path, confidence, span_start as i64, span_end as i64],
        )?;
        Ok(())
    }

    /// Add or increment a co-occurrence edge between two NER entities.
    pub fn upsert_ner_edge(&self, entity_a: i64, entity_b: i64) -> Result<()> {
        // Ensure a < b for canonical ordering
        let (a, b) = if entity_a < entity_b { (entity_a, entity_b) } else { (entity_b, entity_a) };
        let existing: Option<f32> = self.conn.query_row(
            "SELECT weight FROM ner_edges WHERE entity_a = ?1 AND entity_b = ?2",
            params![a, b],
            |row| row.get(0),
        ).ok();

        if let Some(weight) = existing {
            self.conn.execute(
                "UPDATE ner_edges SET weight = ?1, doc_count = doc_count + 1 WHERE entity_a = ?2 AND entity_b = ?3",
                params![weight + 1.0, a, b],
            )?;
        } else {
            self.conn.execute(
                "INSERT INTO ner_edges (entity_a, entity_b, weight, doc_count) VALUES (?1, ?2, 1.0, 1)",
                params![a, b],
            )?;
        }
        Ok(())
    }

    /// Delete NER mentions for a document path (for reindexing).
    pub fn delete_ner_mentions_by_path(&self, doc_path: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM ner_mentions WHERE doc_path = ?1",
            params![doc_path],
        )?;
        Ok(())
    }

    /// Get NER entity count (canonical only, excludes aliases).
    pub fn get_ner_entity_count(&self) -> Result<u64> {
        let count: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM ner_entities WHERE canonical_id IS NULL",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Get NER mention count.
    pub fn get_ner_mention_count(&self) -> Result<u64> {
        let count: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM ner_mentions",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Fetch a context window around a matched chunk.
    ///
    /// Walks up to the chunk's immediate parent (section or document) via a
    /// `contains` edge, retrieves all sibling chunks ordered by node_id
    /// (insertion order == document order), and returns the concatenated text
    /// of the `window` chunks before and after the matched chunk.
    ///
    /// A `window` of 1 returns [prev, matched, next]; 0 returns just the
    /// matched chunk's own text.  The result is always clamped to the
    /// boundaries of the parent node — the window never crosses section or
    /// document edges.
    pub fn get_chunk_window(&self, chunk_id: u64, window: usize) -> Result<String> {
        // Find the immediate parent that contains this chunk
        let parent_id: Option<u64> = self.conn.query_row(
            "SELECT from_node FROM edges WHERE to_node = ?1 AND kind = 'contains' LIMIT 1",
            params![chunk_id],
            |row| row.get(0),
        ).ok();

        let parent_id = match parent_id {
            Some(id) => id,
            None => return Ok(String::new()),
        };

        // Fetch all chunks under the same parent, ordered by node_id.
        // json_extract pulls the text field out of the JSON properties blob.
        let mut stmt = self.conn.prepare(
            "SELECT n.id, COALESCE(json_extract(n.properties, '$.text'), '')
             FROM edges e
             JOIN nodes n ON n.id = e.to_node
             WHERE e.from_node = ?1
               AND e.kind = 'contains'
               AND n.kind = 'chunk'
             ORDER BY n.id ASC"
        )?;
        let siblings: Vec<(u64, String)> = stmt
            .query_map(params![parent_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        let pos = match siblings.iter().position(|(id, _)| *id == chunk_id) {
            Some(p) => p,
            None => return Ok(String::new()),
        };

        let start = pos.saturating_sub(window);
        let end = (pos + window + 1).min(siblings.len());

        let context = siblings[start..end]
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");

        Ok(context)
    }

    /// Find NER entities by name (partial match). Excludes aliases.
    pub fn find_ner_entities(&self, query: &str) -> Result<Vec<(i64, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, entity_type FROM ner_entities WHERE name LIKE ?1 AND canonical_id IS NULL ORDER BY name"
        )?;
        let pattern = format!("%{}%", query);
        let rows = stmt.query_map(params![pattern], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    // ── Document Tags & Metadata ──

    pub fn add_tag(&self, doc_path: &str, tag: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO document_tags (doc_path, tag) VALUES (?1, ?2)",
            params![doc_path, tag],
        )?;
        Ok(())
    }

    pub fn remove_tag(&self, doc_path: &str, tag: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM document_tags WHERE doc_path = ?1 AND tag = ?2",
            params![doc_path, tag],
        )?;
        Ok(())
    }

    pub fn get_tags(&self, doc_path: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT tag FROM document_tags WHERE doc_path = ?1 ORDER BY tag"
        )?;
        let rows = stmt.query_map(params![doc_path], |row| row.get(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn find_docs_by_tag(&self, tag: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT doc_path FROM document_tags WHERE tag = ?1 ORDER BY doc_path"
        )?;
        let rows = stmt.query_map(params![tag], |row| row.get(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn get_all_tags(&self) -> Result<Vec<(String, u64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT tag, COUNT(*) FROM document_tags GROUP BY tag ORDER BY COUNT(*) DESC"
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn set_metadata(&self, doc_path: &str, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO document_metadata (doc_path, key, value) VALUES (?1, ?2, ?3)",
            params![doc_path, key, value],
        )?;
        Ok(())
    }

    pub fn get_metadata(&self, doc_path: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT key, value FROM document_metadata WHERE doc_path = ?1 ORDER BY key"
        )?;
        let rows = stmt.query_map(params![doc_path], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn delete_tags_by_path(&self, doc_path: &str) -> Result<()> {
        self.conn.execute("DELETE FROM document_tags WHERE doc_path = ?1", params![doc_path])?;
        Ok(())
    }

    pub fn delete_metadata_by_path(&self, doc_path: &str) -> Result<()> {
        self.conn.execute("DELETE FROM document_metadata WHERE doc_path = ?1", params![doc_path])?;
        Ok(())
    }

    /// Search NER entities by exact name (case-insensitive).
    /// Follows canonical_id to return the canonical entity if the match is an alias.
    pub fn find_ner_entity_exact(&self, name: &str) -> Result<Option<(i64, String, String)>> {
        let result = self.conn.query_row(
            "SELECT id, name, entity_type, canonical_id FROM ner_entities WHERE LOWER(name) = LOWER(?1)",
            params![name],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, Option<i64>>(3)?)),
        );
        let found = match result {
            Ok(r) => Some(r),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                // Try partial match
                let result2 = self.conn.query_row(
                    "SELECT id, name, entity_type, canonical_id FROM ner_entities WHERE LOWER(name) LIKE LOWER(?1) ORDER BY LENGTH(name) ASC",
                    params![format!("%{}%", name)],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, Option<i64>>(3)?)),
                );
                match result2 {
                    Ok(r) => Some(r),
                    Err(rusqlite::Error::QueryReturnedNoRows) => None,
                    Err(e) => return Err(e.into()),
                }
            }
            Err(e) => return Err(e.into()),
        };

        match found {
            Some((id, name, etype, canonical_id)) => {
                // If this is an alias, return the canonical entity instead
                if let Some(cid) = canonical_id {
                    let canonical = self.conn.query_row(
                        "SELECT id, name, entity_type FROM ner_entities WHERE id = ?1",
                        params![cid],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )?;
                    Ok(Some(canonical))
                } else {
                    Ok(Some((id, name, etype)))
                }
            }
            None => Ok(None),
        }
    }

    /// Get documents where an NER entity was mentioned (includes alias mentions).
    pub fn get_ner_entity_documents(&self, entity_id: i64) -> Result<Vec<(String, f32)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT doc_path, MAX(confidence) as max_conf
             FROM ner_mentions WHERE entity_id = ?1
                OR entity_id IN (SELECT id FROM ner_entities WHERE canonical_id = ?1)
             GROUP BY doc_path ORDER BY max_conf DESC"
        )?;
        let rows = stmt.query_map(params![entity_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Get all neighbors of an entity in the NER co-occurrence graph.
    /// Excludes entities that have been merged (have a canonical_id).
    /// Returns: (entity_id, name, type, weight, doc_count)
    pub fn get_ner_connections(&self, entity_id: i64) -> Result<Vec<(i64, String, String, f32, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.id, e.name, e.entity_type, ne.weight, ne.doc_count
             FROM ner_edges ne
             JOIN ner_entities e ON (
                 CASE WHEN ne.entity_a = ?1 THEN ne.entity_b ELSE ne.entity_a END = e.id
             )
             WHERE (ne.entity_a = ?1 OR ne.entity_b = ?1)
               AND e.canonical_id IS NULL
             ORDER BY ne.weight DESC"
        )?;
        let rows = stmt.query_map(params![entity_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}
