pub fn chrono_like_now_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    ts.to_string()
}

pub fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub async fn run_blocking<T, F>(task: F) -> Result<T, crate::error::AgentJaxError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, crate::error::AgentJaxError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|err| crate::error::AgentJaxError::internal(format!("Background task join error: {err}")))?
}
