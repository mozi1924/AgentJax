use tauri::Manager;

#[tauri::command]
pub fn open_devtools(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(webview) = app.get_webview_window("main") {
        webview.open_devtools();
        Ok(())
    } else {
        Err("Main webview window not found".to_string())
    }
}
