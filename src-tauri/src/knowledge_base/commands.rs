//! Tauri IPC commands for knowledge base management.
//!
//! These commands provide the frontend settings UI with knowledge base
//! status information, path scanning, and indexing control.
//! CRUD operations (add/delete KBs) are handled by the settings patch system.

use crate::config::{load_active_config, KbPathType};
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::knowledge_base::manager::KnowledgeBaseManager;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use tauri::Emitter;

/// A progress event emitted during knowledge base indexing.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeIndexingProgress {
    pub kb_id: String,
    /// Current phase: "chunking" | "fts" | "embedding" | "done"
    pub phase: String,
    pub processed: usize,
    pub total: usize,
    pub current_file: String,
    pub chunks_created: usize,
    pub done: bool,
    pub error: Option<String>,
}

/// A scan result describing a single markdown file found at a path.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScannedFile {
    /// The absolute path to the file.
    pub path: String,
    /// The file name.
    pub name: String,
    /// File size in bytes.
    pub size: u64,
}

/// Status information for a single knowledge base.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeBaseStatus {
    /// KB ID (matches the key in rag.knowledge_bases).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// The configured path on disk.
    pub path: String,
    /// Number of indexed documents.
    pub document_count: usize,
    /// Total chunks in the index.
    pub chunk_count: usize,
    /// Whether the KB directories exist on disk (has been indexed before).
    pub indexed: bool,
}

/// List all configured knowledge bases with their index status.
///
/// Reads the KB definitions from the global config and reports
/// the current index status for each one.
#[tauri::command]
pub async fn list_knowledge_bases() -> Result<Vec<KnowledgeBaseStatus>, AgentJaxError> {
    let full_config = load_active_config()?;
    let app_config = &full_config.shared;

    let kb_manager = KnowledgeBaseManager::from_config(app_config)?;
    let mut statuses = Vec::new();

    for (kb_id, entry) in &app_config.rag.knowledge_bases {
        let kb_dir = kb_manager.root_dir().join(kb_id);
        let indexed = kb_dir.join("vectors").exists() && kb_dir.join("fts.db").exists();

        let (document_count, chunk_count) = if indexed {
            match kb_manager.open_kb(kb_id).await {
                Ok(kb) => {
                    let docs = kb.fts_store.list_documents().unwrap_or_default();
                    let doc_count = docs.len();
                    let chunk_count = kb.fts_store.total_chunk_count().unwrap_or(0);
                    (doc_count, chunk_count)
                }
                Err(_) => (0, 0),
            }
        } else {
            (0, 0)
        };

        statuses.push(KnowledgeBaseStatus {
            id: kb_id.clone(),
            name: entry.name.clone(),
            path: entry.path.clone(),
            document_count,
            chunk_count,
            indexed,
        });
    }

    Ok(statuses)
}

/// Arguments for scanning a path.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanPathRequest {
    /// The absolute path to scan (file or directory).
    pub path: String,
}

/// Scan a path for markdown files.
///
/// If the path points to a single file, returns just that file.
/// If it points to a directory, recursively finds all `.md` files.
#[tauri::command]
pub async fn scan_knowledge_base_path(
    req: ScanPathRequest,
) -> Result<Vec<ScannedFile>, AgentJaxError> {
    let path = req.path;
    let path = if let Some(rest) = path.strip_prefix('~') {
        let home = dirs::home_dir().ok_or_else(|| {
            AgentJaxError::config("Could not resolve home directory".to_string())
        })?;
        home.join(rest.strip_prefix('/').unwrap_or(rest))
    } else {
        std::path::PathBuf::from(&path)
    };

    if !path.exists() {
        return Err(AgentJaxError::config(format!(
            "Path does not exist: {}",
            path.display()
        )));
    }

    let mut files = Vec::new();

    if path.is_file() {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let size = std::fs::metadata(&path)
            .map(|m| m.len())
            .unwrap_or(0);
        files.push(ScannedFile {
            path: path.to_string_lossy().to_string(),
            name,
            size,
        });
    } else if path.is_dir() {
        scan_markdown_files(&path, &mut files)?;
    }

    Ok(files)
}

