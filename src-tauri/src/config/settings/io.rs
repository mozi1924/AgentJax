use crate::error::{AgentJaxError, AgentJaxResult};
use std::fs;
use std::path::Path;

/// Atomically write content to a config file using the shared atomic I/O utility.
///
/// Delegates to [`crate::atomic_io::write_file_atomically`] for the actual
/// temp-file + rename strategy, which includes `sync_all` and Windows retry
/// logic that the previous inline implementation lacked.
pub(super) fn atomic_write(path: &Path, content: &str) -> AgentJaxResult<()> {
    // Ensure parent directory exists (write_file_atomically requires it).
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent).map_err(|e| {
            AgentJaxError::config(format!(
                "Failed to create config directory {}: {e}",
                parent.display()
            ))
            .with_error_source(&e)
        })?;
    }

    crate::atomic_io::write_file_atomically(path, content.as_bytes(), "config")?;
    Ok(())
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
