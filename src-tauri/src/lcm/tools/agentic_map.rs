//! `agentic_map` — parallel sub-agent processing over a JSONL file.
//!
//! Implements the Agentic-Map operator from LCM §3.1 (Figure 4).
//!
//! Spawns a sub-agent session for each item in a JSONL input file.
//! Each sub-agent has access to filesystem tools and can perform
//! multi-step reasoning. Unlike `llm_map` (stateless LLM calls),
//! `agentic_map` runs a proper tool-using agent loop per item.
//!
//! ## Usage
//!
//! ```text
//! 1. Model writes input data as a JSONL file
//! 2. Model calls agentic_map with: inputPath, prompt, outputPath
//! 3. Engine spawns sub-agent sessions in parallel (concurrent workers)
//! 4. Each sub-agent runs a tool-using agent loop (max 5 turns)
//! 5. Engine writes results to outputPath as JSONL
//! 6. Model reads the output file
//! ```

use crate::config::AppConfig;
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::tools::{Tool, ToolExecutionContext, ToolRegistry};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Semaphore;

// ── Arguments ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgenticMapArgs {
    /// Path to the JSONL input file (one JSON object per line).
    input_path: String,
    /// Instructions for each sub-agent. Use `{input}` for the item data.
    /// Example: "Analyze the following code and find bugs:\n{input}"
    prompt: String,
    /// Path to write the JSONL output file.
    output_path: String,
    /// Maximum number of concurrent sub-agents (default 4).
    #[serde(default = "default_concurrency")]
    concurrency: usize,
    /// Maximum retries per item (default 2).
    #[serde(default = "default_max_retries")]
    max_retries: u32,
}

fn default_concurrency() -> usize {
    4
}

fn default_max_retries() -> u32 {
    2
}

// ── Tool ────────────────────────────────────────────────────────────────────

pub struct AgenticMapTool;

