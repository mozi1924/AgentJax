use super::tool_parsing::describe_item_shape;
use super::{MAX_REPEATED_FAILED_SIGNATURES, MAX_TOOL_EXEC_RETRIES};
use crate::conversation_store_utils::now_unix_ms;
use crate::providers::types::{ProviderPendingToolCall, ProviderStreamEvent};
use crate::time_context::attach_tool_output_time_metadata;
use crate::tools::{
    ToolCatalogExecution, ToolCatalogSnapshot, ToolCatalogStateChange, ToolExecutionContext,
};
use futures_util::FutureExt;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::JoinHandle;

const MAX_PARALLEL_TOOL_EXECUTIONS: usize = 4;
pub(super) const TOOL_PROGRESS_HEARTBEAT_SECS: u64 = 5;

pub(super) struct ExecutedToolBatch {
    pub tool_results_items: Vec<Value>,
    pub executed_tool_call_items: Vec<Value>,
    pub timeline_events: Vec<Value>,
    pub state_changes: Vec<ToolCatalogStateChange>,
}

struct ExecutedToolRecord {
    index: usize,
    call_id: String,
    name: String,
    args: Value,
    signature: String,
    output_str: String,
    is_success: bool,
    started_at_unix_ms: i64,
    completed_at_unix_ms: i64,
    duration_ms: u64,
    state_changes: Vec<ToolCatalogStateChange>,
}

struct PreparedToolExecution {
    index: usize,
    call_id: String,
    name: String,
    args: Value,
    signature: String,
    repeated_fail_count: usize,
    is_repeated_failure_guarded: bool,
}

struct ActiveToolExecution {
    name: String,
    started_at: Instant,
}

type ToolCompletionMessage = (String, Option<ExecutedToolRecord>);
type ToolExecutionJoinHandle = JoinHandle<()>;

pub(super) struct ToolExecutionScheduler {
    conversation_id: String,
    tool_snapshot: ToolCatalogSnapshot,
    cancel_rx: watch::Receiver<bool>,
    semaphore: Arc<Semaphore>,
    next_index: usize,
    scheduled_call_ids: HashSet<String>,
    active_tools: HashMap<String, ActiveToolExecution>,
    completed_tx: mpsc::UnboundedSender<ToolCompletionMessage>,
    completed_rx: mpsc::UnboundedReceiver<ToolCompletionMessage>,
    executed_records: Vec<ExecutedToolRecord>,
    handles: Vec<ToolExecutionJoinHandle>,
}

impl ToolExecutionScheduler {
    pub(super) fn new(
        conversation_id: impl Into<String>,
        tool_snapshot: ToolCatalogSnapshot,
        supports_parallel_tool_calls: bool,
        cancel_rx: &watch::Receiver<bool>,
    ) -> Self {
        let parallelism = if supports_parallel_tool_calls {
            MAX_PARALLEL_TOOL_EXECUTIONS.max(1)
        } else {
            1
        };

        let (completed_tx, completed_rx) = mpsc::unbounded_channel();

        Self {
            conversation_id: conversation_id.into(),
            tool_snapshot,
            cancel_rx: cancel_rx.clone(),
            semaphore: Arc::new(Semaphore::new(parallelism)),
            next_index: 0,
            scheduled_call_ids: HashSet::new(),
            active_tools: HashMap::new(),
            completed_tx,
            completed_rx,
            executed_records: Vec::new(),
            handles: Vec::new(),
        }
    }

    pub(super) fn schedule_pending_tool(
        &mut self,
        pending: ProviderPendingToolCall,
        repeated_failed_tool_signatures: &HashMap<String, usize>,
    ) -> bool {
        if self.scheduled_call_ids.contains(&pending.call_id) {
            return false;
        }

        let prepared =
            prepare_tool_execution(self.next_index, pending, repeated_failed_tool_signatures);
        self.next_index += 1;
        self.scheduled_call_ids.insert(prepared.call_id.clone());
        self.active_tools.insert(
            prepared.call_id.clone(),
            ActiveToolExecution {
                name: prepared.name.clone(),
                started_at: Instant::now(),
            },
        );

        let call_id = prepared.call_id.clone();
        let conversation_id = self.conversation_id.clone();
        let tool_snapshot = self.tool_snapshot.clone();
        let cancel_rx = self.cancel_rx.clone();
        let semaphore = self.semaphore.clone();
        let completed_tx = self.completed_tx.clone();
        self.handles.push(tokio::spawn(async move {
            let record = AssertUnwindSafe(run_prepared_tool(
                tool_snapshot,
                conversation_id,
                cancel_rx,
                semaphore,
                prepared,
            ))
            .catch_unwind()
            .await
            .unwrap_or_else(|_| {
                log::warn!("Tool execution task panicked for call_id={call_id}");
                None
            });
            let _ = completed_tx.send((call_id, record));
        }));

        true
    }

