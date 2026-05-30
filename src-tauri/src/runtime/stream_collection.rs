use super::tool_execution::{TOOL_PROGRESS_HEARTBEAT_SECS, ToolExecutionScheduler};
use super::tool_parsing::{
    is_valid_pending_tool_call, parse_tool_arguments, push_or_update_pending_tool_call,
};
use crate::config::AppConfig;
use crate::providers::types::{
    ProviderPendingToolCall, ProviderStreamEvent, ResponseStreamRequest, ResponseStreamResult,
};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

pub(super) struct CollectedProviderTurn {
    pub response_result: ResponseStreamResult,
    pub pending_tools: Vec<ProviderPendingToolCall>,
}

pub(super) async fn collect_provider_turn<F>(
    config: &AppConfig,
    provider_kind: &str,
    provider_key_for_log: &str,
    stream_request: &ResponseStreamRequest,
    cancel_rx: &mut watch::Receiver<bool>,
    on_event: &mut F,
    tool_scheduler: Option<&mut ToolExecutionScheduler>,
    repeated_failed_tool_signatures: &HashMap<String, usize>,
) -> Result<CollectedProviderTurn, String>
where
    F: FnMut(ProviderStreamEvent) -> Result<(), String> + Send,
{
    let mut active_tool_calls_in_turn: HashMap<String, (String, Value)> = HashMap::new();
    let mut tool_args_delta_by_call: HashMap<String, String> = HashMap::new();
    let mut pending_tools_from_events: Vec<ProviderPendingToolCall> = Vec::new();
    let mut tool_scheduler = tool_scheduler;
    let (provider_event_tx, mut provider_event_rx) =
        mpsc::unbounded_channel::<ProviderStreamEvent>();

    // Run provider streaming on its own task so this foreground collector can
    // keep draining tool completions and heartbeat ticks while the model
    // continues emitting text or other tool calls.
    let provider_config = config.clone();
    let provider_request = stream_request.clone();
    let mut provider_cancel_rx = cancel_rx.clone();
    let mut provider_task = tokio::spawn(async move {
        crate::providers::stream_response(
            &provider_config,
            &provider_request,
            &mut provider_cancel_rx,
            |event| {
                provider_event_tx
                    .send(event)
                    .map_err(|_| "Provider stream event receiver dropped".to_string())
            },
        )
        .await
    });

    let mut response_result: Option<ResponseStreamResult> = None;
    let mut provider_done = false;
    let mut provider_events_closed = false;
    let mut progress_interval =
        tokio::time::interval(Duration::from_secs(TOOL_PROGRESS_HEARTBEAT_SECS));
    progress_interval.tick().await;

    while !provider_done || !provider_events_closed {
        tokio::select! {
            maybe_event = provider_event_rx.recv(), if !provider_events_closed => {
                match maybe_event {
                    Some(event) => {
                        handle_provider_stream_event(
                            event,
                            &mut active_tool_calls_in_turn,
                            &mut tool_args_delta_by_call,
                            &mut pending_tools_from_events,
                            &mut tool_scheduler,
                            repeated_failed_tool_signatures,
                            on_event,
                        )?;
                        drain_tool_scheduler_events(&mut tool_scheduler, on_event)?;
                    }
                    None => {
                        provider_events_closed = true;
                    }
                }
            }
            joined = &mut provider_task, if !provider_done => {
                provider_done = true;
                let joined = joined.map_err(|err| format!("Provider stream task failed to join: {err}"))?;
                response_result = Some(joined?);
                drain_tool_scheduler_events(&mut tool_scheduler, on_event)?;
            }
            _ = progress_interval.tick(), if tool_scheduler.as_ref().map(|scheduler| scheduler.has_active_tools()).unwrap_or(false) => {
                drain_tool_scheduler_events(&mut tool_scheduler, on_event)?;
                if let Some(scheduler) = tool_scheduler.as_deref_mut() {
                    scheduler.emit_progress_events(on_event)?;
                }
            }
        }
    }

    let response_result =
        response_result.ok_or_else(|| "Provider stream ended without a response".to_string())?;

    let event_pending_total = pending_tools_from_events.len();
    let mut pending_tools: Vec<ProviderPendingToolCall> = pending_tools_from_events
        .into_iter()
        .filter(is_valid_pending_tool_call)
        .collect();
    let has_invalid_event_pending = pending_tools.len() != event_pending_total;

    if pending_tools.is_empty() || has_invalid_event_pending {
        let extracted_pending = crate::providers::extract_pending_tool_calls(
            provider_kind,
            &response_result.output_items,
        )?;
        if has_invalid_event_pending {
            log::warn!(
                "Provider '{}' emitted incomplete tool-call events; merged fallback extraction from output items",
                provider_key_for_log
            );
        }
        if pending_tools.is_empty() && !extracted_pending.is_empty() {
            log::debug!(
                "Tool-call fallback path used for provider '{}': extracted {} calls from output items",
                provider_key_for_log,
                extracted_pending.len()
            );
        }
        for extracted in extracted_pending {
            if !is_valid_pending_tool_call(&extracted) {
                continue;
            }
            if pending_tools
                .iter()
                .any(|existing| existing.call_id == extracted.call_id)
            {
                continue;
            }
            pending_tools.push(extracted);
        }
    }

    Ok(CollectedProviderTurn {
        response_result,
        pending_tools,
    })
}

