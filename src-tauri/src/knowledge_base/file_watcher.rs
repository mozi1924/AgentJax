//! File system watcher for knowledge base auto-sync.
//!
//! Watches configured KB directories for markdown file changes and
//! automatically triggers incremental re-indexing. Uses `notify` for
//! cross-platform file system events with a debounce to avoid
//! excessive re-indexing during rapid saves.
//!
//! ## Integration
//!
//! Wire into `lib.rs::run()` by creating a `KbFileWatcher` after config
//! loads and calling `start_watching()` for each configured KB. Stop
//! all watchers on `tauri::RunEvent::Exit`.

#![allow(dead_code)]

use crate::config::AppConfig;
use crate::error::AgentJaxResult;
use crate::knowledge_base::manager::KnowledgeBaseManager;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Manages file system watchers for knowledge base auto-sync.
pub struct KbFileWatcher {
    /// Active watchers, keyed by KB ID.
    watchers: RwLock<HashMap<String, ActiveWatch>>,
    /// Debounce duration in milliseconds.
    debounce_ms: u64,
}

struct ActiveWatch {
    /// The notify watcher handle.
    _watcher: RecommendedWatcher,
    /// Channel to signal shutdown.
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
}

impl KbFileWatcher {
    /// Create a new file watcher with the given debounce duration.
    pub fn new(debounce_ms: u64) -> Self {
        Self {
            watchers: RwLock::new(HashMap::new()),
            debounce_ms,
        }
    }

    /// Start watching a single KB directory.
    ///
    /// When `.md` files are created, modified, or removed, the watcher
    /// triggers incremental re-indexing after a debounce period.
    pub async fn start_watching(
        &self,
        kb_id: &str,
        kb_path: PathBuf,
        kb_manager: Arc<KnowledgeBaseManager>,
        app_config: Arc<AppConfig>,
    ) -> AgentJaxResult<()> {
        // Stop any existing watcher for this KB.
        self.stop_watching(kb_id).await;

        let (shutdown_tx, _shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let debounce_ms = self.debounce_ms;
        let kb_id_owned = kb_id.to_string();

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                match res {
                    Ok(event) => {
                        // Only care about .md file changes.
                        let has_md_change = event.paths.iter().any(|p| {
                            p.extension()
                                .map(|ext| ext == "md")
                                .unwrap_or(false)
                        });
                        if !has_md_change {
                            return;
                        }
                        // Only react to create/modify/remove events.
                        let is_relevant = matches!(
                            event.kind,
                            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                        );
                        if !is_relevant {
                            return;
                        }
                        // Spawn a re-index task with debounce.
                        let kb_id = kb_id_owned.clone();
                        let mgr = kb_manager.clone();
                        let cfg = app_config.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(
                                std::time::Duration::from_millis(debounce_ms),
                            )
                            .await;
                            // Re-index all affected .md files.
                            for path in &event.paths {
                                if path
                                    .extension()
                                    .map(|ext| ext == "md")
                                    .unwrap_or(false)
                                {
                                    let doc_id = path
                                        .file_stem()
                                        .map(|s| s.to_string_lossy().to_string())
                                        .unwrap_or_else(|| "doc".to_string());
                                    match tokio::fs::read_to_string(path).await {
                                        Ok(content) => {
                                            let doc = crate::rag::types::Document {
                                                id: doc_id,
                                                content,
                                                metadata: std::collections::BTreeMap::new(),
                                            };
                                            if let Err(e) = mgr
                                                .reindex_document(&kb_id, doc, &cfg)
                                                .await
                                            {
                                                log::warn!(
                                                    "File watcher: failed to re-index '{}': {}",
                                                    path.display(),
                                                    e
                                                );
                                            } else {
                                                log::info!(
                                                    "File watcher: re-indexed '{}' in KB '{}'",
                                                    path.display(),
                                                    kb_id
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            log::warn!(
                                                "File watcher: failed to read '{}': {}",
                                                path.display(),
                                                e
                                            );
                                        }
                                    }
                                }
                            }
                        });
                    }
                    Err(e) => {
                        log::warn!("File watcher error: {}", e);
                    }
                }
            },
            Config::default(),
        )
        .map_err(|e| {
            crate::error::AgentJaxError::config(format!(
                "Failed to create file watcher: {}",
                e
            ))
        })?;

        watcher
            .watch(&kb_path, RecursiveMode::Recursive)
            .map_err(|e| {
                crate::error::AgentJaxError::config(format!(
                    "Failed to watch path '{}': {}",
                    kb_path.display(),
                    e
                ))
            })?;

        let active = ActiveWatch {
            _watcher: watcher,
            shutdown_tx,
        };

        self.watchers
            .write()
            .await
            .insert(kb_id.to_string(), active);

        log::info!(
            "File watcher started for KB '{}' at '{}'",
            kb_id,
            kb_path.display()
        );

        Ok(())
    }

    /// Stop watching a specific KB.
    pub async fn stop_watching(&self, kb_id: &str) {
        if let Some(active) = self.watchers.write().await.remove(kb_id) {
            let _ = active.shutdown_tx.send(());
            log::info!("File watcher stopped for KB '{}'", kb_id);
        }
    }

    /// Stop all active watchers.
    pub async fn stop_all(&self) {
        let ids: Vec<String> = self.watchers.read().await.keys().cloned().collect();
        for id in ids {
            self.stop_watching(&id).await;
        }
    }
}
