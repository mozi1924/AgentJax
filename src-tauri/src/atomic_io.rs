//! Atomic file I/O utilities.
//!
//! Provides a write-rename atomicity pattern: content is first written to a
//! temporary file in the same directory, then atomically renamed to the target
//! path. This ensures the target file is never left in a partially-written
//! state, even after a crash or power loss.

use crate::error::{AgentJaxError, AgentJaxResult};
use std::fs;
use std::io::Write;
use std::path::Path;

/// Atomically write `contents` to `path` using a temp-file + rename strategy.
///
/// 1. Creates a temporary file next to `path` with a unique name.
/// 2. Writes all `contents` to the temp file and calls `sync_all`.
/// 3. Renames the temp file over `path` (atomic on most filesystems).
///
/// The `label` parameter is used in error messages to identify the file
/// type (e.g., "messages", "metadata", "config").
///
/// The caller **must** ensure that the parent directory of `path` exists
/// before calling this function.
pub fn write_file_atomically(
    path: &Path,
    contents: &[u8],
    label: &str,
) -> AgentJaxResult<()> {
    let (tmp_path, mut tmp_file) = create_temp_file(path, label)?;
    tmp_file.write_all(contents).map_err(|e| {
        AgentJaxError::internal(format!(
            "Failed to write temporary {label} file {}: {e}",
            tmp_path.display()
        ))
        .with_error_source(&e)
    })?;
    tmp_file.sync_all().map_err(|e| {
        AgentJaxError::internal(format!(
            "Failed to sync temporary {label} file {}: {e}",
            tmp_path.display()
        ))
        .with_error_source(&e)
    })?;
    drop(tmp_file);

    finalize_atomic_replace(&tmp_path, path, label)
}

/// Create a temporary file in the same directory as `path`.
///
/// The temp file name has the form `.{filename}.tmp-{pid}-{nanos}`.
pub(crate) fn create_temp_file(
    path: &Path,
    label: &str,
) -> AgentJaxResult<(std::path::PathBuf, fs::File)> {
    let parent = path.parent().ok_or_else(|| {
        AgentJaxError::internal(format!(
            "Failed to resolve parent directory for {label} file {}",
            path.display()
        ))
    })?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            AgentJaxError::internal(format!("Invalid {label} file name {}", path.display()))
        })?;
    let unique_suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let tmp_path = parent.join(format!(
        ".{file_name}.tmp-{}-{unique_suffix}",
        std::process::id()
    ));

    let tmp_file = fs::File::create(&tmp_path).map_err(|e| {
        AgentJaxError::internal(format!(
            "Failed to create temporary {label} file {}: {e}",
            tmp_path.display()
        ))
        .with_error_source(&e)
    })?;

    Ok((tmp_path, tmp_file))
}

/// Atomically rename `tmp_path` over `path`, with a Windows-specific retry.
pub(crate) fn finalize_atomic_replace(
    tmp_path: &Path,
    path: &Path,
    label: &str,
) -> AgentJaxResult<()> {
    if let Err(rename_err) = fs::rename(tmp_path, path) {
        #[cfg(target_os = "windows")]
        {
            if path.exists() {
                fs::remove_file(path).map_err(|e| {
                    AgentJaxError::internal(format!(
                        "Failed to replace existing {label} file {} after rename error {}: {e}",
                        path.display(),
                        rename_err
                    ))
                })?;
                fs::rename(tmp_path, path).map_err(|e| {
                    AgentJaxError::internal(format!(
                        "Failed to finalize {label} file {} after rename retry: {e}",
                        path.display()
                    ))
                })?;
            } else {
                return Err(AgentJaxError::internal(format!(
                    "Failed to atomically rename temporary {label} file {} to {}: {rename_err}",
                    tmp_path.display(),
                    path.display()
                )));
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = fs::remove_file(tmp_path);
            return Err(AgentJaxError::internal(format!(
                "Failed to atomically rename temporary {label} file {} to {}: {rename_err}",
                tmp_path.display(),
                path.display()
            )));
        }
    }

    Ok(())
}
