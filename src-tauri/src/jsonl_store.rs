//! Generic JSONL file I/O utilities.
//!
//! Provides shared helpers for reading, appending, and writing JSONL
//! (JSON Lines) files. Used by `street/persist.rs`, `street/tasks.rs`,
//! and `conversation_store/file_io.rs` to eliminate duplicated JSONL
//! handling (~150 lines of duplicate code).
//!
//! # Design
//!
//! All functions take a `label` parameter for descriptive error messages
//! (e.g. `"notification"`, `"task"`, `"messages"`). All errors are
//! returned as `AgentJaxError::internal` with a descriptive message.

use crate::error::{AgentJaxError, AgentJaxResult};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;

// ── Directory Helpers ────────────────────────────────────────────────────────

/// Ensure the parent directory of `path` exists, creating it if necessary.
///
/// `label` is used in error messages (e.g. `"notification"`, `"tasks"`).
pub fn ensure_parent_dir(path: &Path, label: &str) -> AgentJaxResult<()> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| {
                AgentJaxError::internal(format!(
                    "Failed to create {label} directory {}: {e}",
                    parent.display()
                ))
            })?;
        }
    }
    Ok(())
}

// ── Reading ──────────────────────────────────────────────────────────────────

/// Read all lines from a JSONL file, skipping empty and malformed lines.
///
/// Returns an empty vec if the file does not exist. Malformed lines are
/// logged as warnings and skipped.
pub fn read_jsonl<T: DeserializeOwned>(path: &Path, label: &str) -> AgentJaxResult<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(path).map_err(|e| {
        AgentJaxError::internal(format!(
            "Failed to read {label} file {}: {e}",
            path.display()
        ))
    })?;

    let mut items = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<T>(line) {
            Ok(item) => items.push(item),
            Err(e) => {
                log::warn!("Failed to parse {label} line in {}: {e}", path.display());
            }
        }
    }

    Ok(items)
}

/// Deduplicate a vec by a key function: last occurrence wins, first-occurrence
/// insertion order is preserved.
///
/// The key function must return an **owned** value (not a reference into `T`),
/// since items may be rearranged by deduplication.
///
/// This is useful when loading JSONL data where later entries with the same key
/// should override earlier ones (e.g. status updates for notifications).
pub fn dedup_vec<T, F, K>(items: Vec<T>, key_fn: F) -> Vec<T>
where
    F: Fn(&T) -> K,
    K: std::hash::Hash + Eq + Clone,
{
    // First pass: find the last occurrence index for each key.
    let mut last_pos: HashMap<K, usize> = HashMap::new();
    for (i, item) in items.iter().enumerate() {
        last_pos.insert(key_fn(item).clone(), i);
    }

    // Second pass: collect items at last-occurrence positions,
    // preserving original insertion order.
    let mut result: Vec<T> = Vec::with_capacity(last_pos.len());
    for (i, item) in items.into_iter().enumerate() {
        if last_pos.values().any(|&pos| pos == i) {
            result.push(item);
        }
    }
    result
}

// ── Writing ──────────────────────────────────────────────────────────────────

/// Append a single serialized line to a JSONL file.
///
/// Creates the file and parent directory if they don't exist, then
/// appends the JSON-serialized item followed by a newline. Calls
/// `sync_data()` to flush the write to disk.
pub fn append_line<T: Serialize>(path: &Path, item: &T, label: &str) -> AgentJaxResult<()> {
    ensure_parent_dir(path, label)?;

    let line = serde_json::to_string(item).map_err(|e| {
        AgentJaxError::internal(format!("Failed to serialize {label}: {e}"))
    })?;

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| {
            AgentJaxError::internal(format!(
                "Failed to open {label} file {} for append: {e}",
                path.display()
            ))
        })?;

    writeln!(file, "{line}").map_err(|e| {
        AgentJaxError::internal(format!(
            "Failed to write to {label} file {}: {e}",
            path.display()
        ))
    })?;

    file.sync_data().map_err(|e| {
        AgentJaxError::internal(format!(
            "Failed to sync {label} file {}: {e}",
            path.display()
        ))
    })?;

    Ok(())
}

/// Write all items as JSONL, replacing the file entirely.
///
/// If `items` is empty, the file is removed (if it exists) instead of
/// writing an empty file.
pub fn write_lines<T: Serialize>(path: &Path, items: &[T], label: &str) -> AgentJaxResult<()> {
    if items.is_empty() {
        remove_file(path, label)?;
        return Ok(());
    }

    ensure_parent_dir(path, label)?;

    let mut lines: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        match serde_json::to_string(item) {
            Ok(line) => lines.push(line),
            Err(e) => {
                log::warn!("Failed to serialize {label} for write: {e}");
            }
        }
    }

    fs::write(path, lines.join("\n")).map_err(|e| {
        AgentJaxError::internal(format!(
            "Failed to write {label} file {}: {e}",
            path.display()
        ))
    })?;

    Ok(())
}

// ── File Management ──────────────────────────────────────────────────────────

/// Remove a file if it exists. Does nothing if the file does not exist.
pub fn remove_file(path: &Path, label: &str) -> AgentJaxResult<()> {
    if path.exists() {
        fs::remove_file(path).map_err(|e| {
            AgentJaxError::internal(format!(
                "Failed to remove {label} file {}: {e}",
                path.display()
            ))
        })?;
    }
    Ok(())
}


