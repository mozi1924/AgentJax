use std::path::PathBuf;

pub const AGENTJAX_HOME_ENV: &str = "AGENTJAX_HOME";
const AGENTJAX_DIR_NAME: &str = ".agentjax";

pub fn agentjax_home_dir() -> Result<PathBuf, String> {
    let configured = std::env::var(AGENTJAX_HOME_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    if let Some(raw) = configured {
        return expand_home_prefix(&raw);
    }

    let home = dirs::home_dir()
        .ok_or_else(|| "Failed to resolve home directory for AgentJax".to_string())?;
    Ok(home.join(AGENTJAX_DIR_NAME))
}

fn expand_home_prefix(value: &str) -> Result<PathBuf, String> {
    if value == "~" {
        let home = dirs::home_dir()
            .ok_or_else(|| "Failed to resolve home directory for AGENTJAX_HOME".to_string())?;
        return Ok(home);
    }

    if let Some(remainder) = value.strip_prefix("~/") {
        let home = dirs::home_dir()
            .ok_or_else(|| "Failed to resolve home directory for AGENTJAX_HOME".to_string())?;
        return Ok(home.join(remainder));
    }

    Ok(PathBuf::from(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn defaults_to_home_dot_agentjax_when_env_missing() {
        let _guard = env_lock().lock().expect("lock AGENTJAX_HOME env");
        unsafe {
            std::env::remove_var(AGENTJAX_HOME_ENV);
        }

        let resolved = agentjax_home_dir().expect("resolve default agentjax home");
        let home = dirs::home_dir().expect("resolve user home");
        assert_eq!(resolved, home.join(".agentjax"));
        unsafe {
            std::env::remove_var(AGENTJAX_HOME_ENV);
        }
    }

    #[test]
    fn respects_env_override_absolute_path() {
        let _guard = env_lock().lock().expect("lock AGENTJAX_HOME env");
        let expected = std::env::temp_dir().join(format!("agentjax-home-{}", uuid::Uuid::new_v4()));
        unsafe {
            std::env::set_var(AGENTJAX_HOME_ENV, expected.as_os_str());
        }

        let resolved = agentjax_home_dir().expect("resolve AGENTJAX_HOME override");
        assert_eq!(resolved, expected);
        unsafe {
            std::env::remove_var(AGENTJAX_HOME_ENV);
        }
    }

    #[test]
    fn expands_tilde_prefix_from_env() {
        let _guard = env_lock().lock().expect("lock AGENTJAX_HOME env");
        unsafe {
            std::env::set_var(AGENTJAX_HOME_ENV, "~/agentjax-custom-home");
        }

        let resolved = agentjax_home_dir().expect("resolve AGENTJAX_HOME tilde path");
        let home = dirs::home_dir().expect("resolve user home");
        assert_eq!(resolved, home.join("agentjax-custom-home"));
        unsafe {
            std::env::remove_var(AGENTJAX_HOME_ENV);
        }
    }

    #[test]
    fn config_dir_matches_agentjax_home() {
        let _guard = env_lock().lock().expect("lock AGENTJAX_HOME env");
        let expected = PathBuf::from("/tmp/agentjax-config-home");
        unsafe {
            std::env::set_var(AGENTJAX_HOME_ENV, expected.as_os_str());
        }

        let config_dir = crate::config::config_dir_path().expect("resolve config dir");
        assert_eq!(config_dir, expected);
        unsafe {
            std::env::remove_var(AGENTJAX_HOME_ENV);
        }
    }
}
