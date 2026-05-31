use std::path::PathBuf;

pub const AGENTJAX_HOME_ENV: &str = "AGENTJAX_HOME";
const AGENTJAX_DIR_NAME: &str = ".agentjax";
const PLUGINS_DIR_NAME: &str = "plugins";

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

pub fn plugins_dir() -> Result<PathBuf, String> {
    Ok(agentjax_home_dir()?.join(PLUGINS_DIR_NAME))
}

pub fn ensure_plugins_dir() -> Result<PathBuf, String> {
    let dir = plugins_dir()?;
    std::fs::create_dir_all(&dir).map_err(|err| {
        format!(
            "Failed to create plugins directory {}: {err}",
            dir.display()
        )
    })?;
    Ok(dir)
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

    #[test]
    fn defaults_to_home_dot_agentjax_when_env_missing() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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

    #[test]
    fn plugins_dir_lives_under_agentjax_home() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let expected = PathBuf::from("/tmp/agentjax-plugin-home");
        unsafe {
            std::env::set_var(AGENTJAX_HOME_ENV, expected.as_os_str());
        }

        let plugins_dir = plugins_dir().expect("resolve plugins dir");
        assert_eq!(plugins_dir, expected.join("plugins"));
        unsafe {
            std::env::remove_var(AGENTJAX_HOME_ENV);
        }
    }
}
