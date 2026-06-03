use serde::{Deserialize, Serialize};

/// Sandbox policy for plugin code.
///
/// The defaults intentionally start narrow so plugin execution only gains
/// access to capabilities that the host explicitly opts into later.
///
/// Since plugins do not directly access system resources (they return JSON
/// specs to the host), sandbox enforcement happens at the **host boundary**:
/// before the host sends an HTTP request, reads/writes a file, or spawns a
/// process on behalf of a plugin, it must check the plugin's sandbox policy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
#[derive(Default)]
pub struct SandboxPolicy {
    #[serde(default)]
    pub allow_file_read: bool,
    #[serde(default)]
    pub allow_file_write: bool,
    #[serde(default)]
    pub allow_network: bool,
    #[serde(default)]
    pub allow_process_spawn: bool,
    #[serde(default)]
    pub allow_env_read: bool,
    pub max_memory_mb: Option<u64>,
    pub max_execution_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_hosts: Vec<String>,
}


/// Errors returned when a sandbox policy check fails.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Reserved for future use
#[allow(clippy::enum_variant_names)]
pub enum SandboxViolation {
    NetworkNotAllowed,
    HostNotAllowed(String),
    FileReadNotAllowed,
    FileWriteNotAllowed,
    ProcessSpawnNotAllowed,
    EnvReadNotAllowed,
}

impl SandboxViolation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NetworkNotAllowed => "plugin does not have network access permission",
            Self::HostNotAllowed(_host) => {
                // Static string won't work here, but callers use .to_string()
                "plugin does not have access to the requested host"
            }
            Self::FileReadNotAllowed => "plugin does not have file read permission",
            Self::FileWriteNotAllowed => "plugin does not have file write permission",
            Self::ProcessSpawnNotAllowed => "plugin does not have process spawn permission",
            Self::EnvReadNotAllowed => "plugin does not have environment variable read permission",
        }
    }
}

impl std::fmt::Display for SandboxViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HostNotAllowed(host) => {
                write!(
                    f,
                    "plugin does not have access to the requested host '{host}'"
                )
            }
            _ => write!(f, "{}", self.as_str()),
        }
    }
}

#[allow(dead_code)] // Reserved for future use
impl SandboxPolicy {
    /// Check whether the policy allows network access.
    /// Returns `Ok(())` if allowed, or `Err(SandboxViolation)` if denied.
    pub fn check_network(&self, host: Option<&str>) -> Result<(), SandboxViolation> {
        if !self.allow_network {
            return Err(SandboxViolation::NetworkNotAllowed);
        }
        if let Some(host) = host
            && !self.allowed_hosts.is_empty()
                && !self.allowed_hosts.iter().any(|allowed| host_matches(allowed, host))
            {
                return Err(SandboxViolation::HostNotAllowed(host.to_string()));
            }
        Ok(())
    }

    /// Check whether the policy allows file read access.
    pub fn check_file_read(&self) -> Result<(), SandboxViolation> {
        if self.allow_file_read {
            Ok(())
        } else {
            Err(SandboxViolation::FileReadNotAllowed)
        }
    }

    /// Check whether the policy allows file write access.
    pub fn check_file_write(&self) -> Result<(), SandboxViolation> {
        if self.allow_file_write {
            Ok(())
        } else {
            Err(SandboxViolation::FileWriteNotAllowed)
        }
    }

    /// Check whether the policy allows process spawning.
    pub fn check_process_spawn(&self) -> Result<(), SandboxViolation> {
        if self.allow_process_spawn {
            Ok(())
        } else {
            Err(SandboxViolation::ProcessSpawnNotAllowed)
        }
    }

    /// Check whether the policy allows reading environment variables.
    pub fn check_env_read(&self) -> Result<(), SandboxViolation> {
        if self.allow_env_read {
            Ok(())
        } else {
            Err(SandboxViolation::EnvReadNotAllowed)
        }
    }
}

/// Simple host matching: supports exact match and wildcard prefix (`*.example.com`).
///
/// `*.example.com` matches `api.example.com` and `sub.api.example.com`,
/// but NOT `example.com` (must have at least one subdomain level).
#[allow(dead_code)] // Reserved for future use
fn host_matches(pattern: &str, host: &str) -> bool {
    let pattern = pattern.trim();
    let host = host.trim();
    if pattern == host {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        // Require a dot before the suffix so `*.example.com` doesn't match bare `example.com`.
        let dotted = format!(".{suffix}");
        host.ends_with(&dotted)
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_denies_everything() {
        let p = SandboxPolicy::default();
        assert!(p.check_network(None).is_err());
        assert!(p.check_file_read().is_err());
        assert!(p.check_file_write().is_err());
        assert!(p.check_process_spawn().is_err());
        assert!(p.check_env_read().is_err());
    }

    #[test]
    fn network_allowed_without_host_restriction() {
        let p = SandboxPolicy {
            allow_network: true,
            ..Default::default()
        };
        assert!(p.check_network(Some("api.openai.com")).is_ok());
        assert!(p.check_network(Some("example.com")).is_ok());
    }

    #[test]
    fn network_allowed_with_host_restriction() {
        let p = SandboxPolicy {
            allow_network: true,
            allowed_hosts: vec!["api.openai.com".to_string(), "*.anthropic.com".to_string()],
            ..Default::default()
        };
        assert!(p.check_network(Some("api.openai.com")).is_ok());
        assert!(p.check_network(Some("api.anthropic.com")).is_ok());
        assert!(p.check_network(Some("example.com")).is_err());
    }

    #[test]
    fn wildcard_host_matching() {
        assert!(host_matches("*.example.com", "api.example.com"));
        assert!(host_matches("*.example.com", "sub.api.example.com"));
        assert!(!host_matches("*.example.com", "example.com"));
        assert!(host_matches("api.example.com", "api.example.com"));
        assert!(!host_matches("api.example.com", "other.example.com"));
    }
}