#[async_trait::async_trait]
impl Tool for AgenticMapTool {
    fn name(&self) -> &'static str {
        "agentic_map"
    }

    fn description(&self) -> &'static str {
        "Spawn sub-agent sessions for each item in a JSONL file, in parallel. \
         Each sub-agent has full tool access (file I/O, code execution, etc.) \
         and processes one item autonomously. Results are written to a JSONL \
         output file.\n\n\
         Unlike llm_map which uses stateless LLM calls, agentic_map creates \
         actual sub-agent sessions that can use tools and perform multi-step \
         reasoning. This is suitable for complex per-item tasks like code \
         review, data analysis, or multi-step transformations.\n\n\
         The engine handles all concurrency, retries, and result collection \
         deterministically (LCM §3.1, Figure 4)."
    }

    fn display_name(&self) -> &'static str {
        "Agentic Map"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("GitFork")
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "inputPath": {
                    "type": "string",
                    "description": "Path to the JSONL input file (one JSON object per line)."
                },
                "prompt": {
                    "type": "string",
                    "description": "Instructions for each sub-agent. Use {input} for the item data."
                },
                "outputPath": {
                    "type": "string",
                    "description": "Path to write the JSONL output file."
                },
                "concurrency": {
                    "type": "integer",
                    "description": "Maximum concurrent sub-agents (default 4).",
                    "default": 4
                },
                "maxRetries": {
                    "type": "integer",
                    "description": "Maximum retries per item (default 2).",
                    "default": 2
                }
            },
            "required": ["inputPath", "prompt", "outputPath"]
        })
    }

    async fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> AgentJaxResult<Value> {
        let args: AgenticMapArgs = serde_json::from_value(arguments.clone())
            .map_err(|e| AgentJaxError::tool(format!("Invalid arguments for agentic_map: {e}")))?;

        let workspace_dir = get_workspace_dir(context)?;
        let input_path = workspace_dir.join(&args.input_path);
        let output_path = workspace_dir.join(&args.output_path);

        if !input_path.exists() {
            return Err(AgentJaxError::not_found(format!(
                "Input file not found: {}",
                input_path.display()
            )));
        }

        let input_content = std::fs::read_to_string(&input_path)
            .map_err(|e| AgentJaxError::internal(format!("Failed to read input file: {e}")))?;

        let input_lines: Vec<String> = input_content.lines().filter(|l| !l.trim().is_empty()).map(|s| s.to_string()).collect();
        if input_lines.is_empty() {
            return Err(AgentJaxError::tool("Input file is empty".to_string()));
        }

        let total = input_lines.len();
        let model_ref = context.model_id.clone().unwrap_or_else(|| "default".to_string());
        let app_config = context.app_config.clone();
        let prompt_template = args.prompt;
        let concurrency = args.concurrency.max(1);
        let max_retries = args.max_retries;

        let completed = Arc::new(AtomicUsize::new(0));
        let failed = Arc::new(AtomicUsize::new(0));
        let semaphore = Arc::new(Semaphore::new(concurrency));
        let results: Arc<std::sync::Mutex<Vec<(usize, AgentJaxResult<Value>)>>> =
            Arc::new(std::sync::Mutex::new(Vec::with_capacity(total)));

        // Spawn all sub-agent tasks with concurrency control.
        let mut handles = Vec::with_capacity(total);

        for (i, line) in input_lines.iter().enumerate() {
            let permit = semaphore.clone().acquire_owned();
            let prompt = prompt_template.replace("{input}", line);
            let model = model_ref.clone();
            let comp = completed.clone();
            let fail = failed.clone();
            let res = results.clone();
            let retries = max_retries;
            let cfg = app_config.clone();

            handles.push(tokio::spawn(async move {
                let _permit = permit.await.unwrap();

                let mut last_error: Option<AgentJaxError> = None;
                for attempt in 0..retries {
                    if attempt > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                    }
                    match run_subagent_task(&prompt, &model, cfg.as_deref()).await {
                        Ok(output) => {
                            comp.fetch_add(1, Ordering::SeqCst);
                            let mut lock = res.lock().unwrap();
                            lock.push((i, Ok(output)));
                            return;
                        }
                        Err(e) => {
                            last_error = Some(e);
                        }
                    }
                }
                fail.fetch_add(1, Ordering::SeqCst);
                let mut lock = res.lock().unwrap();
                lock.push((i, Err(last_error.unwrap_or_else(|| AgentJaxError::internal("Max retries exceeded")))));
            }));
        }

        // Await all spawned tasks.
        for handle in handles {
            let _ = handle.await;
        }

        let final_results = {
            let mut lock = results.lock().unwrap();
            lock.sort_by_key(|(i, _)| *i);
            lock.clone()
        };

        let mut output_lines = Vec::new();
        for (_i, result) in &final_results {
            match result {
                Ok(val) => {
                    output_lines.push(serde_json::to_string(val).unwrap_or_default());
                }
                Err(e) => {
                    output_lines.push(serde_json::to_string(&json!({
                        "error": e
                    })).unwrap_or_default());
                }
            }
        }

        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AgentJaxError::internal(format!("Failed to create output directory: {e}")))?;
        }
        std::fs::write(&output_path, output_lines.join("\n"))
            .map_err(|e| AgentJaxError::internal(format!("Failed to write output file: {e}")))?;

        let completed_count = completed.load(Ordering::SeqCst);
        let failed_count = failed.load(Ordering::SeqCst);

        Ok(json!({
            "status": if failed_count == 0 { "success" } else { "partial" },
            "totalItems": total,
            "completedItems": completed_count,
            "failedItems": failed_count,
            "outputPath": args.output_path,
            "summary": format!(
                "Processed {total} items: {completed_count} succeeded, {failed_count} failed. Output written to {}",
                args.output_path
            ),
        }))
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn get_workspace_dir(context: &ToolExecutionContext) -> AgentJaxResult<PathBuf> {
    let conv_id = context.conversation_id.as_deref().ok_or_else(|| {
        AgentJaxError::tool("agentic_map requires a conversation context".to_string())
    })?;
    let path = crate::conversation_store::conversation_workspace_path(conv_id)
        .map_err(|e| AgentJaxError::internal(format!("Failed to resolve workspace: {e}")))?;
    Ok(path)
}