    pub(super) fn schedule_pending_tools(
        &mut self,
        pending_tools: Vec<ProviderPendingToolCall>,
        repeated_failed_tool_signatures: &HashMap<String, usize>,
    ) {
        for pending in pending_tools {
            self.schedule_pending_tool(pending, repeated_failed_tool_signatures);
        }
    }

    pub(super) fn has_active_tools(&self) -> bool {
        !self.active_tools.is_empty()
    }

    pub(super) fn try_emit_completed_tools<F>(&mut self, on_event: &mut F) -> Result<(), String>
    where
        F: FnMut(ProviderStreamEvent) -> Result<(), String> + Send,
    {
        while let Ok(message) = self.completed_rx.try_recv() {
            self.record_completion_message(message, on_event)?;
        }
        Ok(())
    }

    pub(super) fn emit_progress_events<F>(&self, on_event: &mut F) -> Result<(), String>
    where
        F: FnMut(ProviderStreamEvent) -> Result<(), String> + Send,
    {
        for (call_id, active) in &self.active_tools {
            on_event(ProviderStreamEvent::ToolCallProgress {
                call_id: call_id.clone(),
                name: active.name.clone(),
                elapsed_ms: active.started_at.elapsed().as_millis() as u64,
                presentation: self.tool_snapshot.presentation_for(&active.name).cloned(),
            })?;
        }
        Ok(())
    }

    pub(super) async fn finish<F>(
        mut self,
        provider_kind: &str,
        repeated_failed_tool_signatures: &mut HashMap<String, usize>,
        on_event: &mut F,
    ) -> Result<ExecutedToolBatch, String>
    where
        F: FnMut(ProviderStreamEvent) -> Result<(), String> + Send,
    {
        let mut progress_interval =
            tokio::time::interval(Duration::from_secs(TOOL_PROGRESS_HEARTBEAT_SECS));
        progress_interval.tick().await;

        while !self.active_tools.is_empty() {
            tokio::select! {
                maybe_message = self.completed_rx.recv() => {
                    let Some(message) = maybe_message else {
                        break;
                    };
                    self.record_completion_message(message, on_event)?;
                }
                _ = progress_interval.tick() => {
                    self.try_emit_completed_tools(on_event)?;
                    self.emit_progress_events(on_event)?;
                }
            }
        }

        let executed_records = std::mem::take(&mut self.executed_records);
        finalize_executed_records(
            provider_kind,
            executed_records,
            repeated_failed_tool_signatures,
        )
    }

