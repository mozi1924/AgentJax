//! SQLite-backed Immutable Store for the LCM system.
//!
//! The LcmStore is the source of truth for all conversation data under LCM.
//! It provides:
//!
//! - **Transactional persistence** of messages, summaries, and file references
//! - **Full-text search** via SQLite FTS5 for `lcm_grep`
//! - **Referential integrity** enforced through foreign keys
//! - **Atomic compaction** through SQL transactions
//!
//! ## Schema
//!
//! ```sql
//! messages(id, conversation_id, role, content, token_count, ts, covered_by)
//! summaries(id, conversation_id, kind, text, token_count, ts, compaction_level)
//! summary_children(summary_id, child_type, child_id)
//! file_refs(id, conversation_id, path, mime_type, token_count, exploration_summary, ts)
//! messages_fts(search_text) — virtual FTS5 table
//! ```

use crate::conversation_store_utils::normalize_title_source;
use crate::lcm::types::{
    ConversationMeta, DescribeResult, FileRefId, FileReference, GrepResult, LcmConfig, LcmError,
    LcmId, MessageId, MessageRole, PaginatedGrepResults, StoredMessage, SummaryChild, SummaryId,
    SummaryKind, SummaryNode,
};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The LCM Immutable Store.
///
/// Wraps a SQLite connection behind a mutex for thread-safe access.
/// All write operations are transactional.
pub struct LcmStore {
    conn: Mutex<Connection>,
    db_path: PathBuf,
    grep_page_size: usize,
}

impl LcmStore {
    // ── Lifecycle ──────────────────────────────────────────────────────

    /// Open (or create) the LCM store at the given path.
    ///
    /// If the database file does not exist, it will be created with the
    /// full schema. If it already exists, the schema is migrated if needed.
    pub fn open(db_path: impl AsRef<Path>, config: LcmConfig) -> Result<Self, LcmError> {
        let db_path = db_path.as_ref().to_path_buf();
        let grep_page_size = config.grep_page_size;

        // Ensure parent directory exists.
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                LcmError::Store(format!(
                    "Failed to create LCM store directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        let conn = Connection::open(&db_path).map_err(|e| {
            LcmError::Store(format!(
                "Failed to open LCM store at {}: {e}",
                db_path.display()
            ))
        })?;

        // Enable WAL mode for better concurrent read performance.
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;",
        )
        .map_err(|e| LcmError::Store(format!("Failed to set pragmas: {e}")))?;

        let store = Self {
            conn: Mutex::new(conn),
            db_path,
            grep_page_size,
        };

        store.initialize_schema()?;
        Ok(store)
    }

    /// Open an in-memory LCM store.
    ///
    /// The store exists only for the lifetime of the `LcmStore` object.
    /// No data is persisted to disk. Useful for ephemeral sub-agents,
    /// tests, and short-lived contexts where disk I/O is wasteful.
    pub fn open_in_memory(config: LcmConfig) -> Result<Self, LcmError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| LcmError::Store(format!("Failed to open in-memory LCM store: {e}")))?;

        conn.execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(|e| LcmError::Store(format!("Failed to set pragmas: {e}")))?;

        let store = Self {
            conn: Mutex::new(conn),
            db_path: PathBuf::from(":memory:"),
            grep_page_size: config.grep_page_size,
        };

        store.initialize_schema()?;
        Ok(store)
    }

    /// Initialize the database schema, creating tables if they don't exist.
    fn initialize_schema(&self) -> Result<(), LcmError> {
        let conn = self.lock_conn()?;

        conn.execute_batch(CREATE_SCHEMA_SQL)
            .map_err(|e| LcmError::Store(format!("Failed to initialize LCM schema: {e}")))?;

        // Migrations: add columns that may not exist in older databases.
        // SQLite does not support IF NOT EXISTS for ALTER TABLE ADD COLUMN,
        // so we silently ignore "duplicate column" errors.
        for migration in SCHEMA_MIGRATIONS {
            if let Err(e) = conn.execute_batch(migration) {
                // Error code 1 with "duplicate column" is expected for existing DBs.
                let msg = e.to_string();
                if !msg.contains("duplicate column") {
                    log::warn!("LCM schema migration warning: {msg}");
                }
            }
        }

        Ok(())
    }

    /// Returns the path to the database file.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Returns the configured page size for grep results.
    pub fn grep_page_size(&self) -> usize {
        self.grep_page_size
    }

    // ── Internal Helpers ───────────────────────────────────────────────

    /// Acquire the SQLite mutex lock with a consistent error message.
    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, LcmError> {
        self.conn
            .lock()
            .map_err(|e| LcmError::Concurrency(format!("Failed to acquire store lock: {e}")))
    }

