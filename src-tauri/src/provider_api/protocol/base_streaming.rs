//! Shared streaming infrastructure for protocol implementations.
//!
//! Provides common HTTP request setup, SSE stream processing, reasoning flush,
//! and result assembly helpers used by both `chat.rs` (Chat Completions) and
//! `responses.rs` (Responses API), eliminating ~80 lines of duplicated code.

use crate::config::ProviderConfig;
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::provider_api::core::ProviderIdFactory;
use crate::provider_api::network::{apply_headers_to_reqwest, split_sse_event_block};
use crate::provider_api::protocol::{build_client, send_and_check};
use crate::provider_api::types::*;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::watch;

// ── State Machine Trait ──────────────────────────────────────────────────────

/// State machine interface for protocol-specific SSE event processing.
///
/// Each protocol (Chat Completions, Responses) implements this trait on its
/// stream state struct, allowing the generic [`run_sse_stream`] function to
/// drive the event loop without knowing protocol-specific details.
pub trait StreamStateMachine {
    /// Process a single SSE event block.
    ///
    /// Returns `true` if the stream should be terminated (e.g., on completion
    /// event), `false` to continue processing.
    fn process_event(
        &mut self,
        event_block: &str,
        response_id: &mut String,
        output_text: &mut String,
        output_items: &mut Vec<Value>,
        usage: &mut Option<ProviderUsage>,
        on_delta: &mut dyn FnMut(ProviderStreamEvent) -> AgentJaxResult<()>,
    ) -> AgentJaxResult<bool>;

    /// Flush any remaining reasoning that wasn't terminated by a finish_reason
    /// or regular content event (e.g. stream ended mid-reasoning).
    ///
    /// This is the same logic in both Chat Completions and Responses protocols,
    /// so a default implementation is provided.
    fn flush_remaining_reasoning(
        &mut self,
        output_items: &mut Vec<Value>,
        on_delta: &mut dyn FnMut(ProviderStreamEvent) -> AgentJaxResult<()>,
    ) -> AgentJaxResult<bool>
    where
        Self: HasReasoningState,
    {
        if self.reasoning_started() && !self.reasoning_buffer().is_empty() {
            self.set_reasoning_started(false);
            on_delta(ProviderStreamEvent::ReasoningCompleted { total_tokens: None })?;
            output_items.push(json!({
                "type": "reasoning",
                "text": self.take_reasoning_buffer(),
            }));
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

/// Accessor trait for reasoning state fields, shared by both protocol state types.
///
/// Only reasoning-related fields are exposed here — protocol-specific fields
/// (like `emitted_output_started`) remain private to their respective modules.
pub trait HasReasoningState {
    fn reasoning_started(&self) -> bool;
    fn reasoning_buffer(&self) -> &str;
    fn set_reasoning_started(&mut self, val: bool);
    fn take_reasoning_buffer(&mut self) -> String;
}

// ── HTTP Request Setup ──────────────────────────────────────────────────────

/// Set up an HTTP POST request to the provider API and send it.
///
/// Handles timeout configuration, credential (Bearer) headers, custom HTTP
/// headers, and error classification.
pub async fn setup_http_request(
    provider_key: &str,
    provider_config: &ProviderConfig,
    url_suffix: &str,
    body: &Value,
) -> AgentJaxResult<reqwest::Response> {
    let timeout_seconds =
        provider_config.resolved_timeout_seconds(crate::config::constants::DEFAULT_TIMEOUT_SECONDS);
    let client = build_client(timeout_seconds)?;

    let base_url = provider_config
        .api_endpoint()
        .trim_end_matches('/')
        .to_string();
    let url = format!("{base_url}{url_suffix}");

    let credential = provider_config.resolved_credential();
    let mut builder = client.post(&url).json(body);
    if let Some(ref credential) = credential {
        builder = builder.header("Authorization", format!("Bearer {credential}"));
    }
    let headers = provider_config.resolved_http_headers();
    builder = apply_headers_to_reqwest(builder, &headers)?;
    send_and_check(builder, provider_key).await
}

// ── SSE Stream Loop ─────────────────────────────────────────────────────────

/// Drive an SSE stream, calling [`StreamStateMachine::process_event`] for each
/// event block.
///
/// Handles cancellation via `cancel_rx`, byte buffer accumulation, and SSE
/// event boundary splitting. Returns when the stream ends naturally, is
/// cancelled, or when `process_event` returns `true`.
pub async fn run_sse_stream<S: StreamStateMachine + Send>(
    response: reqwest::Response,
    mut state: S,
    cancel_rx: &mut watch::Receiver<bool>,
    response_id: &mut String,
    output_text: &mut String,
    output_items: &mut Vec<Value>,
    usage: &mut Option<ProviderUsage>,
    on_delta: &mut (dyn FnMut(ProviderStreamEvent) -> AgentJaxResult<()> + Send + '_),
) -> AgentJaxResult<S> {
    let mut buffer = String::new();
    let mut stream = response.bytes_stream();
    let mut stream_done = false;

    while !stream_done {
        tokio::select! {
            changed = cancel_rx.changed() => {
                if changed.is_ok() && *cancel_rx.borrow() { break; }
            }
            next_chunk = stream.next() => {
                let Some(next_chunk) = next_chunk else { break; };
                let bytes = next_chunk
                    .map_err(|err| AgentJaxError::network(format!("Failed to read stream: {err}")))?;
                buffer.push_str(&String::from_utf8_lossy(&bytes));
                while let Some((event_block, rest)) = split_sse_event_block(&buffer) {
                    buffer = rest;
                    if state.process_event(
                        &event_block, response_id, output_text, output_items, usage, on_delta,
                    )? {
                        stream_done = true;
                        break;
                    }
                }
            }
        }
    }

    // Flush any unprocessed buffer remnant (incomplete SSE block).
    if !stream_done && !buffer.trim().is_empty() {
        let _ = state.process_event(
            &buffer, response_id, output_text, output_items, usage, on_delta,
        )?;
    }

    Ok(state)
}

// ── Response Assembly Helpers ───────────────────────────────────────────────

/// Build the final response ID, falling back to a generated one if empty.
pub fn finalize_response_id(response_id: &str, provider_key: &str) -> String {
    if response_id.is_empty() {
        ProviderIdFactory::new(provider_key)
            .response_id()
            .to_string()
    } else {
        response_id.to_string()
    }
}

/// Build the usage hops record from optional usage data.
pub fn build_usage_hops(
    usage: &Option<ProviderUsage>,
    final_response_id: &str,
) -> Vec<ProviderUsageRecord> {
    usage
        .clone()
        .map(|u| ProviderUsageRecord {
            response_id: final_response_id.to_string(),
            usage: u,
        })
        .into_iter()
        .collect()
}

/// Check whether usage data is non-empty (at least one token counted).
pub fn has_nonzero_usage(usage: &ProviderUsage) -> bool {
    usage.prompt_tokens > 0 || usage.completion_tokens > 0 || usage.total_tokens > 0
}
