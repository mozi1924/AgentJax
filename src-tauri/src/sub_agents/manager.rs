//! SubAgentManager — process-wide registry for async sub-agent tasks.
//!
//! Mirrors the pattern from `tools/background_jobs.rs` exactly:
//! - `OnceLock<Mutex<HashMap<String, Arc<SubAgentTask>>>>` static registry
//! - `Arc<SubAgentTask>` holding `Mutex<SubAgentState>`, `Mutex<Option<JoinHandle<()>>>`,
//!   `watch::Sender<bool>` (for cancel), and `tokio::sync::Notify` (for waiters)
//! - TTL pruning: retain terminal sub-agents for 6 hours, max 200 entries
//! - Conversation-scoped visibility

use crate::conversation_store_utils::now_unix_ms;
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::sub_agents::types::{
    ProgressMessage, SubAgentSnapshot, SubAgentSpec, SubAgentState, SubAgentStatus, SubAgentType,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

// ── Constants ─────────────────────────────────────────────────────────────────

const TERMINAL_AGENT_RETENTION_MS: i64 = 6 * 60 * 60 * 1_000; // 6 hours
const MAX_RETAINED_TERMINAL_AGENTS: usize = 200;
pub(crate) const DEFAULT_MAX_TURNS: usize = 5;
pub(crate) const HARD_MAX_TURNS: usize = 10;

// ── Global Concurrency ──────────────────────────────────────────────────────

/// Maximum concurrent sub-agent executions (across all conversations).
pub(crate) const MAX_CONCURRENT_SUB_AGENTS: usize = 16;

static SUB_AGENT_SEMAPHORE: OnceLock<tokio::sync::Semaphore> = OnceLock::new();

/// Returns the global sub-agent concurrency semaphore.
/// Each spawned sub-agent acquires a permit from this semaphore before starting
/// to execute, limiting the total number of concurrently running sub-agents.
pub(crate) fn sub_agent_semaphore() -> &'static tokio::sync::Semaphore {
    SUB_AGENT_SEMAPHORE.get_or_init(|| tokio::sync::Semaphore::new(MAX_CONCURRENT_SUB_AGENTS))
}

// ── SubAgentTask ──────────────────────────────────────────────────────────────

/// A single sub-agent task tracked in the process-wide registry.
pub(crate) struct SubAgentTask {
    pub state: Mutex<SubAgentState>,
    /// Handle to the spawned tokio task. `None` if not yet spawned or already joined.
    pub handle: Mutex<Option<JoinHandle<()>>>,
    /// Cancel signal sender. The runner watches this.
    pub cancel_tx: tokio::sync::watch::Sender<bool>,
    /// Notified when the task reaches a terminal state.
    pub notify: Notify,
    /// Signal sender for the memory sub-agent (Only populated for Memory type).
    /// The chat handler uses this to send TurnCompleted/Terminate signals.
    pub memory_signal_tx: Mutex<
        Option<tokio::sync::watch::Sender<Option<crate::sub_agents::types::MemoryAgentSignal>>>,
    >,
}

// ── Global Registry ───────────────────────────────────────────────────────────

static SUB_AGENTS: OnceLock<Mutex<HashMap<String, Arc<SubAgentTask>>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, Arc<SubAgentTask>>> {
    SUB_AGENTS.get_or_init(|| Mutex::new(HashMap::new()))
}

// ── Visibility ────────────────────────────────────────────────────────────────

fn agent_visible_to_conversation(task: &SubAgentTask, conversation_id: Option<&str>) -> bool {
    match (
        conversation_id,
        task.state
            .lock()
            .ok()
            .map(|s| s.spec.parent_conversation_id.clone()),
    ) {
        (Some(requested), Some(owner)) => requested == owner,
        (Some(_), None) => false,
        // Tests and internal maintenance can omit conversation_id.
        (None, _) => true,
    }
}