// ── Message Persistence ────────────────────────────────────────────

    /// Persist a new message into the immutable store.
    ///
    /// This is the primary write path. Messages are **never modified**
    /// after insertion — only `covered_by` may be updated during compaction.
    pub fn persist_message(&self, msg: &StoredMessage) -> Result<(), LcmError> {
        let conn = self.lock_conn()?;

        let metadata_json = serde_json::to_string(&msg.metadata).unwrap_or_default();
        let file_refs_json = serde_json::to_string(&msg.file_refs).unwrap_or_default();

        conn.execute(
            MESSAGE_INSERT_SQL,
            params![
                msg.id.as_str(),
                msg.conversation_id,
                msg.role.as_str(),
                msg.content,
                msg.token_count,
                msg.timestamp_unix_ms,
                msg.covered_by.as_ref().map(|s| s.as_str()),
                msg.seq,
                msg.hop_index,
                msg.thinking.as_deref(),
                msg.search_text(),
                metadata_json,
                file_refs_json,
            ],
        )
        .map_err(|e| LcmError::Store(format!("Failed to persist message {}: {e}", msg.id)))?;

        Ok(())
    }

    /// Persist multiple messages in a single transaction.
    pub fn persist_messages(&self, messages: &[StoredMessage]) -> Result<(), LcmError> {
        let conn = self.lock_conn()?;

        let tx = conn
            .unchecked_transaction()
            .map_err(|e| LcmError::Store(format!("Failed to begin transaction: {e}")))?;

        {
            let mut stmt = tx
                .prepare(MESSAGE_INSERT_SQL)
                .map_err(|e| LcmError::Store(format!("Failed to prepare insert: {e}")))?;

            for msg in messages {
                let metadata_json = serde_json::to_string(&msg.metadata).unwrap_or_default();
                let file_refs_json = serde_json::to_string(&msg.file_refs).unwrap_or_default();
                stmt.execute(params![
                    msg.id.as_str(),
                    msg.conversation_id,
                    msg.role.as_str(),
                    msg.content,
                    msg.token_count,
                    msg.timestamp_unix_ms,
                    msg.covered_by.as_ref().map(|s| s.as_str()),
                    msg.seq,
                    msg.hop_index,
                    msg.thinking.as_deref(),
                    msg.search_text(),
                    metadata_json,
                    file_refs_json,
                ])
                .map_err(|e| {
                    LcmError::Store(format!("Failed to insert message {}: {e}", msg.id))
                })?;
            }
        }

        tx.commit()
            .map_err(|e| LcmError::Store(format!("Failed to commit transaction: {e}")))?;

        Ok(())
    }

    // ── Message Retrieval ──────────────────────────────────────────────

    /// Retrieve a message by its ID.
    pub fn get_message(&self, id: &MessageId) -> Result<Option<StoredMessage>, LcmError> {
        let conn = self.lock_conn()?;

        let mut stmt = conn
            .prepare(&format!("{} WHERE id = ?1", MESSAGE_SELECT_SQL))
            .map_err(|e| LcmError::Store(format!("Failed to prepare query: {e}")))?;

        let result = stmt
            .query_row(params![id.as_str()], |row| {
                row_to_stored_message(row, Some(9), 10, 11)
            })
            .optional()
            .map_err(|e| LcmError::Store(format!("Failed to query message {id}: {e}")))?;

        Ok(result)
    }

    /// Retrieve all messages for a conversation, ordered by global sequence.
    pub fn get_conversation_messages(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<StoredMessage>, LcmError> {
        let conn = self.lock_conn()?;

        // Order by seq ASC to ensure correct hop-by-hop context reconstruction.
        let mut stmt = conn
            .prepare(&format!(
                "{} WHERE conversation_id = ?1 ORDER BY seq ASC",
                MESSAGE_SELECT_SQL,
            ))
            .map_err(|e| LcmError::Store(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map(params![conversation_id], |row| {
                row_to_stored_message(row, Some(9), 10, 11)
            })
            .map_err(|e| LcmError::Store(format!("Failed to query messages: {e}")))?;

        let mut messages = Vec::new();
        for row in rows {
            messages.push(row?);
        }
        Ok(messages)
    }

    // ── Full-Text Search (lcm_grep) ────────────────────────────────────

    /// Search messages using SQLite FTS5.
    ///
    /// When `summary_id` is provided, restricts the search to messages
    /// covered by that summary node and its descendants.
    pub fn search_messages(
        &self,
        conversation_id: &str,
        pattern: &str,
        summary_id: Option<&SummaryId>,
        cursor: Option<&str>,
        page_size: usize,
    ) -> Result<PaginatedGrepResults, LcmError> {
        let conn = self.lock_conn()?;

        // Build the base query. We search the messages_fts table and join
        // back to messages for full data.
        let mut sql = "SELECT m.id, m.conversation_id, m.role, m.content, m.token_count,
                    m.timestamp_unix_ms, m.covered_by,
                    -- seq and hop_index for context reconstruction
                    m.seq, m.hop_index,
                    -- Note: column order differs from MESSAGE_SELECT_SQL (no thinking)
                    m.metadata_json, m.file_refs_json,
                    snippet(messages_fts, 2, '<mark>', '</mark>', '...', 40) as snippet
             FROM messages_fts
             JOIN messages m ON messages_fts.rowid = m.rowid
             WHERE m.conversation_id = ?1
               AND messages_fts MATCH ?2".to_string();

        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
            Box::new(conversation_id.to_string()),
            Box::new(build_fts_query(pattern)),
        ];

        // If scoped to a summary, filter to messages covered by it.
        if let Some(sid) = summary_id {
            sql.push_str(" AND m.covered_by = ?3");
            param_values.push(Box::new(sid.to_string()));
        }

        // Pagination cursor.
        if let Some(c) = cursor {
            let cursor_idx: usize = c
                .parse()
                .map_err(|_| LcmError::Store("Invalid pagination cursor".to_string()))?;
            sql.push_str(&format!(
                " LIMIT ?{} OFFSET ?{}",
                param_values.len() + 1,
                param_values.len() + 2
            ));
            param_values.push(Box::new(page_size as i64));
            param_values.push(Box::new(cursor_idx as i64));
        } else {
            sql.push_str(&format!(" LIMIT ?{}", param_values.len() + 1));
            param_values.push(Box::new((page_size + 1) as i64)); // +1 to detect has_more
        }

        sql.push_str(" ORDER BY m.timestamp_unix_ms DESC");

        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| LcmError::Store(format!("Failed to prepare FTS query: {e}")))?;

        let rows = stmt
            .query_map(params_ref.as_slice(), |row| {
                let snippet: String = row.get(11)?;
                let msg = row_to_stored_message(row, None, 9, 10)?;
                Ok((msg, snippet))
            })
            .map_err(|e| LcmError::Store(format!("Failed to execute FTS query: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            let (msg, snippet) = row?;
            results.push(GrepResult {
                covered_by_summary: msg.covered_by.clone(),
                match_context: snippet,
                message: msg,
            });
        }

        let total_count = results.len();
        let has_more = total_count > page_size;
        if has_more {
            results.truncate(page_size);
        }

        let next_cursor = if has_more {
            let current_offset: usize = cursor.and_then(|c| c.parse().ok()).unwrap_or(0);
            Some((current_offset + page_size).to_string())
        } else {
            None
        };

        Ok(PaginatedGrepResults {
            results,
            total_count,
            has_more,
            next_cursor,
        })
    }

    // ── Summary Operations ─────────────────────────────────────────────

    /// Insert a new summary node into the DAG.
    pub fn insert_summary(&self, summary: &SummaryNode) -> Result<(), LcmError> {
        let conn = self.lock_conn()?;

        conn.execute(
            "INSERT OR IGNORE INTO summaries (id, conversation_id, kind, text, token_count, created_at_unix_ms, compaction_level)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                summary.id.as_str(),
                summary.conversation_id,
                summary_kind_str(summary.kind),
                summary.text,
                summary.token_count,
                summary.created_at_unix_ms,
                summary.compaction_level,
            ],
        )
        .map_err(|e| LcmError::Store(format!("Failed to insert summary {}: {e}", summary.id)))?;

        Ok(())
    }

    /// Add a child edge to a summary node.
    pub fn add_summary_child(
        &self,
        summary_id: &SummaryId,
        child: &SummaryChild,
    ) -> Result<(), LcmError> {
        let conn = self.lock_conn()?;

        match child {
            SummaryChild::Messages { ids } => {
                let mut stmt = conn
                    .prepare(
                        "INSERT OR IGNORE INTO summary_children (summary_id, child_type, child_id)
                         VALUES (?1, 'message', ?2)",
                    )
                    .map_err(|e| LcmError::Store(format!("Failed to prepare insert: {e}")))?;

                for id in ids {
                    stmt.execute(params![summary_id.as_str(), id.as_str()])
                        .map_err(|e| {
                            LcmError::Store(format!(
                                "Failed to add child {id} to summary {summary_id}: {e}"
                            ))
                        })?;
                }
            }
            SummaryChild::Summaries { ids } => {
                let mut stmt = conn
                    .prepare(
                        "INSERT OR IGNORE INTO summary_children (summary_id, child_type, child_id)
                         VALUES (?1, 'summary', ?2)",
                    )
                    .map_err(|e| LcmError::Store(format!("Failed to prepare insert: {e}")))?;

                for id in ids {
                    stmt.execute(params![summary_id.as_str(), id.as_str()])
                        .map_err(|e| {
                            LcmError::Store(format!(
                                "Failed to add child {id} to summary {summary_id}: {e}"
                            ))
                        })?;
                }
            }
        }

        Ok(())
    }

    /// Add a parent back-reference to a summary node.
    /// Called when a condensed summary is created that covers this summary.
    pub fn add_summary_parent(
        &self,
        summary_id: &SummaryId,
        parent_id: &SummaryId,
    ) -> Result<(), LcmError> {
        let conn = self.lock_conn()?;

        conn.execute(
            "INSERT OR IGNORE INTO summary_parents (summary_id, parent_id) VALUES (?1, ?2)",
            params![summary_id.as_str(), parent_id.as_str()],
        )
        .map_err(|e| {
            LcmError::Store(format!(
                "Failed to add parent {parent_id} to summary {summary_id}: {e}"
            ))
        })?;

        Ok(())
    }

    /// Internal helper: query parents for a summary (requires an open connection).
    fn get_summary_parents_internal(
        &self,
        conn: &Connection,
        summary_id: &SummaryId,
    ) -> Result<Vec<SummaryId>, LcmError> {
        let mut stmt = conn
            .prepare("SELECT parent_id FROM summary_parents WHERE summary_id = ?1")
            .map_err(|e| LcmError::Store(format!("Failed to prepare parents query: {e}")))?;

        let rows = stmt
            .query_map(params![summary_id.as_str()], |row| {
                Ok(SummaryId::from(row.get::<_, String>(0)?))
            })
            .map_err(|e| LcmError::Store(format!("Failed to query parents: {e}")))?;

        let mut parents = Vec::new();
        for row in rows {
            parents.push(row?);
        }
        Ok(parents)
    }

    /// Update messages to mark them as covered by a summary.
    /// Called during compaction to link raw messages to their summary node.
    pub fn mark_messages_covered(
        &self,
        message_ids: &[MessageId],
        summary_id: &SummaryId,
    ) -> Result<(), LcmError> {
        let conn = self.lock_conn()?;

        let tx = conn
            .unchecked_transaction()
            .map_err(|e| LcmError::Store(format!("Failed to begin transaction: {e}")))?;

        {
            let mut stmt = tx
                .prepare("UPDATE messages SET covered_by = ?1 WHERE id = ?2")
                .map_err(|e| LcmError::Store(format!("Failed to prepare update: {e}")))?;

            for id in message_ids {
                stmt.execute(params![summary_id.as_str(), id.as_str()])
                    .map_err(|e| {
                        LcmError::Store(format!("Failed to mark message {id} as covered: {e}"))
                    })?;
            }
        }

        tx.commit()
            .map_err(|e| LcmError::Store(format!("Failed to commit covered_by update: {e}")))?;

        Ok(())
    }

    /// Get a summary node by ID.
    pub fn get_summary(&self, id: &SummaryId) -> Result<Option<SummaryNode>, LcmError> {
        let conn = self.lock_conn()?;

        let mut stmt = conn
            .prepare(
                "SELECT id, conversation_id, kind, text, token_count, created_at_unix_ms, compaction_level
                 FROM summaries WHERE id = ?1",
            )
            .map_err(|e| LcmError::Store(format!("Failed to prepare query: {e}")))?;

        let mut result = stmt
            .query_row(params![id.as_str()], |row| {
                Ok(SummaryNode {
                    id: SummaryId::from(row.get::<_, String>(0)?),
                    conversation_id: row.get(1)?,
                    kind: parse_summary_kind(row.get::<_, String>(2)?.as_str())?,
                    text: row.get(3)?,
                    token_count: row.get(4)?,
                    created_at_unix_ms: row.get(5)?,
                    compaction_level: row.get(6)?,
                    parents: Vec::new(), // populated below
                    file_refs: Vec::new(),
                })
            })
            .optional()
            .map_err(|e| LcmError::Store(format!("Failed to query summary {id}: {e}")))?;

        // Drop stmt to release its borrow on conn, then populate parents.
        drop(stmt);

        if let Some(ref mut summary) = result {
            summary.parents = self.get_summary_parents_internal(&conn, &summary.id)?;
        }

        Ok(result)
    }

    /// Get the children of a summary node.
    pub fn get_summary_children(
        &self,
        summary_id: &SummaryId,
    ) -> Result<Vec<SummaryChild>, LcmError> {
        let conn = self.lock_conn()?;

        let mut stmt = conn
            .prepare(
                "SELECT child_type, child_id FROM summary_children WHERE summary_id = ?1
                 ORDER BY child_type, child_id",
            )
            .map_err(|e| LcmError::Store(format!("Failed to prepare query: {e}")))?;

        let rows_iter = stmt
            .query_map(params![summary_id.as_str()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| LcmError::Store(format!("Failed to query children: {e}")))?;

        let mut rows: Vec<(String, String)> = Vec::new();
        for row_result in rows_iter {
            let pair = row_result
                .map_err(|e| LcmError::Store(format!("Failed to read child row: {e}")))?;
            rows.push(pair);
        }

        let mut message_ids = Vec::new();
        let mut summary_ids = Vec::new();

        for (child_type, child_id) in rows {
            match child_type.as_str() {
                "message" => message_ids.push(MessageId::from(child_id)),
                "summary" => summary_ids.push(SummaryId::from(child_id)),
                _ => {}
            }
        }

        let mut children = Vec::new();
        if !message_ids.is_empty() {
            children.push(SummaryChild::Messages { ids: message_ids });
        }
        if !summary_ids.is_empty() {
            children.push(SummaryChild::Summaries { ids: summary_ids });
        }

        Ok(children)
    }

    // ── Summary Queries ────────────────────────────────────────────────

    /// Get all summaries for a conversation.
    pub fn get_conversation_summaries(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<SummaryNode>, LcmError> {
        let conn = self.lock_conn()?;

        let mut stmt = conn
            .prepare(
                "SELECT id, conversation_id, kind, text, token_count, created_at_unix_ms, compaction_level
                 FROM summaries WHERE conversation_id = ?1 ORDER BY created_at_unix_ms ASC",
            )
            .map_err(|e| LcmError::Store(format!("Failed to prepare summary query: {e}")))?;

        let rows = stmt
            .query_map(params![conversation_id], |row| {
                let id_str: String = row.get(0)?;
                let kind_str: String = row.get(2)?;
                let kind = match kind_str.as_str() {
                    "leaf" => SummaryKind::Leaf,
                    "condensed" => SummaryKind::Condensed,
                    _ => SummaryKind::Leaf,
                };

                Ok(SummaryNode {
                    id: SummaryId::from(id_str),
                    conversation_id: row.get(1)?,
                    kind,
                    text: row.get(3)?,
                    token_count: row.get::<_, i64>(4).unwrap_or(0) as u32,
                    created_at_unix_ms: row.get(5)?,
                    compaction_level: row.get::<_, i64>(6).unwrap_or(0) as u8,
                    parents: Vec::new(), // loaded separately
                    file_refs: Vec::new(),
                })
            })
            .map_err(|e| LcmError::Store(format!("Failed to query summaries: {e}")))?;

        let mut summaries: Vec<SummaryNode> = rows.filter_map(|r| r.ok()).collect();

        // Load parent references for each summary.
        for summary in &mut summaries {
            let mut parent_stmt = conn
                .prepare("SELECT parent_id FROM summary_parents WHERE summary_id = ?1")
                .map_err(|e| LcmError::Store(format!("Failed to prepare parent query: {e}")))?;

            let parent_ids: Vec<SummaryId> = parent_stmt
                .query_map(params![summary.id.as_str()], |row| {
                    let id_str: String = row.get(0)?;
                    Ok(SummaryId::from(id_str))
                })
                .map_err(|e| LcmError::Store(format!("Failed to query parents: {e}")))?
                .filter_map(|r| r.ok())
                .collect();

            summary.parents = parent_ids;
        }

        Ok(summaries)
    }

    // ── File Reference Operations ──────────────────────────────────────

    /// Get a file reference by ID.
    pub fn get_file_ref(&self, id: &FileRefId) -> Result<Option<FileReference>, LcmError> {
        let conn = self.lock_conn()?;

        let mut stmt = conn
            .prepare(
                "SELECT id, conversation_id, path, mime_type, token_count, exploration_summary, registered_at_unix_ms
                 FROM file_refs WHERE id = ?1",
            )
            .map_err(|e| LcmError::Store(format!("Failed to prepare query: {e}")))?;

        let result = stmt
            .query_row(params![id.as_str()], |row| {
                Ok(FileReference {
                    id: FileRefId::from(row.get::<_, String>(0)?),
                    conversation_id: row.get(1)?,
                    path: row.get(2)?,
                    mime_type: row.get(3)?,
                    token_count: row.get(4)?,
                    exploration_summary: row.get(5)?,
                    registered_at_unix_ms: row.get(6)?,
                })
            })
            .optional()
            .map_err(|e| LcmError::Store(format!("Failed to query file ref {id}: {e}")))?;

        Ok(result)
    }

    // ── Describe (lcm_describe) ────────────────────────────────────────

    /// Describe any LCM entity by its ID.
    ///
    /// Tries message first, then summary, then file reference.
    pub fn describe(&self, id: &LcmId) -> Result<Option<DescribeResult>, LcmError> {
        // Try as message.
        if let Some(msg) = self.get_message(&MessageId::from(id.as_str()))? {
            return Ok(Some(DescribeResult::Message {
                id: msg.id,
                role: msg.role,
                token_count: msg.token_count,
                timestamp_unix_ms: msg.timestamp_unix_ms,
                covered_by: msg.covered_by,
            }));
        }

        // Try as summary.
        if let Some(summary) = self.get_summary(&SummaryId::from(id.as_str()))? {
            let children = self.get_summary_children(&summary.id)?;
            let child_count: usize = children.iter().map(|c| c.len()).sum();
            return Ok(Some(DescribeResult::Summary {
                id: summary.id,
                kind: summary.kind,
                token_count: summary.token_count,
                compaction_level: summary.compaction_level,
                created_at_unix_ms: summary.created_at_unix_ms,
                parents: summary.parents,
                child_count,
                file_refs: summary.file_refs,
                text: summary.text,
            }));
        }

        // Try as file reference.
        if let Some(file_ref) = self.get_file_ref(&FileRefId::from(id.as_str()))? {
            return Ok(Some(DescribeResult::File {
                id: file_ref.id,
                path: file_ref.path,
                mime_type: file_ref.mime_type,
                token_count: file_ref.token_count,
                exploration_summary: file_ref.exploration_summary,
                registered_at_unix_ms: file_ref.registered_at_unix_ms,
            }));
        }

        Ok(None)
    }

    // ── Expand (lcm_expand) ────────────────────────────────────────────

    /// Expand a summary node into its constituent messages.
    ///
    /// Recursively traverses the DAG: if a child is itself a summary,
    /// it is expanded recursively. Returns all raw messages at the leaves.
    pub fn expand_summary(&self, summary_id: &SummaryId) -> Result<Vec<StoredMessage>, LcmError> {
        let mut all_messages: Vec<StoredMessage> = Vec::new();
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();

        self.expand_summary_recursive(summary_id, &mut all_messages, &mut visited)?;

        // Sort by timestamp for chronological order.
        all_messages.sort_by_key(|m| m.timestamp_unix_ms);

        Ok(all_messages)
    }

    fn expand_summary_recursive(
        &self,
        summary_id: &SummaryId,
        messages: &mut Vec<StoredMessage>,
        visited: &mut std::collections::HashSet<String>,
    ) -> Result<(), LcmError> {
        // Guard against cycles (should not happen in a DAG, but be safe).
        if !visited.insert(summary_id.to_string()) {
            return Ok(());
        }

        let children = self.get_summary_children(summary_id)?;

        for child in children {
            match child {
                SummaryChild::Messages { ids } => {
                    for id in ids {
                        if let Some(msg) = self.get_message(&id)? {
                            messages.push(msg);
                        }
                    }
                }
                SummaryChild::Summaries { ids } => {
                    for id in ids {
                        self.expand_summary_recursive(&id, messages, visited)?;
                    }
                }
            }
        }

        Ok(())
    }

    // ── Conversation Metadata ──────────────────────────────────────────

    /// Ensure conversation metadata exists (create if missing).
    pub fn ensure_conversation_meta(
        &self,
        conversation_id: &str,
    ) -> Result<ConversationMeta, LcmError> {
        let conn = self.lock_conn()?;

        let existing = conn
            .query_row(
                &format!("{} WHERE conversation_id = ?1", CONV_META_SELECT_SQL),
                params![conversation_id],
                row_to_conversation_meta,
            )
            .optional()
            .map_err(|e| LcmError::Store(format!("Failed to query conversation meta: {e}")))?;

        if let Some(meta) = existing {
            return Ok(meta);
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let meta = ConversationMeta::new(conversation_id, now_ms);
        let metadata_json = serde_json::to_string(&meta.metadata).unwrap_or_default();
        conn.execute(
            CONV_META_INSERT_SQL,
            params![
                meta.conversation_id,
                meta.title,
                meta.title_source,
                meta.created_at_unix_ms,
                meta.updated_at_unix_ms,
                meta.message_count,
                meta.conversation_type,
                metadata_json,
                meta.version,
                meta.last_message_preview,
            ],
        )
        .map_err(|e| LcmError::Store(format!("Failed to insert conversation meta: {e}")))?;

        Ok(meta)
    }

    /// Get conversation metadata from the LCM store.
    pub fn get_conversation_meta(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ConversationMeta>, LcmError> {
        let conn = self.lock_conn()?;

        conn.query_row(
            &format!("{} WHERE conversation_id = ?1", CONV_META_SELECT_SQL),
            params![conversation_id],
            row_to_conversation_meta,
        )
        .optional()
        .map_err(|e| LcmError::Store(format!("Failed to query conversation meta: {e}")))
    }

    /// Update conversation metadata fields.
    /// Currently only used in tests.
    #[allow(dead_code)]
    pub fn update_conversation_meta(
        &self,
        conversation_id: &str,
        title: Option<&str>,
        title_source: Option<&str>,
        message_count_delta: Option<i32>,
        metadata: Option<&BTreeMap<String, Value>>,
    ) -> Result<(), LcmError> {
        let conn = self.lock_conn()?;

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        // Build dynamic UPDATE.
        let mut sets: Vec<String> = vec!["updated_at_unix_ms = ?".to_string()];
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(now_ms)];

        if let Some(t) = title {
            sets.push("title = ?".to_string());
            param_values.push(Box::new(t.to_string()));
        }
        if let Some(ts) = title_source {
            sets.push("title_source = ?".to_string());
            param_values.push(Box::new(normalize_title_source(ts)));
        }
        if let Some(delta) = message_count_delta {
            sets.push("message_count = MAX(0, message_count + ?)".to_string());
            param_values.push(Box::new(delta));
        }
        if let Some(m) = metadata {
            sets.push("metadata_json = ?".to_string());
            let json_str = serde_json::to_string(m).unwrap_or_default();
            param_values.push(Box::new(json_str));
        }

        let sql = format!(
            "UPDATE conversation_meta SET {} WHERE conversation_id = ?",
            sets.join(", ")
        );
        param_values.push(Box::new(conversation_id.to_string()));

        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();

        conn.execute(&sql, params_ref.as_slice())
            .map_err(|e| LcmError::Store(format!("Failed to update conversation meta: {e}")))?;

        Ok(())
    }

    // ── Maintenance ────────────────────────────────────────────────────

    /// Remove all data for a conversation from the LCM store.
    #[allow(dead_code)]
    pub fn delete_conversation(&self, conversation_id: &str) -> Result<(), LcmError> {
        let conn = self.lock_conn()?;

        let tx = conn
            .unchecked_transaction()
            .map_err(|e| LcmError::Store(format!("Failed to begin transaction: {e}")))?;

        // Delete summary children and parents first (foreign keys), then summaries,
        // then messages, then file refs.
        tx.execute(
            "DELETE FROM summary_children WHERE summary_id IN (SELECT id FROM summaries WHERE conversation_id = ?1)",
            params![conversation_id],
        )
        .map_err(|e| LcmError::Store(format!("Failed to delete summary children: {e}")))?;

        tx.execute(
            "DELETE FROM summary_parents WHERE summary_id IN (SELECT id FROM summaries WHERE conversation_id = ?1)",
            params![conversation_id],
        )
        .map_err(|e| LcmError::Store(format!("Failed to delete summary parents: {e}")))?;

        tx.execute(
            "DELETE FROM summaries WHERE conversation_id = ?1",
            params![conversation_id],
        )
        .map_err(|e| LcmError::Store(format!("Failed to delete summaries: {e}")))?;

        tx.execute(
            "DELETE FROM messages WHERE conversation_id = ?1",
            params![conversation_id],
        )
        .map_err(|e| LcmError::Store(format!("Failed to delete messages: {e}")))?;

        tx.execute(
            "DELETE FROM file_refs WHERE conversation_id = ?1",
            params![conversation_id],
        )
        .map_err(|e| LcmError::Store(format!("Failed to delete file refs: {e}")))?;

        tx.execute(
            "DELETE FROM reasoning_chains WHERE conversation_id = ?1",
            params![conversation_id],
        )
        .map_err(|e| LcmError::Store(format!("Failed to delete reasoning chains: {e}")))?;

        tx.execute(
            "DELETE FROM conversation_meta WHERE conversation_id = ?1",
            params![conversation_id],
        )
        .map_err(|e| LcmError::Store(format!("Failed to delete conversation meta: {e}")))?;

        tx.commit()
            .map_err(|e| LcmError::Store(format!("Failed to commit deletion: {e}")))?;

        Ok(())
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Full schema DDL for the LCM store.
const CREATE_SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'tool')),
    content TEXT NOT NULL DEFAULT '',
    token_count INTEGER NOT NULL DEFAULT 0,
    timestamp_unix_ms INTEGER NOT NULL,
    covered_by TEXT,
    seq INTEGER NOT NULL DEFAULT 0,
    hop_index INTEGER NOT NULL DEFAULT 0,
    thinking TEXT,
    search_text TEXT NOT NULL DEFAULT '',
    metadata_json TEXT NOT NULL DEFAULT '{}',
    file_refs_json TEXT NOT NULL DEFAULT '[]',
    FOREIGN KEY (covered_by) REFERENCES summaries(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_messages_conv_seq
    ON messages(conversation_id, seq);

CREATE INDEX IF NOT EXISTS idx_messages_covered
    ON messages(covered_by);

CREATE TABLE IF NOT EXISTS summaries (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('leaf', 'condensed')),
    text TEXT NOT NULL,
    token_count INTEGER NOT NULL DEFAULT 0,
    created_at_unix_ms INTEGER NOT NULL,
    compaction_level INTEGER NOT NULL DEFAULT 1 CHECK(compaction_level BETWEEN 1 AND 3)
);

CREATE INDEX IF NOT EXISTS idx_summaries_conv
    ON summaries(conversation_id);

CREATE TABLE IF NOT EXISTS summary_children (
    summary_id TEXT NOT NULL,
    child_type TEXT NOT NULL CHECK(child_type IN ('message', 'summary')),
    child_id TEXT NOT NULL,
    PRIMARY KEY (summary_id, child_type, child_id),
    FOREIGN KEY (summary_id) REFERENCES summaries(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_summary_children_child
    ON summary_children(child_type, child_id);

-- Parent back-references for upward DAG traversal.
CREATE TABLE IF NOT EXISTS summary_parents (
    summary_id TEXT NOT NULL,
    parent_id TEXT NOT NULL,
    PRIMARY KEY (summary_id, parent_id),
    FOREIGN KEY (summary_id) REFERENCES summaries(id) ON DELETE CASCADE,
    FOREIGN KEY (parent_id) REFERENCES summaries(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_summary_parents_parent
    ON summary_parents(parent_id);

CREATE TABLE IF NOT EXISTS file_refs (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    path TEXT NOT NULL,
    mime_type TEXT NOT NULL DEFAULT 'application/octet-stream',
    token_count INTEGER NOT NULL DEFAULT 0,
    exploration_summary TEXT NOT NULL DEFAULT '',
    registered_at_unix_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_file_refs_conv
    ON file_refs(conversation_id);

-- Conversation metadata — replaces the legacy metadata.json approach.
CREATE TABLE IF NOT EXISTS conversation_meta (
    conversation_id TEXT PRIMARY KEY,
    title TEXT NOT NULL DEFAULT '',
    title_source TEXT NOT NULL DEFAULT 'pending',
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    message_count INTEGER NOT NULL DEFAULT 0,
    conversation_type TEXT NOT NULL DEFAULT 'standard',
    metadata_json TEXT NOT NULL DEFAULT '{}',
    version INTEGER NOT NULL DEFAULT 0,
    last_message_preview TEXT NOT NULL DEFAULT ''
);

CREATE INDEX IF NOT EXISTS idx_conv_meta_updated
    ON conversation_meta(updated_at_unix_ms);

-- FTS5 virtual table for full-text search.
-- Uses content= to keep FTS in sync with the messages table.
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    search_text,
    content='messages',
    content_rowid='rowid'
);

-- Triggers to keep FTS index in sync with messages table.
CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, search_text) VALUES (new.rowid, new.search_text);
END;

CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, search_text) VALUES('delete', old.rowid, old.search_text);
END;

CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, search_text) VALUES('delete', old.rowid, old.search_text);
    INSERT INTO messages_fts(rowid, search_text) VALUES (new.rowid, new.search_text);
END;
";

/// Schema migrations for existing databases that predate the unified
/// `ConversationMeta` (version, last_message_preview columns).
const SCHEMA_MIGRATIONS: &[&str] = &[
    "ALTER TABLE conversation_meta ADD COLUMN version INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE conversation_meta ADD COLUMN last_message_preview TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE messages ADD COLUMN thinking TEXT",
    "ALTER TABLE messages ADD COLUMN seq INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE messages ADD COLUMN hop_index INTEGER NOT NULL DEFAULT 0",
];

fn parse_role(s: &str) -> Result<MessageRole, rusqlite::Error> {
    match s {
        "user" => Ok(MessageRole::User),
        "assistant" => Ok(MessageRole::Assistant),
        "tool" => Ok(MessageRole::Tool),
        other => Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Unknown role: {other}"),
            )),
        )),
    }
}

fn summary_kind_str(kind: SummaryKind) -> &'static str {
    match kind {
        SummaryKind::Leaf => "leaf",
        SummaryKind::Condensed => "condensed",
    }
}

