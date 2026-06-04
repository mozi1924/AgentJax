use crate::error::{AgentJaxError, AgentJaxResult};
use crate::provider_api::types::{ProviderPendingToolCall, ProviderStreamEvent};
use crate::tools::ToolCatalogSnapshot;
use futures_util::FutureExt;
use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::JoinHandle;

mod batch;
mod execution;
mod types;

use batch::finalize_executed_records;
use execution::{prepare_tool_execution, run_prepared_tool};
use types::{ActiveToolExecution, ExecutedToolBatch, ExecutedToolRecord};

const MAX_PARALLEL_TOOL_EXECUTIONS: usize = 4;
pub(super) const TOOL_PROGRESS_HEARTBEAT_SECS: u64 = 5;
/// Hard ceiling for all tool executions within a single hop.
/// Prevents `finish()` from hanging forever when a tool is unresponsive.
const TOOL_EXECUTION_HARD_TIMEOUT_SECS: u64 = 300;

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

    pub(super) fn try_emit_completed_tools<F>(&mut self, on_event: &mut F) -> AgentJaxResult<()>
    where
        F: FnMut(ProviderStreamEvent) -> Result<(), AgentJaxError> + Send,
    {
        while let Ok(message) = self.completed_rx.try_recv() {
            self.record_completion_message(message, on_event)?;
        }
        Ok(())
    }

    pub(super) fn emit_progress_events<F>(&self, on_event: &mut F) -> AgentJaxResult<()>
    where
        F: FnMut(ProviderStreamEvent) -> Result<(), AgentJaxError> + Send,
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
    ) -> AgentJaxResult<ExecutedToolBatch>
    where
        F: FnMut(ProviderStreamEvent) -> Result<(), AgentJaxError> + Send,
    {
        let mut progress_interval =
            tokio::time::interval(Duration::from_secs(TOOL_PROGRESS_HEARTBEAT_SECS));
        progress_interval.tick().await;

        // Clone of the cancel receiver so `changed()` starts watching from now
        // (the original was used during provider streaming).
        let mut cancel_changed = self.cancel_rx.clone();

        let inner_result = tokio::time::timeout(
            Duration::from_secs(TOOL_EXECUTION_HARD_TIMEOUT_SECS),
            async {
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
                        _ = cancel_changed.changed() => {
                            if *cancel_changed.borrow() {
                                for handle in &self.handles {
                                    handle.abort();
                                }
                                return Err(AgentJaxError::internal(
                                    "Tool execution cancelled",
                                ));
                            }
                        }
                    }
                }
                Ok::<_, AgentJaxError>(())
            },
        )
        .await;

        let () = inner_result.map_err(|_elapsed| {
            for handle in &self.handles {
                handle.abort();
            }
            AgentJaxError::internal(format!(
                "Tool execution timed out after {TOOL_EXECUTION_HARD_TIMEOUT_SECS}s"
            ))
        })??;

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
    ) -> AgentJaxResult<()>
    where
        F: FnMut(ProviderStreamEvent) -> Result<(), AgentJaxError> + Send,
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

#[cfg(test)]
mod tests {
    use super::ToolExecutionScheduler;
    use crate::provider_api::types::{ProviderPendingToolCall, ProviderStreamEvent};
    use crate::tools::{ToolCatalog, ToolExecutionContext, background_jobs};
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::sync::watch;

    #[tokio::test]
    async fn scheduler_can_emit_completed_event_before_finish() {
        let config = crate::config::AppConfig::default();
        let catalog = ToolCatalog::new(Arc::new(crate::mcp::McpManager::new()), &config, &crate::config::AgentConfig::default());
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
            .finish("openai", &mut repeated_failures, &mut |event| {
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

    #[tokio::test]
    async fn scheduler_background_task_uses_shared_semaphore() {
        let config = crate::config::AppConfig::default();
        let catalog = ToolCatalog::new(Arc::new(crate::mcp::McpManager::new()), &config, &crate::config::AgentConfig::default());
        let snapshot = catalog.snapshot(&ToolExecutionContext::default()).await;
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let conversation_id = "conv-runtime-awaiter-test";
        // Serial mode (parallelism=1): background_task now shares the same
        // semaphore as other tools since it's a single native tool, not a
        // separate control-plane partition.
        let mut scheduler =
            ToolExecutionScheduler::new(conversation_id, snapshot, false, &cancel_rx);
        let job = background_jobs::start_job_for_conversation(
            "slow-test-tool",
            Some(conversation_id.to_string()),
        );
        let job_id = background_jobs::job_id(&job);

        assert!(scheduler.schedule_pending_tool(
            ProviderPendingToolCall {
                call_id: "call-wait".to_string(),
                name: "background_task".to_string(),
                arguments: json!({ "action": "wait", "jobId": job_id, "timeoutMs": 600 }),
            },
            &HashMap::new(),
        ));
        assert!(scheduler.schedule_pending_tool(
            ProviderPendingToolCall {
                call_id: "call-calculator".to_string(),
                name: "calculator".to_string(),
                arguments: json!({ "expression": "21 * 2" }),
            },
            &HashMap::new(),
        ));

        // In serial mode with a shared semaphore, the background_task wait
        // holds the only permit until it times out (600ms). The calculator
        // will eventually run after the wait releases its permit.
        let mut events = Vec::new();
        let deadline = Instant::now() + Duration::from_millis(1000);
        while Instant::now() < deadline {
            scheduler
                .try_emit_completed_tools(&mut |event| {
                    events.push(event);
                    Ok(())
                })
                .expect("drain completed tools");
            if events.iter().filter(|event| {
                matches!(event, ProviderStreamEvent::ToolCallExecuted { .. })
            }).count() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let mut repeated_failures = HashMap::new();
        let batch = scheduler
            .finish("openai", &mut repeated_failures, &mut |event| {
                events.push(event);
                Ok(())
            })
            .await
            .expect("finish scheduled tools");

        let _ = background_jobs::cancel_job(&job_id, Some(conversation_id));
        assert_eq!(batch.tool_results_items.len(), 2);
    }
}