/// Run a single sub-agent task with filesystem tool access.
///
/// This implements a proper tool-using agent loop: the sub-agent can call
/// tools to read, write, and list files, and the results are fed back for
/// further processing. The loop runs for up to `MAX_SUBAGENT_TURNS` iterations.
async fn run_subagent_task(
    prompt: &str,
    model_ref: &str,
    app_config: Option<&AppConfig>,
) -> crate::error::AgentJaxResult<Value> {
    use crate::provider_api::types::{
        ProviderPendingToolCall, ProviderStreamEvent, ResponseStreamRequest,
    };
    use tokio::sync::watch;

    let config = match app_config {
        Some(c) => c.clone(),
        None => crate::config::load_config()
            .map_err(|e| format!("Failed to load config: {e}"))?,
    };

    let tool_registry = ToolRegistry::new_with_defaults();
    let tool_schemas = tool_registry.list_schemas();
    let tool_context = ToolExecutionContext::default();

    let subagent_instructions = "\
You are a sub-agent processing one item from a batch. \
You have access to filesystem tools (read, write, list, edit files). \
Complete your task and output ONLY the final result as a JSON object. \
Do not include any other text, explanation, or markdown outside the JSON.";

    // Start with the user prompt as the initial input.
    let mut input_items: Vec<Value> = vec![json!({
        "role": "user",
        "content": [{"type": "input_text", "text": prompt}]
    })];

    const MAX_SUBAGENT_TURNS: usize = 5;

    for _turn in 0..MAX_SUBAGENT_TURNS {
        let (_cancel_tx, mut cancel_rx) = watch::channel(false);

        let request = ResponseStreamRequest {
            input_items: input_items.clone(),
            model: Some(model_ref.to_string()),
            reasoning_effort: None,
            instructions_override: Some(subagent_instructions.to_string()),
            text: None,
            include: None,
            service_tier: None,
            prompt_cache_key: None,
            client_metadata: None,
            generate: None,
            tools: Some(tool_schemas.clone()),
            tool_choice: Some(json!("auto")),
        };

        // Collect tool calls from the stream.
        let mut tool_calls: Vec<ProviderPendingToolCall> = Vec::new();
        let mut assistant_text = String::new();

        let response = crate::provider_api::stream_response(
            &config,
            &request,
            &mut cancel_rx,
            |event| {
                if let ProviderStreamEvent::ToolCallStarted { call_id, name, .. } = &event {
                    tool_calls.push(ProviderPendingToolCall {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        arguments: Value::Null,
                    });
                }
                if let ProviderStreamEvent::ToolCallCompleted { call_id, arguments, .. } = &event {
                    if let Some(tc) = tool_calls.iter_mut().find(|t| &t.call_id == call_id) {
                        tc.arguments = serde_json::from_str(arguments).unwrap_or(Value::Null);
                    }
                }
                if let ProviderStreamEvent::AssistantMessageCompleted { text, .. } = &event {
                    assistant_text = text.clone();
                }
                Ok(())
            },
        )
        .await
        .map_err(|e| format!("Sub-agent LLM call failed: {e}"))?;

        // If no tool calls, this is the final response.
        if tool_calls.is_empty() {
            let text = response.output_text.trim().to_string();
            if text.is_empty() {
                return Err(AgentJaxError::tool("Sub-agent returned empty response"));
            }
            if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                return Ok(parsed);
            }
            return Ok(json!({ "result": text }));
        }

        // Add assistant message (with tool calls) to the conversation.
        let assistant_item = json!({
            "role": "assistant",
            "content": assistant_text,
        });
        // Some providers embed tool calls differently; we add them as
        // separate function_call items following the assistant message.
        input_items.push(assistant_item);

        // Execute each tool call and feed results back to the conversation.
        for tc in &tool_calls {
            // Add function_call item.
            input_items.push(json!({
                "type": "function_call",
                "call_id": tc.call_id,
                "name": tc.name,
                "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default(),
            }));

            let exec_result = tool_registry
                .execute(&tc.name, &tc.arguments, &tool_context)
                .await;

            match exec_result {
                Ok(output) => {
                    let output_str = serde_json::to_string(&output).unwrap_or_default();
                    input_items.push(json!({
                        "type": "function_call_output",
                        "call_id": tc.call_id,
                        "output": output_str,
                    }));
                }
                Err(e) => {
                    let err_str = format!("Tool execution error: {e}");
                    input_items.push(json!({
                        "type": "function_call_output",
                        "call_id": tc.call_id,
                        "output": err_str,
                    }));
                }
            }
        }
    }

    Err(AgentJaxError::tool(format!(
        "Sub-agent exceeded maximum turns ({MAX_SUBAGENT_TURNS}) without completing"
    )))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_deserialize_args() {
        let args = json!({
            "inputPath": "/tmp/items.jsonl",
            "prompt": "Analyze: {input}",
            "outputPath": "/tmp/results.jsonl"
        });
        let parsed: AgenticMapArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.concurrency, 4);
        assert_eq!(parsed.max_retries, 2);
    }

    #[test]
    fn test_deserialize_args_with_overrides() {
        let args = json!({
            "inputPath": "/tmp/items.jsonl",
            "prompt": "Analyze: {input}",
            "outputPath": "/tmp/results.jsonl",
            "concurrency": 8,
            "maxRetries": 5
        });
        let parsed: AgenticMapArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.concurrency, 8);
        assert_eq!(parsed.max_retries, 5);
    }

    #[test]
    fn test_get_workspace_dir_missing_context() {
        let ctx = ToolExecutionContext::default();
        let result = get_workspace_dir(&ctx);
        assert!(result.is_err());
    }
}