// ── Serialization ─────────────────────────────────────────────────────────────

fn serialize_task(task: &SubAgentTask) -> Value {
    let state = task
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    json!({
        "agentId": state.agent_id,
        "parentConversationId": state.spec.parent_conversation_id,
        "subagentType": state.spec.subagent_type.as_str(),
        "prompt": state.spec.prompt,
        "status": state.status.as_str(),
        "startedAtUnixMs": state.started_at_unix_ms,
        "completedAtUnixMs": state.completed_at_unix_ms,
        "durationMs": state.duration_ms,
        "turnsCompleted": state.turns_completed,
        "maxTurns": state.spec.max_turns,
        "result": state.result,
        "error": state.error,
    })
}

fn to_snapshot(task: &SubAgentTask) -> SubAgentSnapshot {
    let state = task
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    SubAgentSnapshot {
        agent_id: state.agent_id.clone(),
        parent_conversation_id: state.spec.parent_conversation_id.clone(),
        subagent_type: state.spec.subagent_type.as_str().to_string(),
        prompt: state.spec.prompt.clone(),
        status: state.status.as_str().to_string(),
        started_at_unix_ms: state.started_at_unix_ms,
        completed_at_unix_ms: state.completed_at_unix_ms,
        duration_ms: state.duration_ms,
        turns_completed: state.turns_completed,
        max_turns: state.spec.max_turns,
        error: state.error.clone(),
    }
}

// ── Resolve ───────────────────────────────────────────────────────────────────

fn resolve_task(
    agent_id: &str,
    conversation_id: Option<&str>,
) -> crate::error::AgentJaxResult<Arc<SubAgentTask>> {
    let guard = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let task = guard
        .get(agent_id)
        .cloned()
        .filter(|t| agent_visible_to_conversation(t, conversation_id))
        .ok_or_else(|| format!("Sub-agent '{}' was not found", agent_id))?;
    Ok(task)
}

// ── Pruning ───────────────────────────────────────────────────────────────────

fn prune_terminal_agents_locked(
    reg: &mut HashMap<String, Arc<SubAgentTask>>,
    retention_ms: i64,
    max_retained: usize,
) -> usize {
    let now = now_unix_ms();
    let cutoff = now.saturating_sub(retention_ms);
    let mut removed = 0usize;

    // Collect terminal agents with their completion timestamps.
    let mut terminal_agents: Vec<(String, i64)> = reg
        .iter()
        .filter_map(|(agent_id, task)| {
            let state = task
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !state.status.is_terminal() {
                return None;
            }
            Some((
                agent_id.clone(),
                state
                    .completed_at_unix_ms
                    .unwrap_or(state.started_at_unix_ms),
            ))
        })
        .collect();

    // Remove expired by age.
    let expired_ids: Vec<String> = terminal_agents
        .iter()
        .filter_map(|(agent_id, completed_at)| {
            if *completed_at <= cutoff {
                Some(agent_id.clone())
            } else {
                None
            }
        })
        .collect();

    for agent_id in &expired_ids {
        if reg.remove(agent_id).is_some() {
            removed += 1;
        }
    }

    // Remove excess by count.
    terminal_agents.retain(|(id, _)| reg.contains_key(id));
    if terminal_agents.len() > max_retained {
        terminal_agents.sort_by_key(|(_, completed_at)| *completed_at);
        let excess = terminal_agents.len() - max_retained;
        for (agent_id, _) in terminal_agents.into_iter().take(excess) {
            if reg.remove(&agent_id).is_some() {
                removed += 1;
            }
        }
    }

    removed
}

