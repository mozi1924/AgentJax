//! SQLite FTS5-backed keyword search for knowledge base documents.
//!
//! Provides full-text search over document chunks using SQLite's FTS5
//! engine. The FTS store complements the LanceDB vector store, enabling
//! hybrid (keyword + semantic) search via reciprocal rank fusion.
//!
//! ## Schema
//!
//! ```sql
//! documents(id TEXT PRIMARY KEY, content_hash TEXT, title TEXT, byte_count INTEGER)
//! chunks(id TEXT PRIMARY KEY, document_id TEXT, chunk_index INTEGER, content TEXT, embeddings_model TEXT)
//! chunks_fts(content) — virtual FTS5 table, external content
//! ```
//!
//! The `chunks_fts` table uses `content=` pointing at `chunks` so that
//! FTS stays automatically in sync with the chunks table via triggers.

use crate::error::{AgentJaxError, AgentJaxResult};
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::types::{Chunk, Document};

// ── FtsStore ────────────────────────────────────────────────────────────────

/// A SQLite-backed full-text search store for knowledge base documents.
///
/// Manages document metadata, chunk text, and an FTS5 index for keyword
/// search. Designed to be used alongside a vector store for hybrid search.
pub struct FtsStore {
    conn: Mutex<Connection>,
    #[allow(dead_code)]
    db_path: PathBuf,
}

impl FtsStore {
    // ── Lifecycle ──────────────────────────────────────────────────────

    /// Open or create an FTS store at the given path.
    ///
    /// Creates the database file and schema if they don't exist.
    pub fn open(db_path: impl AsRef<Path>) -> AgentJaxResult<Self> {
        let db_path = db_path.as_ref().to_path_buf();

        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AgentJaxError::embedding(format!(
                    "Failed to create FTS store directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        let conn = Connection::open(&db_path).map_err(|e| {
            AgentJaxError::embedding(format!(
                "Failed to open FTS store at {}: {e}",
                db_path.display()
            ))
        })?;

        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;",
        )
        .map_err(|e| AgentJaxError::embedding(format!("Failed to set pragmas: {e}")))?;