fn handle_provider_stream_event<F>(
    event: ProviderStreamEvent,
    active_tool_calls_in_turn: &mut HashMap<String, (String, Value)>,
    tool_args_delta_by_call: &mut HashMap<String, String>,
    pending_tools_from_events: &mut Vec<ProviderPendingToolCall>,
    tool_scheduler: &mut Option<&mut ToolExecutionScheduler>,
    repeated_failed_tool_signatures: &HashMap<String, usize>,
    on_event: &mut F,
) -> Result<(), String>
where
    F: FnMut(ProviderStreamEvent) -> Result<(), String> + Send,
{
    match &event {
        ProviderStreamEvent::ToolCallStarted { call_id, name, .. } => {
            active_tool_calls_in_turn.insert(call_id.clone(), (name.clone(), Value::Null));
        }
        ProviderStreamEvent::ToolCallArgumentsDelta { call_id, delta, .. } => {
            let entry = tool_args_delta_by_call.entry(call_id.clone()).or_default();
            entry.push_str(delta);
        }
        ProviderStreamEvent::ToolCallCompleted {
            call_id,
            name,
            arguments,
            ..
        } => {
            let parsed_args = parse_tool_arguments(
                arguments,
                tool_args_delta_by_call.get(call_id).map(String::as_str),
            );
            active_tool_calls_in_turn.insert(call_id.clone(), (name.clone(), parsed_args.clone()));
            let pending_tool = ProviderPendingToolCall {
                call_id: call_id.clone(),
                name: name.clone(),
                arguments: parsed_args.clone(),
            };
            push_or_update_pending_tool_call(
                pending_tools_from_events,
                pending_tool.call_id.clone(),
                pending_tool.name.clone(),
                pending_tool.arguments.clone(),
            );
            if is_valid_pending_tool_call(&pending_tool) {
                if let Some(scheduler) = tool_scheduler.as_deref_mut() {
                    scheduler.schedule_pending_tool(pending_tool, repeated_failed_tool_signatures);
                }
            }
        }
        _ => {}
    }

    on_event(event)
}

fn drain_tool_scheduler_events<F>(
    tool_scheduler: &mut Option<&mut ToolExecutionScheduler>,
    on_event: &mut F,
) -> Result<(), String>
where
    F: FnMut(ProviderStreamEvent) -> Result<(), String> + Send,
{
    if let Some(scheduler) = tool_scheduler.as_deref_mut() {
        scheduler.try_emit_completed_tools(on_event)?;
    }
    Ok(())
}
