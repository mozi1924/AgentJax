use crate::conversation_store_utils::now_unix_ms;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::Notify;
use uuid::Uuid;

const DEFAULT_WAIT_TIMEOUT_MS: u64 = 30_000;
const MAX_WAIT_TIMEOUT_MS: u64 = 120_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackgroundJobStatus {
    InProgress,
    Completed,
    Failed,
}

impl BackgroundJobStatus {
    fn as_str(self) -> &'static str {
        match self {
            BackgroundJobStatus::InProgress => "in_progress",
            BackgroundJobStatus::Completed => "completed",
            BackgroundJobStatus::Failed => "failed",
        }
    }
}

#[derive(Debug)]
struct BackgroundJobState {
    status: BackgroundJobStatus,
    completed_at_unix_ms: Option<i64>,
    duration_ms: Option<u64>,
    output: Option<Value>,
    error: Option<String>,
}

#[derive(Debug)]
pub(crate) struct BackgroundToolJob {
    job_id: String,
    tool_name: String,
    started_at_unix_ms: i64,
    state: Mutex<BackgroundJobState>,
    notify: Notify,
}

static BACKGROUND_JOBS: OnceLock<Mutex<HashMap<String, Arc<BackgroundToolJob>>>> = OnceLock::new();

fn jobs() -> &'static Mutex<HashMap<String, Arc<BackgroundToolJob>>> {
    BACKGROUND_JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn serialize_job(job: &BackgroundToolJob) -> Value {
    let state = job
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    json!({
        "jobId": job.job_id,
        "toolName": job.tool_name,
        "status": state.status.as_str(),
        "startedAtUnixMs": job.started_at_unix_ms,
        "completedAtUnixMs": state.completed_at_unix_ms,
        "durationMs": state.duration_ms,
        "output": state.output,
        "error": state.error,
    })
}

pub(crate) fn job_id(job: &Arc<BackgroundToolJob>) -> String {
    job.job_id.clone()
}

pub(crate) fn job_snapshot(job: &Arc<BackgroundToolJob>) -> Value {
    serialize_job(job)
}

pub(crate) fn start_job(tool_name: impl Into<String>) -> Arc<BackgroundToolJob> {
    let job = Arc::new(BackgroundToolJob {
        job_id: format!("job_{}", Uuid::new_v4().simple()),
        tool_name: tool_name.into(),
        started_at_unix_ms: now_unix_ms(),
        state: Mutex::new(BackgroundJobState {
            status: BackgroundJobStatus::InProgress,
            completed_at_unix_ms: None,
            duration_ms: None,
            output: None,
            error: None,
        }),
        notify: Notify::new(),
    });

    let mut guard = jobs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.insert(job.job_id.clone(), job.clone());
    job
}

pub(crate) fn complete_job(job: &Arc<BackgroundToolJob>, result: Result<Value, String>) {
    let completed_at_unix_ms = now_unix_ms();
    let duration_ms = completed_at_unix_ms
        .saturating_sub(job.started_at_unix_ms)
        .max(0) as u64;
    let mut state = job
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    match result {
        Ok(output) => {
            state.status = BackgroundJobStatus::Completed;
            state.output = Some(output);
            state.error = None;
        }
        Err(error) => {
            state.status = BackgroundJobStatus::Failed;
            state.output = None;
            state.error = Some(error);
        }
    }
    state.completed_at_unix_ms = Some(completed_at_unix_ms);
    state.duration_ms = Some(duration_ms);
    drop(state);

    job.notify.notify_waiters();
}

pub(crate) fn list_jobs() -> Value {
    let guard = jobs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut items = guard
        .values()
        .map(|job| serialize_job(job))
        .collect::<Vec<_>>();
    items.sort_by_key(|item| {
        item.get("startedAtUnixMs")
            .and_then(Value::as_i64)
            .unwrap_or_default()
    });
    json!({
        "ok": true,
        "jobs": items,
    })
}

pub(crate) async fn wait_for_job(job_id: &str, timeout_ms: Option<u64>) -> Result<Value, String> {
    let job = {
        let guard = jobs()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard
            .get(job_id)
            .cloned()
            .ok_or_else(|| format!("Background tool job '{}' was not found", job_id))?
    };
    let timeout_ms = timeout_ms
        .unwrap_or(DEFAULT_WAIT_TIMEOUT_MS)
        .clamp(1, MAX_WAIT_TIMEOUT_MS);

    {
        let state = job
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.status != BackgroundJobStatus::InProgress {
            return Ok(json!({
                "ok": state.status == BackgroundJobStatus::Completed,
                "timedOut": false,
                "job": serialize_job(&job),
            }));
        }
    }

    let notified = job.notify.notified();
    let timed_out = tokio::time::timeout(Duration::from_millis(timeout_ms), notified)
        .await
        .is_err();
    let snapshot = serialize_job(&job);
    let completed = snapshot
        .get("status")
        .and_then(Value::as_str)
        .map(|status| status == BackgroundJobStatus::Completed.as_str())
        .unwrap_or(false);

    Ok(json!({
        "ok": completed,
        "timedOut": timed_out,
        "job": snapshot,
    }))
}