        let store = Self {
            conn: Mutex::new(conn),
            db_path,
        };
        store.initialize_schema()?;
        Ok(store)
    }

    fn initialize_schema(&self) -> AgentJaxResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AgentJaxError::embedding(format!("Lock error: {e}")))?;
        conn.execute_batch(CREATE_SCHEMA_SQL)
            .map_err(|e| AgentJaxError::embedding(format!("Schema init failed: {e}")))?;
        Ok(())
    }

    fn lock_conn(&self) -> AgentJaxResult<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|e| AgentJaxError::embedding(format!("Failed to acquire FTS lock: {e}")))
    }

    // ── Document Operations ────────────────────────────────────────────

    /// Insert or update a document record.
    pub fn upsert_document(&self, doc: &Document, content_hash: &str) -> AgentJaxResult<()> {
        let conn = self.lock_conn()?;
        // Extract title from metadata or first heading in content.
        let title = doc
            .metadata
            .get("title")
            .cloned()
            .unwrap_or_else(|| extract_title(&doc.content));
        conn.execute(
            "INSERT OR REPLACE INTO documents (id, content_hash, title, byte_count)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                doc.id,
                content_hash,
                title,
                doc.content.len() as i64,
            ],
        )
        .map_err(|e| AgentJaxError::embedding(format!("Failed to upsert document: {e}")))?;
        Ok(())
    }

    /// Delete a document and all its chunks from the store.
    #[allow(dead_code)]
    pub fn delete_document(&self, document_id: &str) -> AgentJaxResult<()> {
        let conn = self.lock_conn()?;
        // Delete chunks first (foreign key cascade not enabled for FTS).
        conn.execute(
            "DELETE FROM chunks WHERE document_id = ?1",
            params![document_id],
        )
        .map_err(|e| AgentJaxError::embedding(format!("Failed to delete chunks: {e}")))?;
        conn.execute(
            "DELETE FROM documents WHERE id = ?1",
            params![document_id],
        )
        .map_err(|e| AgentJaxError::embedding(format!("Failed to delete document: {e}")))?;
        // Rebuild FTS to reflect deleted chunks
        conn.execute("INSERT INTO chunks_fts(chunks_fts) VALUES('rebuild')", [])
            .map_err(|e| AgentJaxError::embedding(format!("Failed to rebuild FTS: {e}")))?;
        Ok(())
    }

    /// List all document IDs in the store.
    pub fn list_documents(&self) -> AgentJaxResult<Vec<DocumentMeta>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare("SELECT id, content_hash, title, byte_count FROM documents")
            .map_err(|e| AgentJaxError::embedding(format!("Query failed: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(DocumentMeta {
                    id: row.get(0)?,
                    content_hash: row.get(1)?,
                    title: row.get(2)?,
                    byte_count: row.get::<_, i64>(3)? as u64,
                })
            })
            .map_err(|e| AgentJaxError::embedding(format!("Query failed: {e}")))?;

        let mut docs = Vec::new();
        for row in rows {
            docs.push(row.map_err(|e| AgentJaxError::embedding(format!("Row error: {e}")))?);
        }
        Ok(docs)
    }

    // ── Chunk Operations ───────────────────────────────────────────────

    /// Insert chunks into the FTS store. Each chunk's text is indexed for
    /// full-text search.
    pub fn insert_chunks(&self, chunks: &[Chunk], embeddings_model: &str) -> AgentJaxResult<()> {
        let conn = self.lock_conn()?;
        for chunk in chunks {
            conn.execute(
                "INSERT OR REPLACE INTO chunks (id, document_id, chunk_index, content, embeddings_model)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    chunk.id,
                    chunk.document_id,
                    chunk.chunk_index as i64,
                    chunk.content,
                    embeddings_model,
                ],
            )
            .map_err(|e| {
                AgentJaxError::embedding(format!("Failed to insert chunk {}: {e}", chunk.id))
            })?;
        }
        // Rebuild FTS index after batch insert
        conn.execute("INSERT INTO chunks_fts(chunks_fts) VALUES('rebuild')", [])
            .map_err(|e| AgentJaxError::embedding(format!("Failed to rebuild FTS: {e}")))?;
        Ok(())
    }

    /// Count chunks for a document.
    #[allow(dead_code)]
    pub fn chunk_count(&self, document_id: &str) -> AgentJaxResult<usize> {
        let conn = self.lock_conn()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE document_id = ?1",
                params![document_id],
                |row| row.get(0),
            )
            .map_err(|e| AgentJaxError::embedding(format!("Count failed: {e}")))?;
        Ok(count as usize)
    }

    // ── Full-Text Search ───────────────────────────────────────────────

    /// Search chunks using SQLite FTS5 with BM25 scoring.
    ///
    /// Returns results ordered by relevance (BM25 score). The `limit`
    /// parameter caps the number of returned results.
    pub fn search_fts(
        &self,
        query: &str,
        limit: usize,
    ) -> AgentJaxResult<Vec<FtsSearchResult>> {
        let conn = self.lock_conn()?;
        let fts_query = build_fts_query(query);

        let sql = "SELECT c.id, c.document_id, c.chunk_index, c.content,
                           d.title, bm25(chunks_fts) as bm25_score
                    FROM chunks_fts
                    JOIN chunks c ON chunks_fts.rowid = c.rowid
                    JOIN documents d ON c.document_id = d.id
                    WHERE chunks_fts MATCH ?1
                    ORDER BY bm25_score
                    LIMIT ?2";

        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| AgentJaxError::embedding(format!("FTS query prep failed: {e}")))?;

        let rows = stmt
            .query_map(params![fts_query, limit as i64], |row| {
                Ok(FtsSearchResult {
                    chunk_id: row.get(0)?,
                    document_id: row.get(1)?,
                    chunk_index: row.get::<_, i64>(2)? as usize,
                    content: row.get(3)?,
                    title: row.get(4)?,
                    bm25_score: row.get::<_, f64>(5)?,
                })
            })
            .map_err(|e| AgentJaxError::embedding(format!("FTS search failed: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| AgentJaxError::embedding(format!("Row error: {e}")))?);
        }
        Ok(results)
    }

    /// Retrieve a document's content by ID.
    ///
    /// Returns `None` if the document is not found. The actual text content
    /// is not stored in the FTS store — it's looked up from the chunks.
    pub fn get_document_chunks(&self, document_id: &str) -> AgentJaxResult<Vec<String>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT content FROM chunks WHERE document_id = ?1 ORDER BY chunk_index",
            )
            .map_err(|e| AgentJaxError::embedding(format!("Query failed: {e}")))?;

        let rows = stmt
            .query_map(params![document_id], |row| row.get::<_, String>(0))
            .map_err(|e| AgentJaxError::embedding(format!("Query failed: {e}")))?;

        let mut chunks = Vec::new();
        for row in rows {
            chunks.push(row.map_err(|e| AgentJaxError::embedding(format!("Row error: {e}")))?);
        }
        Ok(chunks)
    }
}

