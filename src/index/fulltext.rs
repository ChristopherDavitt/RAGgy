use anyhow::Result;
use std::path::Path;
use tantivy::{
    schema::{Schema, TEXT, STORED, FAST, Field, Value},
    Index, IndexWriter, IndexReader,
    collector::TopDocs,
    query::QueryParser,
    doc,
    TantivyDocument,
    directory::MmapDirectory,
    IndexSettings,
};

pub struct FulltextIndex {
    index: Index,
    writer: IndexWriter,
    reader: IndexReader,
    node_id_field: Field,
    doc_path_field: Field,
    content_field: Field,
    title_field: Field,
    content_type_field: Field,
}

pub struct SearchResult {
    pub node_id: u64,
    pub doc_path: String,
    pub score: f32,
    pub snippet: String,
    pub title: String,
    pub content_type: String,
}

impl FulltextIndex {
    pub fn open(index_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(index_dir)?;

        // Remove stale lock files left by hard kills (Ctrl+C / abort)
        let lock_path = index_dir.join(".tantivy-writer.lock");
        if lock_path.exists() {
            let _ = std::fs::remove_file(&lock_path);
        }

        let mut schema_builder = Schema::builder();
        let node_id_field = schema_builder.add_u64_field("node_id", STORED | FAST);
        let doc_path_field = schema_builder.add_text_field("doc_path", STORED);
        let content_field = schema_builder.add_text_field("content", TEXT | STORED);
        let title_field = schema_builder.add_text_field("title", TEXT | STORED);
        let content_type_field = schema_builder.add_text_field("content_type", STORED);
        let schema = schema_builder.build();

        let dir = MmapDirectory::open(index_dir)?;

        let index = if Index::exists(&dir)? {
            Index::open(dir)?
        } else {
            Index::create(dir, schema, IndexSettings::default())?
        };

        let writer = index.writer(50_000_000)?; // 50MB buffer
        let reader = index.reader()?;

        Ok(FulltextIndex {
            index,
            writer,
            reader,
            node_id_field,
            doc_path_field,
            content_field,
            title_field,
            content_type_field,
        })
    }

    pub fn add_document(
        &self,
        node_id: u64,
        doc_path: &str,
        content: &str,
        title: &str,
        content_type: &str,
    ) -> Result<()> {
        self.writer.add_document(doc!(
            self.node_id_field => node_id,
            self.doc_path_field => doc_path,
            self.content_field => content,
            self.title_field => title,
            self.content_type_field => content_type,
        ))?;
        Ok(())
    }

    pub fn delete_by_path(&self, doc_path: &str) -> Result<()> {
        let term = tantivy::Term::from_field_text(self.doc_path_field, doc_path);
        self.writer.delete_term(term);
        Ok(())
    }

    pub fn commit(&self) -> Result<()> {
        // IndexWriter::commit requires &mut self, use unsafe to work around shared ref
        let writer_ptr = &self.writer as *const IndexWriter as *mut IndexWriter;
        unsafe { (*writer_ptr).commit()? };
        Ok(())
    }

    pub fn search(&self, query_string: &str, limit: usize) -> Result<Vec<SearchResult>> {
        self.reader.reload()?;
        let searcher = self.reader.searcher();

        let query_parser = QueryParser::for_index(
            &self.index,
            vec![self.content_field, self.title_field],
        );

        let query = match query_parser.parse_query(query_string) {
            Ok(q) => q,
            Err(_) => {
                // Fallback: escape the query and try again
                let escaped = query_string
                    .replace('(', "\\(")
                    .replace(')', "\\)")
                    .replace('[', "\\[")
                    .replace(']', "\\]");
                query_parser.parse_query(&escaped)?
            }
        };

        let top_docs = searcher.search(&query, &TopDocs::with_limit(limit))?;

        let mut results = Vec::new();
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address)?;

            let node_id = doc.get_first(self.node_id_field)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            let doc_path = doc.get_first(self.doc_path_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let content = doc.get_first(self.content_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let title = doc.get_first(self.title_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let content_type = doc.get_first(self.content_type_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let snippet = make_snippet(&content, query_string);

            results.push(SearchResult {
                node_id,
                doc_path,
                score,
                snippet,
                title,
                content_type,
            });
        }

        Ok(results)
    }

    pub fn index_size_bytes(&self) -> Result<u64> {

        let metas = self.index.load_metas()?;
        let mut total = 0u64;
        for segment in metas.segments {
            total += segment.max_doc() as u64 * 100; // rough estimate
        }
        Ok(total)
    }
}

/// Generate a context-aware snippet by finding a ~300-char window around the first
/// matching query term. Falls back to first 300 chars if no term matches.
pub fn make_snippet(content: &str, query: &str) -> String {
    const WINDOW: usize = 300;

    let content_lower = content.to_lowercase();
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|t| t.to_lowercase())
        .filter(|t| t.len() > 1)
        .collect();

    // Find the byte position of the first matching term
    let match_pos = terms.iter()
        .filter_map(|term| content_lower.find(term.as_str()))
        .min();

    let (start_byte, end_byte) = if let Some(pos) = match_pos {
        // Center a window around the match
        let half = WINDOW / 2;
        let raw_start = pos.saturating_sub(half);
        let raw_end = (pos + half).min(content.len());

        // Snap to char boundaries using char_indices
        let start = content[..raw_start]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
        let end = content[raw_end..]
            .char_indices()
            .next()
            .map(|(i, _)| raw_end + i)
            .unwrap_or(content.len());

        // Snap to word boundaries (avoid cutting mid-word)
        let start = if start > 0 {
            content[start..].find(' ').map(|i| start + i + 1).unwrap_or(start)
        } else {
            0
        };
        let end = if end < content.len() {
            content[..end].rfind(' ').unwrap_or(end)
        } else {
            content.len()
        };

        (start, end)
    } else {
        // No match found, take the first WINDOW chars
        let end = if content.len() > WINDOW {
            content[..WINDOW]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(WINDOW)
        } else {
            content.len()
        };
        // Snap end to word boundary
        let end = if end < content.len() {
            content[..end].rfind(' ').unwrap_or(end)
        } else {
            end
        };
        (0, end)
    };

    let slice = content[start_byte..end_byte].trim();
    let prefix = if start_byte > 0 { "..." } else { "" };
    let suffix = if end_byte < content.len() { "..." } else { "" };

    format!("{}{}{}", prefix, slice, suffix)
}
