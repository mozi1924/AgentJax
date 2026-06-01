//! `llm_map` — parallel LLM processing over a JSONL file.
//!
//! Implements the LLM-Map operator from LCM §3.1 (Figure 4).
//!
//! Applies a prompt to every item in a JSONL input file, in parallel.
//! Each item is processed as a single, stateless LLM call. Results are
//! validated against a JSON Schema and written to a JSONL output file.
//!
//! ## Usage
//!
//! ```text
//! 1. Model writes input data as a JSONL file (one item per line)
//! 2. Model calls llm_map with: inputPath, prompt, outputSchema, outputPath
//! 3. Engine processes items in parallel (concurrent workers)
//! 4. Engine writes results to outputPath as JSONL
//! 5. Model reads the output file to get results
//! ```

use crate::error::{AgentJaxError, AgentJaxResult};
use crate::provider_api::retry::{RetryResult, RetryStrategy, retry_with_backoff};
use crate::tools::{Tool, ToolExecutionContext};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Semaphore;

// ── Arguments ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LlmMapArgs {
    /// Path to the JSONL input file (one JSON object per line).
    input_path: String,
    /// Prompt template. Use `{input}` as a placeholder for each item.
    /// Example: "Classify this text: {input}\nCategories: positive, negative"
    prompt: String,
    /// JSON Schema for validating each output item.
    output_schema: Value,
    /// Path to write the JSONL output file.
    output_path: String,
    /// Maximum number of concurrent workers (default 16).
    #[serde(default = "default_concurrency")]
    concurrency: usize,
    /// Maximum retries per item (default 3).
    #[serde(default = "default_max_retries")]
    max_retries: u32,
}

fn default_concurrency() -> usize {
    16
}

fn default_max_retries() -> u32 {
    3
}

// ── Item Status ─────────────────────────────────────────────────────────────

// ── Tool ────────────────────────────────────────────────────────────────────

pub struct LlmMapTool;

