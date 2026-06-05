use crate::conversation_store_utils::now_unix_ms;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use uuid::Uuid;

pub(crate) const DEFAULT_WAIT_TIMEOUT_MS: u64 = 5_000;
pub(crate) const MAX_WAIT_TIMEOUT_MS: u64 = 120_000;
// Terminal job snapshots are useful for follow-up waits/lists, but they should
// not accumulate forever in the process-wide registry.
const TERMINAL_JOB_RETENTION_MS: i64 = 6 * 60 * 60 * 1_000;
const MAX_RETAINED_TERMINAL_JOBS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackgroundJobStatus {
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

impl BackgroundJobStatus {
    fn as_str(self) -> &'static str {
        match self {
            BackgroundJobStatus::InProgress => "in_progress",
            BackgroundJobStatus::Completed => "completed",
            BackgroundJobStatus::Failed => "failed",
            BackgroundJobStatus::Cancelled => "cancelled",
        }
    }

    fn is_terminal(self) -> bool {
        self != BackgroundJobStatus::InProgress
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
    conversation_id: Option<String>,
    started_at_unix_ms: i64,
    state: Mutex<BackgroundJobState>,
    handle: Mutex<Option<JoinHandle<()>>>,
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
        "conversationId": job.conversation_id,
        "status": state.status.as_str(),
        "startedAtUnixMs": job.started_at_unix_ms,
        "completedAtUnixMs": state.completed_at_unix_ms,
        "durationMs": state.duration_ms,
        "output": state.output,
        "error": state.error,
    })
}

fn job_visible_to_conversation(job: &BackgroundToolJob, conversation_id: Option<&str>) -> bool {
    match (conversation_id, job.conversation_id.as_deref()) {
        // Tool calls in a real chat turn should only see jobs that belong to
        // the same conversation. This prevents sidecar tools from leaking
        // another conversation's background output.
        (Some(requested), Some(owner)) => requested == owner,
        (Some(_), None) => false,
        // Tests and internal maintenance callers can omit the conversation id
        // to inspect the process-wide registry.
        (None, _) => true,
    }
}

fn resolve_job(
    job_id: &str,
    conversation_id: Option<&str>,
) -> crate::error::AgentJaxResult<Arc<BackgroundToolJob>> {
    let guard = jobs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let job = guard
        .get(job_id)
        .cloned()
        .filter(|job| job_visible_to_conversation(job, conversation_id))
        .ok_or_else(|| format!("Background tool job '{}' was not found", job_id))?;
    Ok(job)
}

fn mark_job_cancelled(job: &Arc<BackgroundToolJob>, reason: &str) -> bool {
    let mut should_abort = false;
    {
        let completed_at_unix_ms = now_unix_ms();
        let duration_ms = completed_at_unix_ms
            .saturating_sub(job.started_at_unix_ms)
            .max(0) as u64;
        let mut state = job
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if state.status == BackgroundJobStatus::InProgress {
            state.status = BackgroundJobStatus::Cancelled;
            state.completed_at_unix_ms = Some(completed_at_unix_ms);
            state.duration_ms = Some(duration_ms);
            state.output = None;
            state.error = Some(reason.to_string());
            should_abort = true;
        }
    }

    if should_abort {
        if let Some(handle) = job
            .handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            handle.abort();
        }
        job.notify.notify_waiters();
    }

    should_abort
}

