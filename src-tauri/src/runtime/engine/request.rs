use crate::commands::chat::ChatRequest;
use crate::provider_api::types::ResponseStreamRequest;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

#[cfg(test)]
pub(super) fn build_base_context(
    system_items: Vec<Value>,
    recovery_note: Option<Value>,
    context_items: Vec<Value>,
    current_user_item: Value,
) -> Vec<Value> {
    let mut base_context = Vec::new();
    base_context.extend(system_items);
    if let Some(note_item) = recovery_note {
        base_context.push(note_item);
    }
    base_context.extend(context_items);
    base_context.push(current_user_item);
    base_context
}

pub(super) fn build_request(
    req: &ChatRequest,
    input_items: Vec<Value>,
    tools_schemas: Vec<Value>,
) -> ResponseStreamRequest {
    ResponseStreamRequest {
        input_items,
        model: req.model.clone(),
        reasoning: req.reasoning.clone(),
        instructions_override: None,
        text: req.text.clone(),
        include: req.include.clone(),
        service_tier: req.service_tier.clone(),
        prompt_cache_key: req.prompt_cache_key.clone(),
        client_metadata: req.client_metadata.clone(),
        generate: req.generate,
        tools: Some(tools_schemas),
        tool_choice: Some(serde_json::Value::String("auto".to_string())),
        temperature: req.temperature,
        top_p: req.top_p,
        presence_penalty: req.presence_penalty,
        frequency_penalty: req.frequency_penalty,
        max_tokens: req.max_tokens,
        max_completion_tokens: req.max_completion_tokens,

        // Extra body is merged later from ResolvedModelConfig.request.extra_body
        // in provider_api::stream_response(). The ChatRequest does not carry it.
        extra_body: BTreeMap::new(),
        skip_model_extra_body: false,
    }
}

/// Reinsert missing function_call items before paired outputs when providers
/// return only output items in continuation batches.
pub(super) fn ensure_tool_call_output_pairs(
    items: Vec<Value>,
    executed_tool_call_items: &[Value],
) -> Vec<Value> {
    let existing_call_ids: HashSet<String> = items
        .iter()
        .filter_map(|item| match item.get("type").and_then(Value::as_str) {
            Some("function_call") | Some("custom_tool_call") => item
                .get("call_id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            _ => None,
        })
        .collect();

    let mut missing_call_by_id: HashMap<String, Value> = HashMap::new();
    for call_item in executed_tool_call_items {
        let Some(call_id) = call_item.get("call_id").and_then(Value::as_str) else {
            continue;
        };
        if existing_call_ids.contains(call_id) {
            continue;
        }
        missing_call_by_id.insert(call_id.to_string(), call_item.clone());
    }

    if missing_call_by_id.is_empty() {
        return items;
    }

    let mut stitched = Vec::with_capacity(items.len() + missing_call_by_id.len());
    let mut inserted: HashSet<String> = HashSet::new();
    for item in items {
        if matches!(
            item.get("type").and_then(Value::as_str),
            Some("function_call_output") | Some("custom_tool_call_output")
        ) && let Some(call_id) = item.get("call_id").and_then(Value::as_str)
            && let Some(missing_call) = missing_call_by_id.get(call_id)
            && !inserted.contains(call_id)
        {
            stitched.push(missing_call.clone());
            inserted.insert(call_id.to_string());
        }
        stitched.push(item);
    }

    for (call_id, missing_call) in missing_call_by_id {
        if !inserted.contains(&call_id) {
            stitched.push(missing_call);
        }
    }

    stitched
}
