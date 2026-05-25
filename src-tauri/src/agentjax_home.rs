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
