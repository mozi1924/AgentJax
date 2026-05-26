use super::tool_parsing::{
    is_valid_pending_tool_call, parse_tool_arguments, push_or_update_pending_tool_call,
};
use crate::config::AppConfig;
use crate::providers::types::{
    ProviderPendingToolCall, ProviderStreamEvent, ResponseStreamRequest, ResponseStreamResult,
};
use serde_json::Value;
use std::collections::HashMap;
use tokio::sync::watch;

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
) -> Result<CollectedProviderTurn, String>
where
    F: FnMut(ProviderStreamEvent) -> Result<(), String> + Send,
{
    let mut active_tool_calls_in_turn: HashMap<String, (String, Value)> = HashMap::new();
    let mut tool_args_delta_by_call: HashMap<String, String> = HashMap::new();
    let mut pending_tools_from_events: Vec<ProviderPendingToolCall> = Vec::new();

    let response_result =
        crate::providers::stream_response(config, stream_request, cancel_rx, |event| {
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
                    active_tool_calls_in_turn
                        .insert(call_id.clone(), (name.clone(), parsed_args.clone()));
                    push_or_update_pending_tool_call(
                        &mut pending_tools_from_events,
                        call_id.clone(),
                        name.clone(),
                        parsed_args,
                    );
                }
                _ => {}
            }
            on_event(event)
        })
        .await?;

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
