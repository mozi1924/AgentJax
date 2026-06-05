#[tauri::command]
pub fn open_devtools(window: tauri::WebviewWindow) -> Result<(), String> {
    let config = crate::config::load_config()?;
    let agent = crate::config::load_agent_config(&config.active_agent_id)
        .unwrap_or_default()
        .normalize();
    if !agent.enable_developer_tools {
        return Err("Developer tools are disabled in settings".to_string());
    }

    // Open the frontend inspector for the invoking window so custom labels and
    // future secondary windows do not depend on a hard-coded "main" lookup.
    if !window.is_devtools_open() {
        window.open_devtools();
    }
    Ok(())
}
