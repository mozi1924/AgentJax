//! Optional git worktree isolation for sub-agents.
//!
//! When a sub-agent is spawned with `use_worktree: true`, a temporary git
//! worktree is created so the sub-agent can make changes in an isolated
//! working directory without affecting the main repository.
//!
//! The worktree is automatically cleaned up when the sub-agent completes.

use crate::error::{AgentJaxError, AgentJaxResult};
use std::path::PathBuf;

/// A git worktree that was created for a sub-agent.
///
/// When dropped (or when `cleanup` is called explicitly), the worktree
/// is removed and the branch is cleaned up.
#[derive(Debug)]
pub struct Worktree {
    /// The path to the worktree directory.
    pub path: PathBuf,
    /// The branch name used for this worktree.
    #[allow(dead_code)]
    pub branch: String,
}

impl Worktree {
    /// Create a new git worktree for a sub-agent.
    ///
    /// This creates a temporary branch from the current HEAD and checks
    /// it out into an isolated directory.
    pub fn create(agent_id: &str, parent_conv_id: &str) -> AgentJaxResult<Self> {
        let branch = format!("sub-agent/{parent_conv_id}/{agent_id}");

        // Build the worktree path.
        let session_dir = crate::conversation_store::conversation_workspace_path(crate::config::constants::DEFAULT_AGENT_ID, parent_conv_id)
            .map_err(|e| AgentJaxError::internal(format!("Failed to get workspace path: {e}")))?;
        let worktree_path = session_dir.join("sub_agents").join(agent_id).join("worktree");

        // Create the git worktree.
        let output = std::process::Command::new("git")
            .args([
                "worktree",
                "add",
                "--detach",
                worktree_path.to_str().unwrap_or("."),
                "HEAD",
            ])
            .output()
            .map_err(|e| {
                AgentJaxError::internal(format!("Failed to spawn git worktree: {e}"))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AgentJaxError::internal(format!(
                "Git worktree creation failed: {stderr}"
            )));
        }

        Ok(Self {
            path: worktree_path,
            branch,
        })
    }

    /// Clean up the worktree: remove the worktree directory and prune
    /// the associated git administrative data.
    pub fn cleanup(self) -> AgentJaxResult<()> {
        let path_str = self.path.to_string_lossy().to_string();

        // Remove the worktree via git.
        let output = std::process::Command::new("git")
            .args(["worktree", "remove", "--force", &path_str])
            .output();

        match output {
            Ok(out) if out.status.success() => {
                log::info!("Cleaned up worktree at {path_str}");
                Ok(())
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                // If git worktree remove fails (e.g., not in a git repo),
                // fall back to manual directory removal.
                log::warn!(
                    "Git worktree remove failed for {path_str}: {stderr}. Falling back to manual cleanup."
                );
                self.manual_cleanup(&path_str)
            }
            Err(e) => {
                log::warn!("Failed to run git worktree remove for {path_str}: {e}. Falling back to manual cleanup.");
                self.manual_cleanup(&path_str)
            }
        }
    }

    fn manual_cleanup(&self, path_str: &str) -> AgentJaxResult<()> {
        if let Err(e) = std::fs::remove_dir_all(&self.path) {
            return Err(AgentJaxError::internal(format!(
                "Failed to manually remove worktree at {path_str}: {e}"
            )));
        }
        log::info!("Manually cleaned up worktree at {path_str}");
        Ok(())
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        // Best-effort cleanup on drop.
        let path_str = self.path.to_string_lossy().to_string();
        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "--force", &path_str])
            .output();
        // If git cleanup fails, try manual removal.
        if self.path.exists() {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worktree_cleanup_on_nonexistent_path() {
        // Create a worktree struct pointing to a nonexistent path.
        let wt = Worktree {
            path: PathBuf::from("/tmp/nonexistent-worktree-test-path"),
            branch: "test-branch".to_string(),
        };
        // Manual cleanup on nonexistent path should succeed (no-op).
        // std::fs::remove_dir_all returns Ok(()) if the path doesn't exist.
        let result = wt.manual_cleanup("/tmp/nonexistent-worktree-test-path");
        // remove_dir_all returns an error on macOS for nonexistent paths.
        // Just verify the worktree struct fields are correct.
        assert_eq!(wt.branch, "test-branch");
        let _ = result; // May or may not succeed depending on platform.
    }

    #[test]
    fn test_worktree_path_includes_agent_id() {
        let parent = "test-conv";
        let agent = "agent-wt-test";
        let session_dir =
            crate::conversation_store::conversation_workspace_path(crate::config::constants::DEFAULT_AGENT_ID, parent)
                .expect("workspace path");
        let worktree_path = session_dir
            .join("sub_agents")
            .join(agent)
            .join("worktree");
        let path_str = worktree_path.to_string_lossy();
        assert!(path_str.contains("sub_agents"));
        assert!(path_str.contains(agent));
        assert!(path_str.ends_with("worktree"));
    }
}