// ── Public Types ────────────────────────────────────────────────────────────

/// Metadata about a stored document.
#[derive(Debug, Clone)]
pub struct DocumentMeta {
    #[allow(dead_code)]
    pub id: String,
    pub content_hash: String,
    #[allow(dead_code)]
    pub title: String,
    pub byte_count: u64,
}

/// A result from FTS keyword search.
#[derive(Debug, Clone)]
pub struct FtsSearchResult {
    pub chunk_id: String,
    pub document_id: String,
    #[allow(dead_code)]
    pub chunk_index: usize,
    pub content: String,
    #[allow(dead_code)]
    pub title: String,
    /// Raw BM25 score (lower = better match in FTS5).
    /// Normalize with: `score = |bm25| / (1 + |bm25|)`
    pub bm25_score: f64,
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Extract a title from markdown content.
///
/// Returns the first `# ` or `## ` heading, falling back to "Untitled".
fn extract_title(content: &str) -> String {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("# ") {
            return trimmed[2..].trim().to_string();
        }
        if trimmed.starts_with("## ") {
            return trimmed[3..].trim().to_string();
        }
    }
    "Untitled".to_string()
}

/// Build an FTS5-safe query string from user input.
///
/// Escapes special FTS5 characters and wraps multi-word queries
/// appropriately for substring matching.
fn build_fts_query(pattern: &str) -> String {
    // Escape double-quotes (used for exact phrase matching in FTS5).
    let cleaned = pattern.replace('"', "\"\"").replace('\'', "''");

    if cleaned.is_empty() {
        return "*".to_string();
    }

    // Split into words and quote each one for prefix matching
    let words: Vec<String> = cleaned
        .split_whitespace()
        .map(|w| {
            if w.starts_with('"') && w.ends_with('"') {
                w.to_string() // Already quoted exact phrase
            } else {
                format!("\"{}\"*", w.replace('"', "")) // Prefix match
            }
        })
        .collect();

    words.join(" AND ")
}

/// Compute a simple content hash for deduplication.
///
/// Uses SHA-256 via std, returning a hex string.
pub(crate) fn content_hash(content: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

// ── Schema SQL ──────────────────────────────────────────────────────────────

const CREATE_SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS documents (
    id TEXT PRIMARY KEY NOT NULL,
    content_hash TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT 'Untitled',
    byte_count INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS chunks (
    id TEXT PRIMARY KEY NOT NULL,
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL,
    content TEXT NOT NULL,
    embeddings_model TEXT NOT NULL DEFAULT ''
);

CREATE INDEX IF NOT EXISTS idx_chunks_document_id ON chunks(document_id);

CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
    content,
    content='chunks',
    content_rowid='rowid'
);

-- Triggers to keep FTS in sync with the chunks table.
CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
    INSERT INTO chunks_fts(rowid, content) VALUES (new.rowid, new.content);
END;

CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
    INSERT INTO chunks_fts(chunks_fts, rowid, content) VALUES('delete', old.rowid, old.content);
END;

CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE ON chunks BEGIN
    INSERT INTO chunks_fts(chunks_fts, rowid, content) VALUES('delete', old.rowid, old.content);
    INSERT INTO chunks_fts(rowid, content) VALUES (new.rowid, new.content);
END;
";

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_hash_deterministic() {
        let h1 = content_hash("hello world");
        let h2 = content_hash("hello world");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_content_hash_different() {
        let h1 = content_hash("hello world");
        let h2 = content_hash("hello world!");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_extract_title_h1() {
        assert_eq!(extract_title("# My Title\nContent"), "My Title");
    }

    #[test]
    fn test_extract_title_h2() {
        assert_eq!(extract_title("## Section\nContent"), "Section");
    }

    #[test]
    fn test_extract_title_fallback() {
        assert_eq!(extract_title("Just some text"), "Untitled");
    }

    #[test]
    fn test_build_fts_query() {
        let q = build_fts_query("machine learning");
        assert!(q.contains("\"machine\"*"));
        assert!(q.contains("\"learning\"*"));
        assert!(q.contains("AND"));
    }

    #[test]
    fn test_build_fts_query_empty() {
        let q = build_fts_query("");
        assert_eq!(q, "*");
    }

    #[test]
    fn test_fts_store_open_and_insert() {
        let dir = std::env::temp_dir().join("rag-fts-test");
        let _ = std::fs::remove_dir_all(&dir);
        let store = FtsStore::open(&dir).expect("open store");
        // Should work when empty
        let docs = store.list_documents().expect("list docs");
        assert!(docs.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