/// Recursively scan a directory for markdown files.
fn scan_markdown_files(dir: &Path, files: &mut Vec<ScannedFile>) -> AgentJaxResult<()> {
    if !dir.is_dir() {
        return Ok(());
    }

    let entries = std::fs::read_dir(dir).map_err(|e| {
        AgentJaxError::config(format!("Failed to read directory '{}': {e}", dir.display()))
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            AgentJaxError::config(format!("Failed to read directory entry: {e}"))
        })?;
        let entry_path = entry.path();

        if entry_path.is_dir() {
            // Skip hidden directories
            if entry_path
                .file_name()
                .map(|n| n.to_string_lossy().starts_with('.'))
                .unwrap_or(false)
            {
                continue;
            }
            scan_markdown_files(&entry_path, files)?;
        } else if entry_path.is_file()
            && entry_path
                .extension()
                .map(|ext| ext == "md")
                .unwrap_or(false)
            {
                let name = entry_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let size = std::fs::metadata(&entry_path)
                    .map(|m| m.len())
                    .unwrap_or(0);
                files.push(ScannedFile {
                    path: entry_path.to_string_lossy().to_string(),
                    name,
                    size,
                });
            }
    }

    Ok(())
}

/// Refresh / re-index a knowledge base.
///
/// Scans the configured path for markdown files and re-indexes all documents.
/// Emits `kb_indexing_progress` events to the frontend for real-time progress.
///
/// ## Concurrency
///
/// Only one indexing operation per KB is allowed at a time. Calling this
/// while the same KB is already indexing returns an error. Different KBs
/// can be indexed in parallel.
///
/// ## Cancellation
///
/// Use `cancel_kb_indexing` to stop an in-progress indexing operation.
/// Cancellation is also triggered automatically when a KB is deleted.
#[tauri::command]
pub async fn refresh_knowledge_base(
    app_handle: tauri::AppHandle,
    kb_id: String,
) -> Result<Value, AgentJaxError> {
    // ── Acquire per-KB indexing lock ──────────────────────────────────
    let token = crate::knowledge_base::indexing::acquire(&kb_id).map_err(|msg| {
        AgentJaxError::config(msg)
    })?;

    // Ensure the lock is released on exit (success, error, or panic).
    let _guard = KbIndexingGuard { kb_id: kb_id.clone() };

    // ── Helper: check cancellation ────────────────────────────────────
    let check_cancel = || -> AgentJaxResult<()> {
        if token.is_cancelled() {
            Err(AgentJaxError::config(format!(
                "Indexing of '{}' was cancelled",
                kb_id
            )))
        } else {
            Ok(())
        }
    };

    let full_config = load_active_config()?;
    let app_config = Arc::new(full_config.shared.clone());

    let kb_manager = KnowledgeBaseManager::from_config(&app_config)?;

    let entry = app_config
        .rag
        .knowledge_bases
        .get(&kb_id)
        .ok_or_else(|| {
            AgentJaxError::config(format!("Knowledge base '{kb_id}' not found in config"))
        })?;

    let resolved_path = if let Some(rest) = entry.path.strip_prefix('~') {
        let home = dirs::home_dir().ok_or_else(|| {
            AgentJaxError::config("Could not resolve home directory".to_string())
        })?;
        home.join(rest.strip_prefix('/').unwrap_or(rest))
    } else {
        std::path::PathBuf::from(&entry.path)
    };

    if !resolved_path.exists() {
        return Err(AgentJaxError::config(format!(
            "Knowledge base path does not exist: {}",
            resolved_path.display()
        )));
    }

    let emit_progress = |phase: &str,
                         processed: usize,
                         total: usize,
                         current_file: &str,
                         chunks: usize,
                         done: bool,
                         error: Option<String>| {
        let _ = app_handle.emit(
            "kb_indexing_progress",
            KnowledgeIndexingProgress {
                kb_id: kb_id.clone(),
                phase: phase.to_string(),
                processed,
                total,
                current_file: current_file.to_string(),
                chunks_created: chunks,
                done,
                error,
            },
        );
    };

    // Index all markdown files
    let mut total_docs = 0;
    let mut total_chunks = 0;

    let result = match entry.path_type {
        KbPathType::File => {
            let file_name = resolved_path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "doc".to_string());

            emit_progress("embedding", 0, 1, &file_name, 0, false, None);

            let content = tokio::fs::read_to_string(&resolved_path).await.map_err(|e| {
                AgentJaxError::config(format!("Failed to read file '{}': {e}", resolved_path.display()))
            })?;

            check_cancel()?;

            let _modified_at = std::fs::metadata(&resolved_path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            let document = crate::rag::types::Document {
                id: file_name,
                content,
                metadata: std::collections::BTreeMap::new(),
            };

            match kb_manager.index_document(&kb_id, document, &app_config).await {
                Ok(progress) => {
                    total_docs += 1;
                    total_chunks += progress.chunks_created;
                    Ok(())
                }
                Err(e) => {
                    log::error!("Failed to index single-file KB '{}': {e}", kb_id);
                    Err(e)
                }
            }
        }
        KbPathType::Folder => {
            // ═══════════════════════════════════════════════════════════
            // Phase 1: Scanning
            // ═══════════════════════════════════════════════════════════
            let mut md_files: Vec<std::path::PathBuf> = Vec::new();
            scan_markdown_files_for_indexing(&resolved_path, &mut md_files)?;
            let total_files = md_files.len();

            if total_files == 0 {
                emit_progress("done", 0, 0, "", 0, true, None);
                return Ok(serde_json::json!({
                    "kbId": kb_id,
                    "totalDocuments": 0,
                    "totalChunks": 0,
                }));
            }

            emit_progress("scanning", 0, total_files, "", 0, false, None);

            let mut documents: Vec<(String, String, i64)> = Vec::with_capacity(total_files);
            for (idx, file_path) in md_files.iter().enumerate() {
                let file_name = file_path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "doc".to_string());

                emit_progress("scanning", idx, total_files, &file_name, 0, false, None);

                match tokio::fs::read_to_string(file_path).await {
                    Ok(content) => {
                        let modified_at = std::fs::metadata(file_path)
                            .ok()
                            .and_then(|m| m.modified().ok())
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        documents.push((file_name, content, modified_at));
                    }
                    Err(e) => {
                        log::warn!("Failed to read '{}': {e}", file_path.display());
                        emit_progress(
                            "scanning",
                            idx + 1,
                            total_files,
                            "",
                            0,
                            false,
                            Some(format!("Failed to read '{}': {e}", file_path.display())),
                        );
                    }
                }
            }

            total_docs = documents.len();
            check_cancel()?;

            // ═══════════════════════════════════════════════════════════
            // Phase 2: Chunking
            // ═══════════════════════════════════════════════════════════
            let app_handle3 = app_handle.clone();
            let kb_id3 = kb_id.clone();
            let total_docs_chunk = total_docs;
            let token_chunk = token.clone();

            let prepared_chunks = kb_manager
                .prepare_kb(&kb_id, &documents, move |idx, total, doc_id| {
                    // Check cancellation during chunking (coarse-grained,
                    // checked between documents by prepare_kb).
                    if token_chunk.is_cancelled() {
                        return;
                    }
                    let _ = app_handle3.emit(
                        "kb_indexing_progress",
                        KnowledgeIndexingProgress {
                            kb_id: kb_id3.clone(),
                            phase: "chunking".to_string(),
                            processed: idx,
                            total,
                            current_file: doc_id.to_string(),
                            chunks_created: 0,
                            done: false,
                            error: None,
                        },
                    );
                })
                .await?;

            check_cancel()?;

            emit_progress(
                "chunking",
                total_docs_chunk,
                total_docs_chunk,
                "",
                prepared_chunks,
                false,
                None,
            );

            log::info!(
                "KB '{}' chunking complete: {} docs → {} chunks",
                kb_id, total_docs, prepared_chunks
            );

            // ═══════════════════════════════════════════════════════════
            // Phase 3: Embedding
            // ═══════════════════════════════════════════════════════════
            let unembedded = kb_manager.unembedded_chunk_count(&kb_id).await.unwrap_or(0);
            if unembedded > 0 && !kb_manager.is_embedding_disabled() {
                let app_handle2 = app_handle.clone();
                let kb_id2 = kb_id.clone();
                let total_docs_embed = total_docs;
                let token_embed = token.clone();

                match kb_manager
                    .embed_prepared_chunks(
                        &kb_id,
                        &app_config,
                        move |processed, total| {
                            if token_embed.is_cancelled() {
                                return;
                            }
                            let _ = app_handle2.emit(
                                "kb_indexing_progress",
                                KnowledgeIndexingProgress {
                                    kb_id: kb_id2.clone(),
                                    phase: "embedding".to_string(),
                                    processed,
                                    total,
                                    current_file: String::new(),
                                    chunks_created: total_docs_embed,
                                    done: false,
                                    error: None,
                                },
                            );
                        },
                    )
                    .await
                {
                    Ok(embedded_count) => {
                        total_chunks = embedded_count;
                        Ok(())
                    }
                    Err(e) => {
                        log::error!(
                            "KB '{}' embedding phase failed: {}. FTS index is available, vector index may be incomplete.",
                            kb_id, e
                        );
                        Err(e)
                    }
                }
            } else {
                total_chunks = prepared_chunks;
                Ok(())
            }
        }
    };

    match result {
        Ok(()) => {
            emit_progress("done", total_docs, total_docs, "", total_chunks, true, None);
            Ok(serde_json::json!({
                "kbId": kb_id,
                "totalDocuments": total_docs,
                "totalChunks": total_chunks,
            }))
        }
        Err(e) => {
            let is_cancelled = token.is_cancelled();
            emit_progress(
                "done",
                total_docs,
                total_docs,
                "",
                total_chunks,
                true,
                Some(if is_cancelled {
                    "Indexing cancelled".to_string()
                } else {
                    e.to_string()
                }),
            );
            if is_cancelled {
                Ok(serde_json::json!({
                    "kbId": kb_id,
                    "totalDocuments": total_docs,
                    "totalChunks": total_chunks,
                    "cancelled": true,
                }))
            } else {
                Err(e)
            }
        }
    }
}