#[async_trait::async_trait]
impl Tool for LlmMapTool {
    fn name(&self) -> &'static str {
        "llm_map"
    }

    fn description(&self) -> &'static str {
        "Apply a prompt to every item in a JSONL file, in parallel. \
         Each item is processed as a single LLM call. Results are validated \
         against a JSON Schema and written to a JSONL output file. \
         The model should write the input file first, then call this tool, \
         then read the output file.\n\n\
         This is more reliable than writing the loop yourself because the \
         engine handles concurrency, retries, and schema validation \
         deterministically (LCM §3.1, Figure 4)."
    }

    fn display_name(&self) -> &'static str {
        "LLM Map"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("LayoutGrid")
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
                    "description": "Prompt template. Use {input} as placeholder for each item."
                },
                "outputSchema": {
                    "type": "object",
                    "description": "JSON Schema for validating each output item."
                },
                "outputPath": {
                    "type": "string",
                    "description": "Path to write the JSONL output file."
                },
                "concurrency": {
                    "type": "integer",
                    "description": "Maximum concurrent workers (default 16).",
                    "default": 16
                },
                "maxRetries": {
                    "type": "integer",
                    "description": "Maximum retries per item (default 3).",
                    "default": 3
                }
            },
            "required": ["inputPath", "prompt", "outputSchema", "outputPath"]
        })
    }

    async fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> AgentJaxResult<Value> {
        let args: LlmMapArgs = serde_json::from_value(arguments.clone())
            .map_err(|e| AgentJaxError::tool(format!("Invalid arguments for llm_map: {e}")))?;

        // Validate and resolve paths.
        let workspace_dir = get_workspace_dir(context)?;
        let input_path = workspace_dir.join(&args.input_path);
        let output_path = workspace_dir.join(&args.output_path);

        if !input_path.exists() {
            return Err(AgentJaxError::not_found(format!(
                "Input file not found: {}",
                input_path.display()
            )));
        }

        // Read input items.
        let input_content = std::fs::read_to_string(&input_path)
            .map_err(|e| AgentJaxError::internal(format!("Failed to read input file: {e}")))?;

        let input_lines: Vec<String> = input_content.lines().filter(|l| !l.trim().is_empty()).map(|s| s.to_string()).collect();
        if input_lines.is_empty() {
            return Err(AgentJaxError::tool("Input file is empty".to_string()));
        }

        let total = input_lines.len();
        let model_ref = context.model_id.clone().unwrap_or_else(|| "default".to_string());
        let prompt_template = args.prompt;
        let output_schema = args.output_schema;
        let concurrency = args.concurrency.max(1);
        let max_retries = args.max_retries;

        // Status tracking.
        let completed = Arc::new(AtomicUsize::new(0));
        let failed = Arc::new(AtomicUsize::new(0));
        let semaphore = Arc::new(Semaphore::new(concurrency));
        let results: Arc<std::sync::Mutex<Vec<(usize, Result<Value, String>)>>> =
            Arc::new(std::sync::Mutex::new(Vec::with_capacity(total)));

        let rt = tokio::runtime::Handle::current();

        // Process items in parallel using a synchronous approach since
        // Tool::execute is synchronous. We spawn a tokio task to do the
        // async work and block on it.
        //
        // For a production version, this should be implemented as a
        // background job tool (like BackgroundTask) rather than blocking
        // the tool execution thread.
        let task = {
            let completed = completed.clone();
            let failed = failed.clone();
            let semaphore = semaphore.clone();
            let results = results.clone();
            let prompt_template = prompt_template.clone();
            let output_schema = output_schema.clone();
            let model_ref = model_ref.clone();

            rt.spawn(async move {
                let mut handles = Vec::with_capacity(total);

                for (i, line) in input_lines.iter().enumerate() {
                    let permit = semaphore.clone().acquire_owned();
                    let line = line.clone();
                    let prompt = prompt_template.replace("{input}", &line);
                    let schema = output_schema.clone();
                    let model = model_ref.clone();
                    let comp = completed.clone();
                    let fail = failed.clone();
                    let res = results.clone();
                    let retries = max_retries;

                    handles.push(tokio::spawn(async move {
                        let _permit = permit.await.unwrap();

                        let result = retry_with_backoff(
                            RetryStrategy {
                                max_attempts: retries,
                                base_delay_ms: 500,
                                max_delay_ms: 10_000,
                                jitter: true,
                                non_retryable_kinds: vec![
                                    crate::error::ErrorKind::ProviderAuth,
                                    crate::error::ErrorKind::Config,
                                ],
                            },
                            || {
                                let p = prompt.clone();
                                let m = model.clone();
                                async move {
                                    call_llm_for_item(&p, &m).await
                                }
                            },
                        ).await;

                        match result {
                            RetryResult::Success(output) => {
                                // Validate against schema.
                                if let Err(e) = validate_against_schema(&output, &schema) {
                                    fail.fetch_add(1, Ordering::SeqCst);
                                    let mut lock = res.lock().unwrap();
                                    lock.push((i, Err(format!("Schema validation failed: {e}"))));
                                    return;
                                }
                                comp.fetch_add(1, Ordering::SeqCst);
                                let mut lock = res.lock().unwrap();
                                lock.push((i, Ok(output)));
                            }
                            RetryResult::Failed(e) | RetryResult::NonRetryable(e) => {
                                fail.fetch_add(1, Ordering::SeqCst);
                                let mut lock = res.lock().unwrap();
                                lock.push((i, Err(e.to_string())));
                            }
                        }
                    }));
                }

                // Wait for all tasks.
                for handle in handles {
                    let _ = handle.await;
                }
            })
        };

        // Await the task directly (execute is now async).
        task.await
            .map_err(|e| AgentJaxError::internal(format!("llm_map task failed: {e}")))?;

        // Sort results by index and write output.
        let final_results = {
            let mut lock = results.lock().unwrap();
            lock.sort_by_key(|(i, _)| *i);
            lock.clone()
        };

        let mut output_lines = Vec::new();
        let mut output_items = Vec::new();
        for (_i, result) in &final_results {
            match result {
                Ok(val) => {
                    output_lines.push(serde_json::to_string(val).unwrap_or_default());
                    output_items.push(val.clone());
                }
                Err(e) => {
                    output_lines.push(serde_json::to_string(&json!({
                        "error": e
                    })).unwrap_or_default());
                }
            }
        }

        // Write output file.
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

impl LlmMapTool {
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Resolve the workspace directory from the tool context.
fn get_workspace_dir(context: &ToolExecutionContext) -> AgentJaxResult<PathBuf> {
    let conv_id = context.conversation_id.as_deref().ok_or_else(|| {
        AgentJaxError::tool("llm_map requires a conversation context".to_string())
    })?;
    let path = crate::conversation_store::conversation_workspace_path(conv_id)
        .map_err(|e| AgentJaxError::internal(format!("Failed to resolve workspace: {e}")))?;
    Ok(path)
}

/// Make a single LLM call for one item.
async fn call_llm_for_item(prompt: &str, model_ref: &str) -> AgentJaxResult<Value> {
    use crate::provider_api::types::ResponseStreamRequest;
    use tokio::sync::watch;

    let (_cancel_tx, mut cancel_rx) = watch::channel(false);

    let request = ResponseStreamRequest {
        input_items: vec![json!({
            "role": "user",
            "content": [{"type": "input_text", "text": prompt}]
        })],
        model: Some(model_ref.to_string()),
        reasoning_effort: None,
        instructions_override: None,
        text: None,
        include: None,
        service_tier: None,
        prompt_cache_key: None,
        client_metadata: None,
        generate: None,
        tools: None,
        tool_choice: None,
    };

    let config = crate::config::load_config()
        .map_err(|e| AgentJaxError::config(format!("Failed to load config: {e}")))?;

    let response = crate::provider_api::stream_response(
        &config,
        &request,
        &mut cancel_rx,
        |_| Ok(()),
    )
    .await?;

    let text = response.output_text.trim().to_string();
    if text.is_empty() {
        return Err(AgentJaxError::internal("LLM returned empty response".to_string()));
    }

    // Try to parse as JSON (the model may output structured data).
    if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
        Ok(parsed)
    } else {
        Ok(json!({ "result": text }))
    }
}

/// Validate a value against a JSON Schema.
fn validate_against_schema(value: &Value, schema: &Value) -> Result<(), String> {
    // Basic validation: check that required top-level keys exist.
    if let Some(required) = schema.get("required").and_then(|v| v.as_array()) {
        for key in required {
            let key_str = key.as_str().ok_or_else(|| "Invalid schema: required key is not a string".to_string())?;
            if !value.get(key_str).is_some() {
                return Err(format!("Missing required field: '{key_str}'"));
            }
        }
    }

    // Check type constraint.
    if let Some(expected_type) = schema.get("type").and_then(|v| v.as_str()) {
        let actual_type = match value {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        if actual_type != expected_type {
            return Err(format!(
                "Type mismatch: expected '{expected_type}', got '{actual_type}'"
            ));
        }
    }

    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_validate_against_schema_required_fields() {
        let value = json!({"name": "Alice", "age": 30});
        let schema = json!({
            "type": "object",
            "required": ["name", "age"]
        });
        assert!(validate_against_schema(&value, &schema).is_ok());
    }

    #[test]
    fn test_validate_against_schema_missing_field() {
        let value = json!({"name": "Alice"});
        let schema = json!({
            "type": "object",
            "required": ["name", "age"]
        });
        assert!(validate_against_schema(&value, &schema).is_err());
    }

    #[test]
    fn test_validate_against_schema_type_check() {
        let value = json!("hello");
        let schema = json!({"type": "string"});
        assert!(validate_against_schema(&value, &schema).is_ok());

        let value = json!(42);
        assert!(validate_against_schema(&value, &schema).is_err());
    }

    #[test]
    fn test_get_workspace_dir_missing_context() {
        let ctx = ToolExecutionContext::default();
        let result = get_workspace_dir(&ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_args() {
        let args = json!({
            "inputPath": "/tmp/input.jsonl",
            "prompt": "Classify: {input}",
            "outputSchema": {"type": "object"},
            "outputPath": "/tmp/output.jsonl"
        });
        let parsed: LlmMapArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.concurrency, 16);
        assert_eq!(parsed.max_retries, 3);
    }
}