fn parse_summary_kind(s: &str) -> Result<SummaryKind, rusqlite::Error> {
    match s {
        "leaf" => Ok(SummaryKind::Leaf),
        "condensed" => Ok(SummaryKind::Condensed),
        other => Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Unknown summary kind: {other}"),
            )),
        )),
    }
}

/// Build an FTS5 query string from a user-provided pattern.
///
/// Escapes special FTS5 characters and wraps the pattern appropriately.
fn build_fts_query(pattern: &str) -> String {
    // FTS5 special characters: must be enclosed in double quotes to be
    // treated as literals, or escaped. We use a simple quoted approach.
    let cleaned = pattern.replace('"', "\"\"").replace('\'', "''");

    // If the pattern looks like a regex with special chars, do a simple
    // substring match. Otherwise, do a token-based match.
    if cleaned
        .chars()
        .any(|c| c == '*' || c == '?' || c == '.' || c == '|')
    {
        // Contains regex-like chars — use LIKE-compatible search.
        // FTS5 doesn't natively support regex; we fall back to simple
        // token matching.
        format!("\"{}\"", cleaned)
    } else {
        // Simple word-based search.
        cleaned
    }
}

// ── Row Deserialization Helpers ─────────────────────────────────────────

/// Convert a SQLite row to a `StoredMessage`.
///
/// Expects columns in `MESSAGE_SELECT_SQL` order (indices 0-9).
/// Convert a SQLite row to a `StoredMessage`.
///
/// For `MESSAGE_SELECT_SQL` queries, columns are:
///   0=id, 1=conversation_id, 2=role, 3=content, 4=token_count,
///   5=timestamp_unix_ms, 6=covered_by, 7=seq, 8=hop_index,
///   9=thinking (if thinking_idx=Some(9)), 10=metadata_json, 11=file_refs_json.
///
/// `thinking_idx` specifies the column index for thinking content
/// (Some(9) for standard SELECT, None when thinking is absent in FTS queries).
/// seq/hop_index default to 0 for callers (like FTS) that don't select them.
fn row_to_stored_message(
    row: &rusqlite::Row,
    thinking_idx: Option<usize>,
    meta_idx: usize,
    refs_idx: usize,
) -> Result<StoredMessage, rusqlite::Error> {
    let covered_by: Option<String> = row.get(6)?;
    let thinking: Option<String> = match thinking_idx {
        Some(idx) => row.get(idx)?,
        None => None,
    };
    let metadata_json: String = row.get::<_, String>(meta_idx).unwrap_or_default();
    let file_refs_json: String = row.get::<_, String>(refs_idx).unwrap_or_default();
    let metadata: BTreeMap<String, Value> =
        serde_json::from_str(&metadata_json).unwrap_or_default();
    let file_refs: Vec<FileRefId> = serde_json::from_str(&file_refs_json).unwrap_or_default();
    // seq is at index 7, hop_index at index 8 when using MESSAGE_SELECT_SQL.
    // For FTS queries these columns aren't selected, so default to 0.
    let seq: u32 = row.get(7).unwrap_or(0);
    let hop_index: u32 = row.get(8).unwrap_or(0);
    Ok(StoredMessage {
        id: MessageId::from(row.get::<_, String>(0)?),
        conversation_id: row.get(1)?,
        role: parse_role(row.get::<_, String>(2)?.as_str())?,
        content: row.get(3)?,
        token_count: row.get(4)?,
        timestamp_unix_ms: row.get(5)?,
        covered_by: covered_by.map(SummaryId::from),
        thinking,
        seq,
        hop_index,
        metadata,
        file_refs,
    })
}

