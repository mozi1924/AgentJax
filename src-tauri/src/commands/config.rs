use crate::config::{self, ConfigInfo};

#[tauri::command]
pub fn get_runtime_config() -> Result<ConfigInfo, String> {
  config::get_config_info()
}

#[tauri::command]
pub fn get_config_file_path() -> Result<String, String> {
  let path = config::init_config_if_missing()?;
  Ok(path.display().to_string())
}
