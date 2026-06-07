//! Tauri IPC commands for knowledge base management.
//!
//! These commands provide the frontend settings UI with knowledge base
//! status information, path scanning, and indexing control.
//! CRUD operations (add/delete KBs) are handled by the settings patch system.

use crate::config::{load_active_config, KbPathType};
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::rag::KnowledgeBaseManager;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tauri::Emitter;

/// A progress event emitted during knowledge base indexing.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeIndexingProgress {
    pub kb_id: String,
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
    let _agent_config = &full_config.agent;

    let kb_manager = KnowledgeBaseManager::from_config(app_config, _agent_config)?;
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
        } else if entry_path.is_file() {
            if entry_path
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
    }

    Ok(())
}

/// Refresh / re-index a knowledge base.
///
/// Scans the configured path for markdown files and re-indexes all documents.
/// Emits `kb_indexing_progress` events to the frontend for real-time progress.
/// This is a long-running operation for large KBs.
#[tauri::command]
pub async fn refresh_knowledge_base(
    app_handle: tauri::AppHandle,
    kb_id: String,
) -> Result<Value, AgentJaxError> {
    let full_config = load_active_config()?;
    let app_config = Arc::new(full_config.shared.clone());
    let agent_config = Arc::new(full_config.agent.clone());

    let kb_manager = KnowledgeBaseManager::from_config(&app_config, &agent_config)?;

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

    let emit_progress = |processed: usize,
                         total: usize,
                         current_file: &str,
                         chunks_created: usize,
                         done: bool,
                         error: Option<String>| {
        let _ = app_handle.emit(
            "kb_indexing_progress",
            KnowledgeIndexingProgress {
                kb_id: kb_id.clone(),
                processed,
                total,
                current_file: current_file.to_string(),
                chunks_created,
                done,
                error,
            },
        );
    };

    // Index all markdown files
    let mut total_docs = 0;
    let mut total_chunks = 0;

    match entry.path_type {
        KbPathType::File => {
            let file_name = resolved_path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "doc".to_string());

            emit_progress(0, 1, &file_name, 0, false, None);

            let content = tokio::fs::read_to_string(&resolved_path).await.map_err(|e| {
                AgentJaxError::config(format!("Failed to read file '{}': {e}", resolved_path.display()))
            })?;

            let document = crate::rag::types::Document {
                id: file_name,
                content,
                metadata: std::collections::BTreeMap::new(),
            };

            match kb_manager.index_document(&kb_id, document, &app_config).await {
                Ok(progress) => {
                    total_docs += 1;
                    total_chunks += progress.chunks_created;
                    emit_progress(1, 1, "", total_chunks, true, None);
                }
                Err(e) => {
                    emit_progress(0, 1, "", 0, true, Some(e.to_string()));
                    return Err(e);
                }
            }
        }
        KbPathType::Folder => {
            let mut md_files = Vec::new();
            scan_markdown_files_for_indexing(&resolved_path, &mut md_files)?;
            let total = md_files.len();

            if total == 0 {
                emit_progress(0, 0, "", 0, true, None);
                return Ok(serde_json::json!({
                    "kbId": kb_id,
                    "totalDocuments": 0,
                    "totalChunks": 0,
                }));
            }

            // ── Concurrent indexing with bounded parallelism ──────────
            // Process up to 8 documents in parallel to keep the embedding
            // API saturated without overwhelming network/disk resources.
            const MAX_CONCURRENT: usize = 8;
            let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT));
            let kb_manager = Arc::new(kb_manager);
            let processed = Arc::new(AtomicUsize::new(0));
            let total_chunks_atomic = Arc::new(AtomicUsize::new(0));
            let mut handles = futures_util::stream::FuturesUnordered::new();

            for file_path in md_files {
                let kb_id = kb_id.clone();
                let kb_manager = Arc::clone(&kb_manager);
                let app_handle = app_handle.clone();
                let app_config = Arc::clone(&app_config);
                let sem = Arc::clone(&semaphore);
                let processed = Arc::clone(&processed);
                let total_chunks = Arc::clone(&total_chunks_atomic);

                handles.push(tokio::spawn(async move {
                    let _permit = sem.acquire().await;
                    let file_name = file_path
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "doc".to_string());

                    let content = match tokio::fs::read_to_string(&file_path).await {
                        Ok(c) => c,
                        Err(e) => {
                            let p = processed.fetch_add(1, Ordering::SeqCst);
                            let tc = total_chunks.load(Ordering::SeqCst);
                            let _ = app_handle.emit(
                                "kb_indexing_progress",
                                KnowledgeIndexingProgress {
                                    kb_id: kb_id.clone(),
                                    processed: p + 1,
                                    total,
                                    current_file: file_name,
                                    chunks_created: tc,
                                    done: false,
                                    error: Some(format!(
                                        "Failed to read '{}': {e}",
                                        file_path.display()
                                    )),
                                },
                            );
                            return (0, 0);
                        }
                    };

                    let document = crate::rag::types::Document {
                        id: file_name.clone(),
                        content,
                        metadata: std::collections::BTreeMap::new(),
                    };

                    match kb_manager
                        .index_document(&kb_id, document, &app_config)
                        .await
                    {
                        Ok(progress) => {
                            let p = processed.fetch_add(1, Ordering::SeqCst);
                            let tc = total_chunks.fetch_add(
                                progress.chunks_created,
                                Ordering::SeqCst,
                            );
                            let _ = app_handle.emit(
                                "kb_indexing_progress",
                                KnowledgeIndexingProgress {
                                    kb_id: kb_id.clone(),
                                    processed: p + 1,
                                    total,
                                    current_file: file_name,
                                    chunks_created: tc + progress.chunks_created,
                                    done: false,
                                    error: None,
                                },
                            );
                            (1, progress.chunks_created)
                        }
                        Err(e) => {
                            log::warn!(
                                "Failed to index '{}': {e}",
                                file_path.display()
                            );
                            let p = processed.fetch_add(1, Ordering::SeqCst);
                            let tc = total_chunks.load(Ordering::SeqCst);
                            let _ = app_handle.emit(
                                "kb_indexing_progress",
                                KnowledgeIndexingProgress {
                                    kb_id: kb_id.clone(),
                                    processed: p + 1,
                                    total,
                                    current_file: file_name,
                                    chunks_created: tc,
                                    done: false,
                                    error: Some(format!(
                                        "Failed to index '{}': {e}",
                                        file_path.display()
                                    )),
                                },
                            );
                            (0, 0)
                        }
                    }
                }));
            }

            // Collect results
            use futures_util::StreamExt;
            while let Some(result) = handles.next().await {
                if let Ok((docs, chunks)) = result {
                    total_docs += docs;
                    total_chunks += chunks;
                }
            }

            emit_progress(total, total, "", total_chunks, true, None);
        }
    }

    Ok(serde_json::json!({
        "kbId": kb_id,
        "totalDocuments": total_docs,
        "totalChunks": total_chunks,
    }))
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
        } else if entry_path.is_file() {
            if entry_path
                .extension()
                .map(|ext| ext == "md")
                .unwrap_or(false)
            {
                files.push(entry_path);
            }
        }
    }

    Ok(())
}
