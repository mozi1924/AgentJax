use serde_json::{Value, json};

use crate::providers::core::ProviderIdFactory;
use crate::providers::sse::sse_data_payload;
use crate::providers::types::{ProviderEventSink, ProviderStreamEvent, ProviderUsage};

#[derive(Debug, Clone)]
struct GeminiToolCall {
    item_id: String,
    call_id: String,
    name: String,
    args: Value,
}

/// Parse one Gemini SSE block into AgentJax provider stream events.
#[allow(clippy::too_many_arguments)]
pub(super) fn process_gemini_event(
    block: &str,
    response_id: &str,
    output_text: &mut String,
    output_items: &mut Vec<Value>,
    usage: &mut Option<ProviderUsage>,
    emitted_output_started: &mut bool,
    id_factory: &mut ProviderIdFactory,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<(), String> {
    let Some(payload) = sse_data_payload(block) else {
        return Ok(());
    };
    if payload == "[DONE]" || payload.trim().is_empty() {
        return Ok(());
    }

    let value: Value = serde_json::from_str(&payload).map_err(|err| {
        format!(
            "Failed to parse Gemini streaming event: {err}. body={}",
            preview(&payload)
        )
    })?;
    if let Some(error) = value.get("error") {
        return Err(format!("Gemini streaming error: {error}"));
    }
    if let Some(next_usage) = ProviderUsage::from_api_value(
        value
            .get("usageMetadata")
            .or_else(|| value.get("usage_metadata"))
            .unwrap_or(&value),
    ) {
        *usage = Some(next_usage);
    }

    let Some(candidates) = value.get("candidates").and_then(Value::as_array) else {
        return Ok(());
    };
    for candidate in candidates {
        let parts = candidate
            .get("content")
            .and_then(|content| content.get("parts"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        for part in parts {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                if !text.is_empty() {
                    if !*emitted_output_started {
                        *emitted_output_started = true;
                        on_delta(ProviderStreamEvent::OutputTextStarted)?;
                    }
                    output_text.push_str(text);
                    on_delta(ProviderStreamEvent::OutputTextDelta {
                        delta: text.to_string(),
                        phase: None,
                    })?;
                }
            }

            if let Some(function_call) = part.get("functionCall") {
                let tool_call = normalize_gemini_tool_call(function_call, id_factory)?;
                emit_gemini_tool_call(&tool_call, output_items, on_delta)?;
            }
        }
    }

    let _ = response_id;
    Ok(())
}

fn normalize_gemini_tool_call(
    function_call: &Value,
    id_factory: &mut ProviderIdFactory,
) -> Result<GeminiToolCall, String> {
    let name = function_call
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if name.trim().is_empty() {
        return Err("Gemini functionCall is missing name".to_string());
    }
    let args = function_call
        .get("args")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let item_id = id_factory.next_item_id(&name);
    let call_id = id_factory.next_call_id(&name);

    Ok(GeminiToolCall {
        item_id,
        call_id,
        name,
        args,
    })
}

fn emit_gemini_tool_call(
    tool_call: &GeminiToolCall,
    output_items: &mut Vec<Value>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<(), String> {
    let arguments = serde_json::to_string(&tool_call.args)
        .map_err(|err| format!("Failed to serialize Gemini tool call args: {err}"))?;
    on_delta(ProviderStreamEvent::ToolCallStarted {
        item_id: tool_call.item_id.clone(),
        call_id: tool_call.call_id.clone(),
        name: tool_call.name.clone(),
        presentation: None,
    })?;
    on_delta(ProviderStreamEvent::ToolCallCompleted {
        item_id: tool_call.item_id.clone(),
        call_id: tool_call.call_id.clone(),
        name: tool_call.name.clone(),
        arguments: arguments.clone(),
        presentation: None,
    })?;
    output_items.push(json!({
        "type": "function_call",
        "id": tool_call.item_id,
        "call_id": tool_call.call_id,
        "name": tool_call.name,
        "arguments": arguments
    }));
    Ok(())
}

fn preview(raw: &str) -> String {
    const MAX: usize = 400;
    if raw.len() <= MAX {
        raw.to_string()
    } else {
        format!("{}...[truncated]", &raw[..MAX])
    }
}
