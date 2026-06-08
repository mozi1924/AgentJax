//! Lightweight tool-calling loop for simple agents.
//!
//! Unlike the full `AgentRuntime::run_turn()`, this loop has no LCM
//! persistence, no MCP management, and no frontend event streaming.
//! It is designed for background agents (e.g., memory agent) that need
//! a minimal tool-calling loop with a fixed set of tools.
//!
//! # Flow
//!
//! 1. System prompt + context items → provider (with tools declared)
//! 2. Provider responds with text and/or tool calls
//! 3. Execute tool calls directly
//! 4. Feed results back to provider
//! 5. Repeat up to `max_turns` times
//! 6. Return final result when no more tool calls

use crate::config::{AgentConfig, AppConfig};
use crate::error::AgentJaxResult;
use crate::provider_api::types::ResponseStreamRequest;
use serde_json::Value;
use tokio::sync::watch;

/// Execute one turn of a lightweight tool-calling loop.
///
/// The loop ends when the model produces no more tool calls, or when
/// `max_turns` is reached.
///
/// # Parameters
/// - `system_prompt`: System instruction for the agent.
/// - `context_items`: Conversation history or other context items.
/// - `tool_definitions`: Tool schemas to provide to the model.
/// - `tool_handlers`: Map of tool name → async handler function.
/// - `model_id`: The model to use.
/// - `max_turns`: Maximum iterations (default 3).
/// - `app_config`: Application configuration.
/// - `agent_config`: Agent configuration (for provider resolution).
///
/// # Returns
/// The model's final output text.
#[allow(clippy::too_many_arguments)]
pub async fn run_tool_loop(
    system_prompt: &str,
    context_items: Vec<Value>,
    tool_definitions: Vec<Value>,
    tool_handlers: Vec<Box<dyn ToolHandler>>,
    model_id: &str,
    max_turns: usize,
    app_config: &AppConfig,
    agent_config: &AgentConfig,
) -> AgentJaxResult<String> {
    let (_cancel_tx, mut cancel_rx) = watch::channel(false);

    let handler_map: std::collections::HashMap<String, Box<dyn ToolHandler>> = tool_handlers
        .into_iter()
        .map(|h| (h.name().to_string(), h))
        .collect();

    let mut accumulated_output = String::new();
    let mut accumulated_context: Vec<Value> = Vec::new();

    for turn in 0..max_turns {
        // Build input items for this turn.
        let input_items = if turn == 0 {
            let mut items = vec![serde_json::json!({
                "role": "system",
                "content": [{"type": "input_text", "text": system_prompt}]
            })];
            items.extend(context_items.clone());
            items
        } else {
            accumulated_context.clone()
        };

        let request = ResponseStreamRequest {
            input_items,
            model: Some(model_id.to_string()),
            tools: Some(tool_definitions.clone()),
            tool_choice: None,
            ..Default::default()
        };

        let response = crate::provider_api::stream_response(
            app_config,
            agent_config,
            &request,
            &mut cancel_rx,
            |_event| Ok(()),
        )
        .await?;

        let response_text = response.output_text.trim().to_string();
        let tool_calls = extract_tool_calls(&response.output_items);

        if tool_calls.is_empty() {
            // No tool calls → this is the final answer.
            if !response_text.is_empty() {
                accumulated_output = response_text;
            }
            break;
        }

        // Execute tool calls and build continuation.
        let mut continuation = response.output_items.clone();
        for tc in &tool_calls {
            let result_item = if let Some(handler) = handler_map.get(&tc.name) {
                let output = handler.execute(&tc.arguments).await;
                serde_json::json!({
                    "type": "function_call_output",
                    "call_id": tc.call_id,
                    "output": output,
                })
            } else {
                let output_payload = serde_json::json!({
                    "ok": false,
                    "tool": tc.name,
                    "error": {
                        "message": format!("unknown tool '{}'", tc.name),
                    }
                });
                serde_json::json!({
                    "type": "function_call_output",
                    "call_id": tc.call_id,
                    "output": serde_json::to_string(&output_payload).unwrap_or_default(),
                })
            };
            continuation.push(result_item);
        }

        if turn == 0 {
            accumulated_context = continuation;
        } else {
            accumulated_context.extend(continuation);
        }
    }

    Ok(accumulated_output)
}

// ── Tool handler trait ────────────────────────────────────────────────────────

/// A handler for a single tool in the lightweight tool loop.
#[async_trait::async_trait]
pub trait ToolHandler: Send + Sync {
    fn name(&self) -> &str;
    async fn execute(&self, arguments: &str) -> String;
}

// ── Tool call extraction ──────────────────────────────────────────────────────

struct ToolCallInfo {
    call_id: String,
    name: String,
    arguments: String,
}

fn extract_tool_calls(output_items: &[Value]) -> Vec<ToolCallInfo> {
    output_items
        .iter()
        .filter(|item| item.get("type").and_then(|v| v.as_str()) == Some("function_call"))
        .map(|item| ToolCallInfo {
            call_id: item
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            name: item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            arguments: item
                .get("arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("{}")
                .to_string(),
        })
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_tool_calls_empty() {
        let items: Vec<Value> = vec![
            json!({"type": "text", "text": "Hello"}),
            json!({"type": "reasoning", "text": "Thinking..."}),
        ];
        let calls = extract_tool_calls(&items);
        assert!(calls.is_empty());
    }

    #[test]
    fn test_extract_tool_calls_finds_function_calls() {
        let items: Vec<Value> = vec![
            json!({"type": "text", "text": "Let me check..."}),
            json!({
                "type": "function_call",
                "call_id": "call_1",
                "name": "memory_search",
                "arguments": "{\"query\": \"test\"}"
            }),
        ];
        let calls = extract_tool_calls(&items);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "memory_search");
        assert_eq!(calls[0].call_id, "call_1");
    }
}