fn prune_terminal_jobs_locked(
    registry: &mut HashMap<String, Arc<BackgroundToolJob>>,
    retention_ms: i64,
    max_retained_terminal_jobs: usize,
) -> usize {
    // Only prune terminal jobs. In-progress jobs may still be holding MCP or
    // native tool resources, so those are cancelled through explicit lifecycle
    // paths rather than garbage-collected here.
    let now = now_unix_ms();
    let cutoff = now.saturating_sub(retention_ms);
    let mut terminal_jobs = registry
        .iter()
        .filter_map(|(job_id, job)| {
            let state = job
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !state.status.is_terminal() {
                return None;
            }

            Some((
                job_id.clone(),
                state.completed_at_unix_ms.unwrap_or(job.started_at_unix_ms),
            ))
        })
        .collect::<Vec<_>>();

    let mut removed = 0usize;
    let expired_job_ids = terminal_jobs
        .iter()
        .filter_map(|(job_id, completed_at_unix_ms)| {
            if *completed_at_unix_ms <= cutoff {
                Some(job_id.clone())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    for job_id in expired_job_ids {
        if registry.remove(&job_id).is_some() {
            removed += 1;
        }
    }

    terminal_jobs.retain(|(job_id, _)| registry.contains_key(job_id));
    if terminal_jobs.len() > max_retained_terminal_jobs {
        terminal_jobs.sort_by_key(|(_, completed_at_unix_ms)| *completed_at_unix_ms);
        let excess_count = terminal_jobs.len() - max_retained_terminal_jobs;
        for (job_id, _) in terminal_jobs.into_iter().take(excess_count) {
            if registry.remove(&job_id).is_some() {
                removed += 1;
            }
        }
    }

    removed
}

fn prune_jobs() -> usize {
    let mut guard = jobs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    prune_terminal_jobs_locked(
        &mut guard,
        TERMINAL_JOB_RETENTION_MS,
        MAX_RETAINED_TERMINAL_JOBS,
    )
}

fn serialized_status_is_terminal(snapshot: &Value) -> bool {
    snapshot
        .get("status")
        .and_then(Value::as_str)
        .map(|status| status != BackgroundJobStatus::InProgress.as_str())
        .unwrap_or(false)
}

pub(crate) fn job_id(job: &Arc<BackgroundToolJob>) -> String {
    job.job_id.clone()
}

pub(crate) fn job_snapshot(job: &Arc<BackgroundToolJob>) -> Value {
    serialize_job(job)
}

pub(crate) fn start_job_for_conversation(
    tool_name: impl Into<String>,
    conversation_id: Option<String>,
) -> Arc<BackgroundToolJob> {
    let job = Arc::new(BackgroundToolJob {
        job_id: format!("job_{}", Uuid::new_v4().simple()),
        tool_name: tool_name.into(),
        conversation_id,
        started_at_unix_ms: now_unix_ms(),
        state: Mutex::new(BackgroundJobState {
            status: BackgroundJobStatus::InProgress,
            completed_at_unix_ms: None,
            duration_ms: None,
            output: None,
            error: None,
        }),
        handle: Mutex::new(None),
        notify: Notify::new(),
    });

    let mut guard = jobs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    prune_terminal_jobs_locked(
        &mut guard,
        TERMINAL_JOB_RETENTION_MS,
        MAX_RETAINED_TERMINAL_JOBS,
    );
    guard.insert(job.job_id.clone(), job.clone());
    job
}

pub(crate) fn register_job_handle(job: &Arc<BackgroundToolJob>, handle: JoinHandle<()>) {
    let mut guard = job
        .handle
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = Some(handle);
}

pub(crate) fn complete_job(
    job: &Arc<BackgroundToolJob>,
    result: crate::error::AgentJaxResult<Value>,
) {
    let completed_at_unix_ms = now_unix_ms();
    let duration_ms = completed_at_unix_ms
        .saturating_sub(job.started_at_unix_ms)
        .max(0) as u64;
    let (is_success, output_val, error_msg, conv_id) = {
        let mut state = job
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.status != BackgroundJobStatus::InProgress {
            return;
        }

        let success;
        match result {
            Ok(output) => {
                state.status = BackgroundJobStatus::Completed;
                state.output = Some(output.clone());
                state.error = None;
                success = true;
            }
            Err(error) => {
                state.status = BackgroundJobStatus::Failed;
                state.output = None;
                state.error = Some(error.to_string());
                success = false;
            }
        }
        state.completed_at_unix_ms = Some(completed_at_unix_ms);
        state.duration_ms = Some(duration_ms);
        (
            success,
            state.output.clone(),
            state.error.clone(),
            job.conversation_id.clone(),
        )
    };

    job.notify.notify_waiters();

    // Deposit into Street for proactive context injection.
    if let Some(conv_id) = conv_id {
        let title = if is_success {
            format!(
                "Background job '{}' ({}) completed",
                job.job_id, job.tool_name
            )
        } else {
            let err_preview: String = error_msg
                .clone()
                .unwrap_or_default()
                .chars()
                .take(80)
                .collect();
            format!(
                "Background job '{}' ({}) failed: {}",
                job.job_id, job.tool_name, err_preview
            )
        };
        let payload = output_val
            .unwrap_or_else(|| serde_json::json!({"error": error_msg.unwrap_or_default()}));
        crate::street::StreetManager::deposit(crate::street::StreetItem::new(
            &conv_id,
            crate::street::StreetSource::BackgroundJob,
            crate::street::Priority::Low,
            &title,
            payload,
        ));
    }
}

pub(crate) fn cancel_job(
    job_id: &str,
    conversation_id: Option<&str>,
) -> crate::error::AgentJaxResult<Value> {
    let job = resolve_job(job_id, conversation_id)?;
    let should_abort = mark_job_cancelled(&job, "Background tool job was cancelled");
    prune_jobs();

    Ok(json!({
        "ok": should_abort,
        "cancelled": should_abort,
        "job": serialize_job(&job),
    }))
}

pub(crate) fn cancel_conversation_jobs(conversation_id: &str) -> usize {
    let jobs_to_cancel = {
        let guard = jobs()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard
            .values()
            .filter(|job| job.conversation_id.as_deref() == Some(conversation_id))
            .cloned()
            .collect::<Vec<_>>()
    };

    let cancelled_count = jobs_to_cancel
        .iter()
        .filter(|job| mark_job_cancelled(job, "Conversation background tool job was cancelled"))
        .count();
    prune_jobs();
    cancelled_count
}

pub(crate) fn list_jobs(conversation_id: Option<&str>) -> Value {
    let mut guard = jobs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    prune_terminal_jobs_locked(
        &mut guard,
        TERMINAL_JOB_RETENTION_MS,
        MAX_RETAINED_TERMINAL_JOBS,
    );
    let mut items = guard
        .values()
        .filter(|job| job_visible_to_conversation(job, conversation_id))
        .map(|job| serialize_job(job))
        .collect::<Vec<_>>();
    items.sort_by_key(|item| {
        item.get("startedAtUnixMs")
            .and_then(Value::as_i64)
            .unwrap_or_default()
    });
    json!({
        "ok": true,
        "role": "background_tool_observer",
        "decision": "inspect_jobs_before_awaiting",
        "jobs": items,
    })
}

pub(crate) async fn wait_for_job(
    job_id: &str,
    timeout_ms: Option<u64>,
    conversation_id: Option<&str>,
) -> crate::error::AgentJaxResult<Value> {
    let job = resolve_job(job_id, conversation_id)?;
    let timeout_ms = timeout_ms
        .unwrap_or(DEFAULT_WAIT_TIMEOUT_MS)
        .clamp(1, MAX_WAIT_TIMEOUT_MS);

    // Register notification interest before checking state. Otherwise a fast
    // completion between the state check and `notified()` could be missed,
    // making waiters sleep until timeout even though the job already finished.
    let notified = job.notify.notified();

    {
        let state = job
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.status != BackgroundJobStatus::InProgress {
            let ok = state.status == BackgroundJobStatus::Completed;
            drop(state);
            let snapshot = serialize_job(&job);
            // Wait-heavy workflows may be the only code path observing that a
            // job became terminal, so prune after capturing the return payload.
            prune_jobs();
            return Ok(json!({
                "ok": ok,
                "timedOut": false,
                "role": "background_tool_awaiter",
                "decision": if ok { "result_ready" } else { "terminal_failure" },
                "job": snapshot,
            }));
        }
    }

    let timed_out = tokio::time::timeout(Duration::from_millis(timeout_ms), notified)
        .await
        .is_err();
    let snapshot = serialize_job(&job);
    let completed = snapshot
        .get("status")
        .and_then(Value::as_str)
        .map(|status| status == BackgroundJobStatus::Completed.as_str())
        .unwrap_or(false);
    if serialized_status_is_terminal(&snapshot) {
        prune_jobs();
    }

    Ok(json!({
        "ok": completed,
        "timedOut": timed_out,
        "role": "background_tool_awaiter",
        "decision": if completed {
            "result_ready"
        } else if timed_out {
            "continue_other_work_or_wait_again"
        } else {
            "terminal_failure"
        },
        "job": snapshot,
        "usage": if completed || serialized_status_is_terminal(&snapshot) {
            json!({})
        } else {
            json!({
                "waitAgain": {
                    "tool": "background_task",
                    "arguments": {
                        "action": "wait",
                        "jobId": job_id,
                        "timeoutMs": DEFAULT_WAIT_TIMEOUT_MS
                    }
                },
                "list": {
                    "tool": "background_task",
                    "arguments": { "action": "list" }
                },
                "cancel": {
                    "tool": "background_task",
                    "arguments": { "action": "cancel", "jobId": job_id }
                }
            })
        },
    }))
}

#[cfg(test)]
pub(crate) fn age_completed_job_for_test(job_id: &str, completed_at_unix_ms: i64) {
    if let Ok(job) = resolve_job(job_id, None) {
        let mut state = job
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.status.is_terminal() {
            state.completed_at_unix_ms = Some(completed_at_unix_ms);
        }
    }
}

#[cfg(test)]
pub(crate) fn prune_jobs_for_test(retention_ms: i64, max_retained_terminal_jobs: usize) -> usize {
    let mut guard = jobs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    prune_terminal_jobs_locked(&mut guard, retention_ms, max_retained_terminal_jobs)
}