/// Convert a SQLite row to a `ConversationMeta`.
///
/// Expects columns in `CONV_META_SELECT_SQL` order (indices 0-9).
fn row_to_conversation_meta(row: &rusqlite::Row) -> Result<ConversationMeta, rusqlite::Error> {
    let metadata_json: String = row.get::<_, String>(7).unwrap_or_default();
    let title_source: String = row.get::<_, String>(2).unwrap_or_default();
    Ok(ConversationMeta {
        conversation_id: row.get(0)?,
        title: row.get(1)?,
        title_source: normalize_title_source(&title_source),
        created_at_unix_ms: row.get(3)?,
        updated_at_unix_ms: row.get(4)?,
        message_count: row.get(5)?,
        conversation_type: row.get(6)?,
        metadata: serde_json::from_str(&metadata_json).unwrap_or_default(),
        version: row.get::<_, i32>(8).unwrap_or(0) as u32,
        last_message_preview: row.get::<_, String>(9).unwrap_or_default(),
    })
}

// ── SQL Column Constants ───────────────────────────────────────────

/// INSERT into messages with all 13 columns.
const MESSAGE_INSERT_SQL: &str =
    "INSERT OR IGNORE INTO messages \
     (id, conversation_id, role, content, token_count, timestamp_unix_ms, \
      covered_by, seq, hop_index, thinking, search_text, metadata_json, file_refs_json) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)";

