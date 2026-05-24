use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const APP_CONFIG_DIR_NAME: &str = "AgentJax";
const CONFIG_FILE_NAME: &str = "config.yaml";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
  pub base_url: String,
  pub websocket_url: Option<String>,
  pub transport: String,
  pub api_key: Option<String>,
  pub store: bool,
  pub instructions: String,
  pub default_model: String,
  pub available_models: Vec<String>,
  pub request_timeout_seconds: u64,
}

impl Default for AppConfig {
  fn default() -> Self {
    Self {
      base_url: "https://api.openai.com/v1".to_string(),
      websocket_url: None,
      transport: "websocket".to_string(),
      api_key: None,
      store: false,
      instructions: "You are Codex, a helpful AI assistant. Follow the user's instructions.".to_string(),
      default_model: "gpt-5-mini".to_string(),
      available_models: vec!["gpt-5-mini".to_string(), "gpt-5".to_string()],
      request_timeout_seconds: 120,
    }
  }
}

impl AppConfig {
  pub fn normalize(mut self) -> Self {
    self.base_url = self.base_url.trim().trim_end_matches('/').to_string();
    if self.base_url.is_empty() {
      self.base_url = "https://api.openai.com/v1".to_string();
    }

    self.transport = self.transport.trim().to_lowercase();
    if self.transport != "websocket" && self.transport != "sse" {
      self.transport = "websocket".to_string();
    }

    self.websocket_url = self
      .websocket_url
      .as_deref()
      .map(str::trim)
      .filter(|s| !s.is_empty())
      .map(|s| s.trim_end_matches('/').to_string());

    self.instructions = self.instructions.trim().to_string();
    if self.instructions.is_empty() {
      self.instructions =
        "You are Codex, a helpful AI assistant. Follow the user's instructions.".to_string();
    }

    if self.default_model.trim().is_empty() {
      self.default_model = "gpt-5-mini".to_string();
    }

    if self.available_models.is_empty() {
      self.available_models.push(self.default_model.clone());
    }

    if !self.available_models.iter().any(|m| m == &self.default_model) {
      self.available_models.insert(0, self.default_model.clone());
    }

    if self.request_timeout_seconds == 0 {
      self.request_timeout_seconds = 120;
    }

    self
  }

  pub fn resolved_api_key(&self) -> Option<String> {
    let from_config = self
      .api_key
      .as_deref()
      .map(str::trim)
      .filter(|s| !s.is_empty())
      .map(ToOwned::to_owned);

    from_config.or_else(|| {
      std::env::var("OPENAI_API_KEY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    })
  }

  pub fn resolve_model(&self, requested: Option<&str>) -> String {
    requested
      .map(str::trim)
      .filter(|s| !s.is_empty())
      .map(ToOwned::to_owned)
      .unwrap_or_else(|| self.default_model.clone())
  }

  pub fn resolved_websocket_url(&self) -> String {
    if let Some(url) = &self.websocket_url {
      return url.clone();
    }

    if self.base_url.starts_with("https://") {
      return format!("wss://{}", self.base_url.trim_start_matches("https://"));
    }
    if self.base_url.starts_with("http://") {
      return format!("ws://{}", self.base_url.trim_start_matches("http://"));
    }

    format!("wss://{}", self.base_url)
  }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigInfo {
  pub config_path: String,
  pub base_url: String,
  pub websocket_url: String,
  pub transport: String,
  pub store: bool,
  pub instructions: String,
  pub default_model: String,
  pub available_models: Vec<String>,
  pub has_api_key: bool,
  pub request_timeout_seconds: u64,
}

pub fn config_dir_path() -> Result<PathBuf, String> {
  let base = dirs::config_dir()
    .ok_or_else(|| "Unable to locate OS config directory".to_string())?;
  Ok(base.join(APP_CONFIG_DIR_NAME))
}

pub fn init_config_if_missing() -> Result<PathBuf, String> {
  let dir = config_dir_path()?;
  if !dir.exists() {
    fs::create_dir_all(&dir)
      .map_err(|e| format!("Failed to create config directory {}: {e}", dir.display()))?;
  }

  let path = dir.join(CONFIG_FILE_NAME);
  if !path.exists() {
    let template = default_config_yaml();
    fs::write(&path, template)
      .map_err(|e| format!("Failed to create config file {}: {e}", path.display()))?;
  }

  Ok(path)
}

pub fn load_config() -> Result<AppConfig, String> {
  let path = init_config_if_missing()?;
  let raw = fs::read_to_string(&path)
    .map_err(|e| format!("Failed to read config file {}: {e}", path.display()))?;

  let parsed: AppConfig = serde_yaml::from_str(&raw)
    .map_err(|e| format!("Invalid YAML in {}: {e}", path.display()))?;

  Ok(parsed.normalize())
}

pub fn get_config_info() -> Result<ConfigInfo, String> {
  let path = init_config_if_missing()?;
  let config = load_config()?;

  Ok(ConfigInfo {
    config_path: path.display().to_string(),
    base_url: config.base_url.clone(),
    websocket_url: config.resolved_websocket_url(),
    transport: config.transport.clone(),
    store: config.store,
    instructions: config.instructions.clone(),
    default_model: config.default_model.clone(),
    available_models: config.available_models.clone(),
    has_api_key: config.resolved_api_key().is_some(),
    request_timeout_seconds: config.request_timeout_seconds,
  })
}

fn default_config_yaml() -> String {
  [
    "# AgentJax configuration",
    "# macOS: ~/Library/Application Support/AgentJax/config.yaml",
    "# Linux: ~/.config/AgentJax/config.yaml",
    "# Windows: %APPDATA%\\AgentJax\\config.yaml",
    "",
    "base_url: \"https://api.openai.com/v1\"",
    "websocket_url: \"\"",
    "transport: \"websocket\"",
    "api_key: \"\"",
    "store: false",
    "instructions: \"You are Codex, a helpful AI assistant. Follow the user's instructions.\"",
    "default_model: \"gpt-5-mini\"",
    "available_models:",
    "  - \"gpt-5-mini\"",
    "  - \"gpt-5\"",
    "request_timeout_seconds: 120",
    "",
  ]
  .join("\n")
}