fn prune_agents() -> usize {
    let mut guard = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    prune_terminal_agents_locked(
        &mut guard,
        TERMINAL_AGENT_RETENTION_MS,
        MAX_RETAINED_TERMINAL_AGENTS,
    )
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Manages the lifecycle of async sub-agents.
pub struct SubAgentManager;

impl SubAgentManager {
    /// Register a new sub-agent task in the process-wide registry.
    ///
    /// Returns the `Arc<SubAgentTask>` so the caller can spawn the runner
    /// and register the join handle.
    pub fn register(spec: SubAgentSpec) -> Arc<SubAgentTask> {
        let now = now_unix_ms();
        let (cancel_tx, _cancel_rx) = tokio::sync::watch::channel(false);

        let state = SubAgentState {
            agent_id: spec.agent_id.clone(),
            spec: spec.clone(),
            status: SubAgentStatus::Pending,
            started_at_unix_ms: now,
            completed_at_unix_ms: None,
            duration_ms: None,
            result: None,
            error: None,
            progress_messages: Vec::new(),
            turns_completed: 0,
        };

        let task = Arc::new(SubAgentTask {
            state: Mutex::new(state),
            handle: Mutex::new(None),
            cancel_tx,
            notify: Notify::new(),
            memory_signal_tx: Mutex::new(None),
        });

        let mut guard = registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        prune_terminal_agents_locked(
            &mut guard,
            TERMINAL_AGENT_RETENTION_MS,
            MAX_RETAINED_TERMINAL_AGENTS,
        );
        guard.insert(spec.agent_id.clone(), task.clone());
        task
    }

    /// Mark a sub-agent as running and attach its JoinHandle.
    #[allow(dead_code)]
    pub fn mark_running(task: &Arc<SubAgentTask>, handle: JoinHandle<()>) {
        {
            let mut state = task
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.status = SubAgentStatus::Running;
        }
        let mut guard = task
            .handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Some(handle);
    }

    /// Append a progress message to a running sub-agent.
    #[allow(dead_code)]
    pub fn append_progress(task: &Arc<SubAgentTask>, text: String) {
        if let Ok(mut state) = task.state.lock()
            && state.status == SubAgentStatus::Running
        {
            state.progress_messages.push(ProgressMessage {
                text,
                ts: now_unix_ms(),
            });
        }
    }

    /// Mark a sub-agent as completed with a result.
    pub fn complete(task: &Arc<SubAgentTask>, result: Value) {
        let completed_at = now_unix_ms();
        let (conv_id, agent_id, type_str) = {
            let mut state = task
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.status != SubAgentStatus::Running {
                return;
            }
            state.status = SubAgentStatus::Completed;
            state.completed_at_unix_ms = Some(completed_at);
            state.duration_ms =
                Some(completed_at.saturating_sub(state.started_at_unix_ms).max(0) as u64);
            state.result = Some(result.clone());
            (
                state.spec.parent_conversation_id.clone(),
                state.agent_id.clone(),
                state.spec.subagent_type.as_str().to_string(),
            )
        };
        task.notify.notify_waiters();

        // Deposit into Street for proactive context injection.
        crate::street::StreetManager::deposit(crate::street::StreetItem::new(
            &conv_id,
            crate::street::StreetSource::SubAgent,
            crate::street::Priority::Normal,
            &format!(
                "Sub-agent '{}' ({}) completed successfully",
                agent_id, type_str
            ),
            result,
        ));
    }

    /// Mark a sub-agent as failed with an error.
    pub fn fail(task: &Arc<SubAgentTask>, error: String) {
        let completed_at = now_unix_ms();
        let (conv_id, agent_id, type_str) = {
            let mut state = task
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.status != SubAgentStatus::Running {
                return;
            }
            state.status = SubAgentStatus::Failed;
            state.completed_at_unix_ms = Some(completed_at);
            state.duration_ms =
                Some(completed_at.saturating_sub(state.started_at_unix_ms).max(0) as u64);
            state.error = Some(error.clone());
            (
                state.spec.parent_conversation_id.clone(),
                state.agent_id.clone(),
                state.spec.subagent_type.as_str().to_string(),
            )
        };
        task.notify.notify_waiters();

        // Deposit into Street for proactive context injection.
        let truncated: String = error.chars().take(200).collect();
        crate::street::StreetManager::deposit(crate::street::StreetItem::new(
            &conv_id,
            crate::street::StreetSource::SubAgent,
            crate::street::Priority::Normal,
            &format!(
                "Sub-agent '{}' ({}) failed: {}",
                agent_id, type_str, truncated
            ),
            serde_json::json!({"error": error}),
        ));
    }

    /// Cancel a sub-agent by ID.
    pub fn cancel(agent_id: &str, conversation_id: Option<&str>) -> AgentJaxResult<Value> {
        let task = resolve_task(agent_id, conversation_id).map_err(AgentJaxError::not_found)?;

        let should_abort = {
            let completed_at = now_unix_ms();
            let mut state = task
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.status == SubAgentStatus::Running || state.status == SubAgentStatus::Pending {
                state.status = SubAgentStatus::Cancelled;
                state.completed_at_unix_ms = Some(completed_at);
                state.duration_ms =
                    Some(completed_at.saturating_sub(state.started_at_unix_ms).max(0) as u64);
                state.error = Some("Sub-agent was cancelled".to_string());
                true
            } else {
                false
            }
        };

        if should_abort {
            // Signal cancellation through the watch channel.
            let _ = task.cancel_tx.send(true);
            // Abort the tokio task if it's still running.
            if let Some(handle) = task
                .handle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
            {
                handle.abort();
            }
            task.notify.notify_waiters();
        }

        prune_agents();
        Ok(json!({
            "ok": true,
            "cancelled": should_abort,
            "agent": serialize_task(&task),
        }))
    }

    /// Collect and atomically mark as Running all Pending sub-agent specs
    /// for a given conversation. Returns (Arc<SubAgentTask>, SubAgentSpec)
    /// pairs so the caller can spawn runners via tokio::spawn.
    ///
    /// This separates registration (tool execution) from spawning (chat handler)
    /// to avoid circular dependencies between the tool layer and the catalog.
    pub fn collect_pending(conversation_id: &str) -> Vec<(Arc<SubAgentTask>, SubAgentSpec)> {
        let guard = registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard
            .values()
            .filter(|task| {
                task.state
                    .lock()
                    .ok()
                    .map(|s| {
                        s.status == SubAgentStatus::Pending
                            && s.spec.parent_conversation_id == conversation_id
                    })
                    .unwrap_or(false)
            })
            .map(|task| {
                // Atomically transition to Running under the registry lock.
                let spec = {
                    let mut state = task
                        .state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.status = SubAgentStatus::Running;
                    state.spec.clone()
                };
                (task.clone(), spec)
            })
            .collect()
    }

    /// Find the memory sub-agent for a given conversation, if one exists.
    pub fn get_memory_agent_for_conversation(conversation_id: &str) -> Option<Arc<SubAgentTask>> {
        let guard = registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard
            .values()
            .find(|task| {
                task.state
                    .lock()
                    .ok()
                    .map(|s| {
                        s.spec.subagent_type == SubAgentType::Memory
                            && s.spec.parent_conversation_id == conversation_id
                    })
                    .unwrap_or(false)
            })
            .cloned()
    }

    /// Send a signal to the memory agent for a given conversation.
    /// Returns true if the signal was sent successfully.
    pub fn signal_memory_agent(
        conversation_id: &str,
        signal: crate::sub_agents::types::MemoryAgentSignal,
    ) -> bool {
        if let Some(task) = Self::get_memory_agent_for_conversation(conversation_id)
            && let Ok(tx_guard) = task.memory_signal_tx.lock()
            && let Some(tx) = tx_guard.as_ref()
        {
            let _ = tx.send(Some(signal));
            return true;
        }
        false
    }

    /// Cancel all sub-agents belonging to a conversation.
    /// Memory agents are skipped — they are persistent and should
    /// complete gracefully via Terminate signal.
    pub fn cancel_conversation_agents(conversation_id: &str) -> usize {
        let agents_to_cancel: Vec<Arc<SubAgentTask>> = {
            let guard = registry()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard
                .values()
                .filter(|task| {
                    task.state
                        .lock()
                        .ok()
                        .map(|s| {
                            s.spec.parent_conversation_id == conversation_id
                                && s.spec.subagent_type != SubAgentType::Memory
                        })
                        .unwrap_or(false)
                })
                .cloned()
                .collect()
        };

        let mut cancelled = 0usize;
        for task in &agents_to_cancel {
            let should_abort = {
                let mut state = task
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if state.status == SubAgentStatus::Running
                    || state.status == SubAgentStatus::Pending
                {
                    let completed_at = now_unix_ms();
                    state.status = SubAgentStatus::Cancelled;
                    state.completed_at_unix_ms = Some(completed_at);
                    state.duration_ms =
                        Some(completed_at.saturating_sub(state.started_at_unix_ms).max(0) as u64);
                    state.error = Some("Conversation was cancelled".to_string());
                    true
                } else {
                    false
                }
            };

            if should_abort {
                let _ = task.cancel_tx.send(true);
                if let Some(handle) = task
                    .handle
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take()
                {
                    handle.abort();
                }
                task.notify.notify_waiters();
                cancelled += 1;
            }
        }

        prune_agents();
        cancelled
    }

    /// Get the specification of a sub-agent by ID.
    pub fn get_spec(agent_id: &str) -> Option<SubAgentSpec> {
        let guard = registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.get(agent_id).and_then(|task| {
            task.state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .spec
                .clone()
                .into()
        })
    }

    /// Get a snapshot of a specific sub-agent.
    pub fn status(agent_id: &str, conversation_id: Option<&str>) -> AgentJaxResult<Value> {
        let task = resolve_task(agent_id, conversation_id).map_err(AgentJaxError::not_found)?;
        let snapshot = serialize_task(&task);
        let is_terminal = snapshot
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s != SubAgentStatus::Running.as_str() && s != SubAgentStatus::Pending.as_str())
            .unwrap_or(false);

        if is_terminal {
            prune_agents();
        }

        Ok(json!({
            "ok": true,
            "agent": snapshot,
        }))
    }

    /// List all sub-agents visible to a conversation.
    pub fn list(conversation_id: Option<&str>) -> Vec<SubAgentSnapshot> {
        let mut guard = registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        prune_terminal_agents_locked(
            &mut guard,
            TERMINAL_AGENT_RETENTION_MS,
            MAX_RETAINED_TERMINAL_AGENTS,
        );
        let mut snapshots: Vec<SubAgentSnapshot> = guard
            .values()
            .filter(|task| agent_visible_to_conversation(task, conversation_id))
            .map(|task| to_snapshot(task))
            .collect();
        snapshots.sort_by_key(|s| s.started_at_unix_ms);
        snapshots
    }

    /// Wait for a sub-agent to reach a terminal state.
    pub async fn wait(
        agent_id: &str,
        timeout_ms: Option<u64>,
        conversation_id: Option<&str>,
    ) -> AgentJaxResult<Value> {
        let task = resolve_task(agent_id, conversation_id).map_err(AgentJaxError::not_found)?;

        let timeout_ms = timeout_ms.unwrap_or(30_000).clamp(1, 300_000); // 30s default, 5min max

        // Register notification interest before checking state.
        let notified = task.notify.notified();

        {
            let state = task
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.status.is_terminal() {
                let completed = state.status == SubAgentStatus::Completed;
                drop(state);
                let snapshot = serialize_task(&task);
                prune_agents();
                return Ok(json!({
                    "ok": completed,
                    "timedOut": false,
                    "agent": snapshot,
                }));
            }
        }

        let timed_out =
            tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), notified)
                .await
                .is_err();

        let snapshot = serialize_task(&task);
        let completed = snapshot
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s == SubAgentStatus::Completed.as_str())
            .unwrap_or(false);

        let is_terminal = snapshot
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s != SubAgentStatus::Running.as_str() && s != SubAgentStatus::Pending.as_str())
            .unwrap_or(false);

        if is_terminal {
            prune_agents();
        }

        Ok(json!({
            "ok": completed,
            "timedOut": timed_out,
            "agent": snapshot,
        }))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sub_agents::types::SubAgentType;

    fn make_spec(agent_id: &str, conv_id: &str) -> SubAgentSpec {
        SubAgentSpec {
            agent_id: agent_id.to_string(),
            parent_conversation_id: conv_id.to_string(),
            subagent_type: SubAgentType::GeneralPurpose,
            prompt: "Test task".to_string(),
            delegated_scope: vec!["filesystem".to_string()],
            kept_work: vec!["result".to_string()],
            max_turns: 5,
            max_retries: 0,
            use_worktree: false,
            model_id: None,
            parent_request_id: "req_test".to_string(),
            persistent: false,
        }
    }

    #[test]
    fn test_register_and_list() {
        let conv_id = format!("conv_reg_{}", uuid::Uuid::new_v4().simple());
        let spec = make_spec("test_agent_1", &conv_id);
        let task = SubAgentManager::register(spec);
        assert_eq!(task.state.lock().unwrap().status, SubAgentStatus::Pending);

        let list = SubAgentManager::list(Some(&conv_id));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].agent_id, "test_agent_1");
    }

    fn unique_conv(prefix: &str) -> String {
        format!("{}_{}", prefix, uuid::Uuid::new_v4().simple())
    }

    #[test]
    fn test_visibility_scoped_to_conversation() {
        let ca = unique_conv("vis_a");
        let cb = unique_conv("vis_b");
        let spec_a = make_spec("agent_a", &ca);
        let spec_b = make_spec("agent_b", &cb);
        SubAgentManager::register(spec_a);
        SubAgentManager::register(spec_b);

        let list_a = SubAgentManager::list(Some(&ca));
        assert_eq!(list_a.len(), 1);
        assert_eq!(list_a[0].agent_id, "agent_a");

        let list_b = SubAgentManager::list(Some(&cb));
        assert_eq!(list_b.len(), 1);
        assert_eq!(list_b[0].agent_id, "agent_b");
    }

    #[tokio::test]
    async fn test_complete_and_status() {
        let conv = unique_conv("complete");
        let spec = make_spec("agent_complete", &conv);
        let task = SubAgentManager::register(spec);
        SubAgentManager::mark_running(&task, tokio::spawn(async {}));
        SubAgentManager::complete(&task, json!({"done": true}));

        let status = SubAgentManager::status("agent_complete", Some(&conv)).unwrap();
        let agent = &status["agent"];
        assert_eq!(agent["status"].as_str().unwrap(), "completed");
        assert!(agent["result"]["done"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_fail_and_status() {
        let conv = unique_conv("fail");
        let spec = make_spec("agent_fail", &conv);
        let task = SubAgentManager::register(spec);
        SubAgentManager::mark_running(&task, tokio::spawn(async {}));
        SubAgentManager::fail(&task, "something broke".to_string());

        let status = SubAgentManager::status("agent_fail", Some(&conv)).unwrap();
        let agent = &status["agent"];
        assert_eq!(agent["status"].as_str().unwrap(), "failed");
        assert!(agent["error"].as_str().unwrap().contains("broke"));
    }

    #[tokio::test]
    async fn test_cancel() {
        let conv = unique_conv("cancel");
        let spec = make_spec("agent_cancel", &conv);
        let task = SubAgentManager::register(spec);
        SubAgentManager::mark_running(&task, tokio::spawn(async {}));

        let result = SubAgentManager::cancel("agent_cancel", Some(&conv)).unwrap();
        assert!(result["cancelled"].as_bool().unwrap());

        let status = SubAgentManager::status("agent_cancel", Some(&conv)).unwrap();
        assert_eq!(status["agent"]["status"].as_str().unwrap(), "cancelled");
    }

    #[tokio::test]
    async fn test_cancel_conversation_agents() {
        let cx = unique_conv("cc_x");
        let cy = unique_conv("cc_y");
        let spec1 = make_spec("agent_cc_1", &cx);
        let spec2 = make_spec("agent_cc_2", &cx);
        let spec3 = make_spec("agent_cc_3", &cy);
        let t1 = SubAgentManager::register(spec1);
        let t2 = SubAgentManager::register(spec2);
        let t3 = SubAgentManager::register(spec3);
        SubAgentManager::mark_running(&t1, tokio::spawn(async {}));
        SubAgentManager::mark_running(&t2, tokio::spawn(async {}));
        SubAgentManager::mark_running(&t3, tokio::spawn(async {}));

        let cancelled = SubAgentManager::cancel_conversation_agents(&cx);
        assert_eq!(cancelled, 2);

        // conv_y agent should still be running.
        let list_y = SubAgentManager::list(Some(&cy));
        assert_eq!(list_y.len(), 1);
        assert_eq!(list_y[0].status, "running");
    }

    #[test]
    fn test_not_found() {
        let conv = unique_conv("nf");
        let result = SubAgentManager::status("nonexistent", Some(&conv));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_wait_for_completion() {
        let conv = unique_conv("wait");
        let spec = make_spec("agent_wait", &conv);
        let task = SubAgentManager::register(spec);
        SubAgentManager::mark_running(&task, tokio::spawn(async {}));

        // Complete in a separate task after a short delay.
        let task_clone = task.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            SubAgentManager::complete(&task_clone, json!({"waited": true}));
        });

        let result = SubAgentManager::wait("agent_wait", Some(5000), Some(&conv))
            .await
            .unwrap();
        assert!(result["ok"].as_bool().unwrap());
        assert!(!result["timedOut"].as_bool().unwrap());
        assert_eq!(result["agent"]["status"].as_str().unwrap(), "completed");
    }

    #[tokio::test]
    async fn test_wait_timeout() {
        let conv = unique_conv("timeout");
        let spec = make_spec("agent_timeout", &conv);
        let task = SubAgentManager::register(spec);
        SubAgentManager::mark_running(&task, tokio::spawn(async {}));

        let result = SubAgentManager::wait("agent_timeout", Some(10), Some(&conv))
            .await
            .unwrap();
        assert!(result["timedOut"].as_bool().unwrap());
        assert_eq!(result["agent"]["status"].as_str().unwrap(), "running");
    }

    #[cfg(test)]
    pub(crate) fn age_completed_agent_for_test(agent_id: &str, completed_at_unix_ms: i64) {
        if let Ok(task) = resolve_task(agent_id, None) {
            let mut state = task
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.status.is_terminal() {
                state.completed_at_unix_ms = Some(completed_at_unix_ms);
            }
        }
    }

    #[tokio::test]
    async fn test_prune_by_age() {
        let conv = unique_conv("prune");
        let spec = make_spec("agent_old", &conv);
        let task = SubAgentManager::register(spec);
        SubAgentManager::mark_running(&task, tokio::spawn(async {}));
        SubAgentManager::complete(&task, json!({"done": true}));

        // Age the completed agent beyond retention.
        age_completed_agent_for_test(
            "agent_old",
            now_unix_ms() - TERMINAL_AGENT_RETENTION_MS - 1000,
        );

        let mut guard = registry().lock().unwrap();
        let removed = prune_terminal_agents_locked(&mut guard, TERMINAL_AGENT_RETENTION_MS, 200);
        assert!(removed >= 1, "Expected at least 1 pruned, got {removed}");
    }
}