/// RAII guard that releases the per-KB indexing lock on drop.
struct KbIndexingGuard {
    kb_id: String,
}

impl Drop for KbIndexingGuard {
    fn drop(&mut self) {
        crate::knowledge_base::indexing::release(&self.kb_id);
    }
}

/// Cancel an in-progress indexing operation for a knowledge base.
///
/// If no indexing is in progress for the given KB, this is a no-op.
/// Called by the frontend when the user clicks "Cancel" or when a KB
/// is deleted while indexing.
#[tauri::command]
pub async fn cancel_kb_indexing(kb_id: String) -> Result<bool, AgentJaxError> {
    Ok(crate::knowledge_base::indexing::cancel(&kb_id))
}

/// List KBs currently being indexed (for UI status display).
#[tauri::command]
pub async fn list_active_indexing_kb_ids() -> Result<Vec<String>, AgentJaxError> {
    Ok(crate::knowledge_base::indexing::active_indexing_kb_ids())
}

/// Helper to recursively find markdown files for indexing.
fn scan_markdown_files_for_indexing(dir: &Path, files: &mut Vec<std::path::PathBuf>) -> AgentJaxResult<()> {
    if !dir.is_dir() {
        return Ok(());
    }

    let entries = std::fs::read_dir(dir).map_err(|e| {
        AgentJaxError::config(format!("Failed to read directory '{}': {e}", dir.display()))
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            AgentJaxError::config(format!("Failed to read directory entry: {e}"))
        })?;
        let entry_path = entry.path();

        if entry_path.is_dir() {
            if entry_path
                .file_name()
                .map(|n| n.to_string_lossy().starts_with('.'))
                .unwrap_or(false)
            {
                continue;
            }
            scan_markdown_files_for_indexing(&entry_path, files)?;
        } else if entry_path.is_file()
            && entry_path
                .extension()
                .map(|ext| ext == "md")
                .unwrap_or(false)
            {
                files.push(entry_path);
            }
    }

    Ok(())
}
