use crate::error::AgentJaxError;

#[tauri::command]
pub fn open_devtools(window: tauri::WebviewWindow) -> Result<(), AgentJaxError> {
    let full = crate::config::load_active_config()?;
    if !full.agent.enable_developer_tools {
        return Err(AgentJaxError::config("Developer tools are disabled in settings"));
    }

    // Open the frontend inspector for the invoking window so custom labels and
    // future secondary windows do not depend on a hard-coded "main" lookup.
    if !window.is_devtools_open() {
        window.open_devtools();
    }
    Ok(())
}
