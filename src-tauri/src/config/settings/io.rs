use crate::error::{AgentJaxError, AgentJaxResult};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn atomic_write(path: &Path, content: &str) -> AgentJaxResult<()> {
    let temp_path = temp_config_path(path);
    fs::write(&temp_path, content).map_err(|e| {
        AgentJaxError::config(format!(
            "Failed to write temporary config file {}: {e}",
            temp_path.display()
        ))
        .with_error_source(&e)
    })?;
    fs::rename(&temp_path, path).map_err(|e| {
        AgentJaxError::config(format!(
            "Failed to replace config file {} with {}: {e}",
            path.display(),
            temp_path.display()
        ))
        .with_error_source(&e)
    })?;
    Ok(())
}

fn temp_config_path(path: &Path) -> PathBuf {
    let mut temp = path.to_path_buf();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.yaml");
    temp.set_file_name(format!("{}.tmp", file_name));
    temp
}

pub(super) fn compute_revision(raw: &str) -> String {
    // Stable, non-cryptographic revision for optimistic concurrency checks.
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in raw.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}