/// SELECT columns (0-10) from messages — caller must append WHERE clause.
const MESSAGE_SELECT_SQL: &str =
    "SELECT id, conversation_id, role, content, token_count, timestamp_unix_ms, \
            covered_by, seq, hop_index, thinking, metadata_json, file_refs_json \
     FROM messages";

/// SELECT all columns from conversation_meta — caller must append WHERE clause.
const CONV_META_SELECT_SQL: &str =
    "SELECT conversation_id, title, title_source, created_at_unix_ms, \
            updated_at_unix_ms, message_count, conversation_type, \
            metadata_json, version, last_message_preview \
     FROM conversation_meta";

/// INSERT into conversation_meta with all 10 columns.
const CONV_META_INSERT_SQL: &str =
    "INSERT OR IGNORE INTO conversation_meta \
     (conversation_id, title, title_source, created_at_unix_ms, \
      updated_at_unix_ms, message_count, conversation_type, \
      metadata_json, version, last_message_preview) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)";

// ── Trait for optional usage ────────────────────────────────────────────────

/// Helper trait to convert `Result<T, E>` to `Option<T>` when E is not needed.
trait OptionalResult {
    type Output;
    fn optional(self) -> Result<Option<Self::Output>, rusqlite::Error>;
}

impl<T> OptionalResult for Result<T, rusqlite::Error> {
    type Output = T;
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(val) => Ok(Some(val)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