    fn record_completion_message<F>(
        &mut self,
        (call_id, maybe_record): ToolCompletionMessage,
        on_event: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(ProviderStreamEvent) -> Result<(), String> + Send,
    {
        self.active_tools.remove(&call_id);
        if let Some(record) = maybe_record {
            on_event(ProviderStreamEvent::ToolCallExecuted {
                call_id: record.call_id.clone(),
                name: record.name.clone(),
                output: record.output_str.clone(),
                is_success: record.is_success,
                started_at_unix_ms: record.started_at_unix_ms,
                completed_at_unix_ms: record.completed_at_unix_ms,
                duration_ms: record.duration_ms,
                presentation: self.tool_snapshot.presentation_for(&record.name).cloned(),
            })?;
            self.executed_records.push(record);
        }
        Ok(())
    }
}

impl Drop for ToolExecutionScheduler {
    fn drop(&mut self) {
        if self.active_tools.is_empty() {
            return;
        }
        for handle in &self.handles {
            handle.abort();
        }
    }
}

fn prepare_tool_execution(
    index: usize,
    pending: ProviderPendingToolCall,
    repeated_failed_tool_signatures: &HashMap<String, usize>,
) -> PreparedToolExecution {
    let call_id = pending.call_id;
    let name = pending.name;
    let args = if pending.arguments.is_object() {
        pending.arguments
    } else {
        log::warn!(
            "Tool call '{}' arguments are not an object (type={}), defaulting to empty object",
            call_id,
            describe_item_shape(&pending.arguments)
        );
        json!({})
    };

    let signature = format!(
        "{}::{}",
        name,
        serde_json::to_string(&args).unwrap_or_default()
    );
    let repeated_fail_count = repeated_failed_tool_signatures
        .get(&signature)
        .copied()
        .unwrap_or(0);
    let is_repeated_failure_guarded = repeated_fail_count >= MAX_REPEATED_FAILED_SIGNATURES;

    PreparedToolExecution {
        index,
        call_id,
        name,
        args,
        signature,
        repeated_fail_count,
        is_repeated_failure_guarded,
    }
}

async fn run_prepared_tool(
    tool_snapshot: ToolCatalogSnapshot,
    conversation_id: String,
    cancel_rx: watch::Receiver<bool>,
    semaphore: Arc<Semaphore>,
    pending: PreparedToolExecution,
) -> Option<ExecutedToolRecord> {
    let permit = semaphore.acquire_owned().await.ok()?;
    let record = execute_prepared_tool(tool_snapshot, conversation_id, cancel_rx, pending).await;
    drop(permit);
    record
}

async fn execute_prepared_tool(
    tool_snapshot: ToolCatalogSnapshot,
    conversation_id: String,
    cancel_rx: watch::Receiver<bool>,
    pending: PreparedToolExecution,
) -> Option<ExecutedToolRecord> {
    if *cancel_rx.borrow() {
        return None;
    }

    let PreparedToolExecution {
        index,
        call_id,
        name,
        args,
        signature,
        repeated_fail_count,
        is_repeated_failure_guarded,
    } = pending;

    let context = ToolExecutionContext {
        conversation_id: Some(conversation_id),
    };
    let start_time = Instant::now();
    let started_at_unix_ms = now_unix_ms();
    let mut last_error: Option<String> = None;
    let mut success_result: Option<ToolCatalogExecution> = None;
    let mut attempt = 0usize;
    let mut max_attempts = MAX_TOOL_EXEC_RETRIES;
    if is_repeated_failure_guarded {
        max_attempts = 0;
    }

    while attempt < max_attempts {
        if *cancel_rx.borrow() {
            last_error = Some("Tool execution cancelled".to_string());
            break;
        }
        attempt += 1;
        let exec_future = tool_snapshot.execute_with_effects(&name, &args, &context);
        let mut cancel_changed = cancel_rx.clone();
        tokio::select! {
            exec_result = exec_future => {
                match exec_result {
                    Ok(res) => {
                        success_result = Some(res);
                        break;
                    }
                    Err(err) => {
                        last_error = Some(err);
                    }
                }
            }
            changed = cancel_changed.changed() => {
                // Dropping the execution future is the fastest local
                // cancellation path for slow MCP/network-backed tools.
                if changed.is_err() || *cancel_changed.borrow() {
                    last_error = Some("Tool execution cancelled".to_string());
                    break;
                }
            }
        }
    }
    let duration_ms = start_time.elapsed().as_millis() as u64;
    let completed_at_unix_ms = now_unix_ms();

    let (output_str, is_success, state_changes) = if let Some(res) = success_result {
        let output_payload = json!({
            "ok": true,
            "tool": name,
            "result": res.output,
        });
        let output_payload = attach_tool_output_time_metadata(
            &output_payload,
            started_at_unix_ms,
            Some(completed_at_unix_ms),
            Some(duration_ms),
        );
        (
            serde_json::to_string(&output_payload).unwrap_or_default(),
            true,
            res.state_changes,
        )
    } else {
        let error_message = if is_repeated_failure_guarded {
            format!(
                "Tool '{}' with the same arguments has failed {} times in a row. Stop retrying this exact call and adjust arguments or choose another approach.",
                name, repeated_fail_count
            )
        } else {
            last_error.unwrap_or_else(|| "Tool execution failed".to_string())
        };
        let output_payload = json!({
            "ok": false,
            "tool": name,
            "error": {
                "message": error_message,
                "retriable": !is_repeated_failure_guarded,
                "attempts": max_attempts,
            }
        });
        let output_payload = attach_tool_output_time_metadata(
            &output_payload,
            started_at_unix_ms,
            Some(completed_at_unix_ms),
            Some(duration_ms),
        );
        (
            serde_json::to_string(&output_payload).unwrap_or_default(),
            false,
            Vec::new(),
        )
    };

    Some(ExecutedToolRecord {
        index,
        call_id,
        name,
        args,
        signature,
        output_str,
        is_success,
        started_at_unix_ms,
        completed_at_unix_ms,
        duration_ms,
        state_changes,
    })
}

fn finalize_executed_records(
    provider_kind: &str,
    mut executed_records: Vec<ExecutedToolRecord>,
    repeated_failed_tool_signatures: &mut HashMap<String, usize>,
) -> Result<ExecutedToolBatch, String> {
    let mut tool_results_items = Vec::new();
    let mut executed_tool_call_items = Vec::new();
    let mut timeline_events = Vec::new();
    let mut state_changes = Vec::new();

    executed_records.sort_by_key(|record| record.index);

    for record in executed_records {
        let ExecutedToolRecord {
            call_id,
            name,
            args,
            signature,
            output_str,
            is_success,
            started_at_unix_ms,
            completed_at_unix_ms,
            duration_ms,
            state_changes: record_state_changes,
            ..
        } = record;

        let output_val: Value =
            serde_json::from_str(&output_str).unwrap_or_else(|_| Value::String(output_str.clone()));
        timeline_events.push(json!({
            "type": "toolCall",
            "callId": call_id.clone(),
            "name": name.clone(),
            "arguments": args.clone(),
            "output": output_val,
            "status": if is_success { "success" } else { "failed" },
            "startedAtUnixMs": started_at_unix_ms,
            "completedAtUnixMs": completed_at_unix_ms,
            "durationMs": duration_ms
        }));
        if is_success {
            repeated_failed_tool_signatures.remove(&signature);
        } else {
            repeated_failed_tool_signatures
                .entry(signature)
                .and_modify(|count| *count += 1)
                .or_insert(1);
        }

        let tool_input_item =
            crate::providers::build_tool_result_input_item(provider_kind, &call_id, &output_str)?;
        tool_results_items.push(tool_input_item);
        state_changes.extend(record_state_changes);

        executed_tool_call_items.push(json!({
            "type": "function_call",
            "call_id": call_id,
            "name": name,
            "arguments": serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string()),
        }));
    }

    Ok(ExecutedToolBatch {
        tool_results_items,
        executed_tool_call_items,
        timeline_events,
        state_changes,
    })
}

#[cfg(test)]
mod tests {
    use super::ToolExecutionScheduler;
    use crate::providers::types::{ProviderPendingToolCall, ProviderStreamEvent};
    use crate::tools::{ToolCatalog, ToolExecutionContext};
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::sync::watch;

    #[tokio::test]
    async fn scheduler_can_emit_completed_event_before_finish() {
        let config = crate::config::AppConfig::default();
        let catalog = ToolCatalog::new(Arc::new(crate::mcp::McpManager::new()), &config);
        let snapshot = catalog.snapshot(&ToolExecutionContext::default()).await;
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let mut scheduler =
            ToolExecutionScheduler::new("conv-runtime-test", snapshot, true, &cancel_rx);

        assert!(scheduler.schedule_pending_tool(
            ProviderPendingToolCall {
                call_id: "call-calculator".to_string(),
                name: "calculator".to_string(),
                arguments: json!({ "expression": "6 * 7" }),
            },
            &HashMap::new(),
        ));

        // The foreground collector calls this while provider streaming is still
        // active, so completion events must be available before final batching.
        let mut events = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            scheduler
                .try_emit_completed_tools(&mut |event| {
                    events.push(event);
                    Ok(())
                })
                .expect("drain completed tools");
            if events
                .iter()
                .any(|event| matches!(event, ProviderStreamEvent::ToolCallExecuted { .. }))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let executed_events = events
            .iter()
            .filter(|event| matches!(event, ProviderStreamEvent::ToolCallExecuted { .. }))
            .count();
        assert_eq!(executed_events, 1);

        let mut repeated_failures = HashMap::new();
        let batch = scheduler
            .finish("openai-responses", &mut repeated_failures, &mut |event| {
                events.push(event);
                Ok(())
            })
            .await
            .expect("finish scheduled tools");

        assert_eq!(batch.tool_results_items.len(), 1);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, ProviderStreamEvent::ToolCallExecuted { .. }))
                .count(),
            1,
            "finish must not re-emit executions already drained by the foreground collector"
        );
    }
}
