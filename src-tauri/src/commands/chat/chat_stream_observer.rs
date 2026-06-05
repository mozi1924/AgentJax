use super::chat_persistence::{
    ToolProgressPersistInput, persist_assistant_line, persist_tool_progress_event,
};
use crate::conversation_store;
use crate::provider_api::types::{ProviderStreamEvent, ProviderUsage, ResponseStreamResult};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// Persists provider stream side effects and maintains local token estimates.
#[derive(Clone)]
pub(super) struct ChatStreamObserver {
    agent_id: String,
    conversation_id: String,
    request_id: String,
    model_id: Option<String>,
    jsonl_backup_enabled: bool,
    fallback_token_count: Arc<AtomicUsize>,
    visible_token_count: Arc<AtomicUsize>,
    /// Accumulated thinking/reasoning content from ReasoningDelta events.
    /// Cleared after each HopAssistantText/AssistantMessageCompleted persist.
    accumulated_thinking: Arc<Mutex<Option<String>>>,
}

impl ChatStreamObserver {
    pub(super) fn new(
        agent_id: String,
        conversation_id: String,
        request_id: String,
        model_id: Option<String>,
        initial_token_count: usize,
        jsonl_backup_enabled: bool,
    ) -> Self {
        Self {
            agent_id,
            conversation_id,
            request_id,
            model_id,
            jsonl_backup_enabled,
            fallback_token_count: Arc::new(AtomicUsize::new(initial_token_count)),
            visible_token_count: Arc::new(AtomicUsize::new(initial_token_count)),
            accumulated_thinking: Arc::new(Mutex::new(None)),
        }
    }

    /// Persist stream events that affect conversation history and return the
    /// token count that should be attached to the outbound UI event, if any.
    /// Track thinking content from ReasoningDelta events so it can be
    /// persisted alongside assistant text in the JSONL fallback path.
    fn accumulate_thinking(&self, delta: &str) {
        if !delta.is_empty() {
            if let Ok(mut guard) = self.accumulated_thinking.lock() {
                let new_val = match guard.take() {
                    Some(mut existing) => { existing.push_str(delta); existing }
                    None => delta.to_string(),
                };
                *guard = Some(new_val);
            }
        }
    }

    fn take_thinking(&self) -> Option<String> {
        self.accumulated_thinking.lock().ok().and_then(|mut g| g.take())
    }

    pub(super) fn handle_provider_event(&self, event: &ProviderStreamEvent) -> Option<usize> {
        match event {
            ProviderStreamEvent::ReasoningDelta { delta } => {
                self.accumulate_thinking(delta);
            }
            ProviderStreamEvent::ReasoningCompleted { .. } => {
                // Thinking block complete; content is already accumulated.
            }
            ProviderStreamEvent::ToolCallStarted {
                call_id,
                name,
                presentation,
                ..
            } => {
                let _ = persist_tool_progress_event(ToolProgressPersistInput {
                    agent_id: &self.agent_id,
                    conversation_id: &self.conversation_id,
                    request_id: &self.request_id,
                    event_kind: "tool_call_started",
                    tool_call_id: call_id,
                    tool_name: Some(name),
                    tool_display_name: presentation.as_ref().map(|meta| meta.display_name.as_str()),
                    tool_description: presentation.as_ref().map(|meta| meta.description.as_str()),
                    tool_icon: presentation.as_ref().and_then(|meta| meta.icon.as_deref()),
                    payload: None,
                    started_at_unix_ms: None,
                    completed_at_unix_ms: None,
                }, self.jsonl_backup_enabled);
            }
            ProviderStreamEvent::ToolCallCompleted {
                call_id,
                name,
                arguments,
                presentation,
                ..
            } => {
                let _ = persist_tool_progress_event(ToolProgressPersistInput {
                    agent_id: &self.agent_id,
                    conversation_id: &self.conversation_id,
                    request_id: &self.request_id,
                    event_kind: "tool_call_done",
                    tool_call_id: call_id,
                    tool_name: Some(name),
                    tool_display_name: presentation.as_ref().map(|meta| meta.display_name.as_str()),
                    tool_description: presentation.as_ref().map(|meta| meta.description.as_str()),
                    tool_icon: presentation.as_ref().and_then(|meta| meta.icon.as_deref()),
                    payload: Some(arguments),
                    started_at_unix_ms: None,
                    completed_at_unix_ms: None,
                }, self.jsonl_backup_enabled);
                self.add_tool_call_argument_tokens(name, presentation.as_ref(), arguments);
            }
            ProviderStreamEvent::ToolCallExecuted {
                call_id,
                name,
                output,
                started_at_unix_ms,
                completed_at_unix_ms,
                presentation,
                ..
            } => {
                let _ = persist_tool_progress_event(ToolProgressPersistInput {
                    agent_id: &self.agent_id,
                    conversation_id: &self.conversation_id,
                    request_id: &self.request_id,
                    event_kind: "tool_call_exec",
                    tool_call_id: call_id,
                    tool_name: Some(name),
                    tool_display_name: presentation.as_ref().map(|meta| meta.display_name.as_str()),
                    tool_description: presentation.as_ref().map(|meta| meta.description.as_str()),
                    tool_icon: presentation.as_ref().and_then(|meta| meta.icon.as_deref()),
                    payload: Some(output),
                    started_at_unix_ms: Some(*started_at_unix_ms),
                    completed_at_unix_ms: Some(*completed_at_unix_ms),
                }, self.jsonl_backup_enabled);
                self.add_text_tokens(output);
            }
            ProviderStreamEvent::AssistantMessageCompleted {
                text,
                phase,
                response_id,
            }
                if *phase == Some(crate::message_phase::AssistantPhase::Commentary) => {
                    let thinking = self.take_thinking();
                    let _ = persist_assistant_line(                        &self.agent_id,                        &self.conversation_id,
                        &self.request_id,
                        response_id,
                        *phase,
                        text,
                        thinking.as_deref(),
                        self.jsonl_backup_enabled,
                    );
                    self.add_text_tokens(text);
                }
            ProviderStreamEvent::HopAssistantText {
                text,
                phase,
                response_id,
            } => {
                let thinking = self.take_thinking();
                let _ = persist_assistant_line(
                    &self.agent_id,
                    &self.conversation_id,
                    &self.request_id,
                    response_id,
                    *phase,
                    text,
                    thinking.as_deref(),
                    self.jsonl_backup_enabled,
                );
                self.add_text_tokens(text);
            }
            ProviderStreamEvent::UsageUpdated { usage, .. } => {
                self.visible_token_count
                    .store(usage.total_tokens, Ordering::Relaxed);
                return Some(usage.total_tokens);
            }
            _ => {}
        }

        None
    }

    pub(super) fn persist_final_token_usage(&self, response: &ResponseStreamResult) -> usize {
        if let Some(latest_usage_record) = response.usage_hops.last() {
            self.visible_token_count
                .store(latest_usage_record.usage.total_tokens, Ordering::Relaxed);
            if self.jsonl_backup_enabled
                && let Err(err) = conversation_store::update_conversation_token_usage(
                    &self.agent_id,
                    &self.conversation_id,
                    &self.request_id,
                    &latest_usage_record.response_id,
                    "provider",
                    "latest_response",
                    &latest_usage_record.usage,
                    response.usage.as_ref(),
                    &response.usage_hops,
                ) {
                log::warn!(
                    "Failed to persist provider token usage for conversation '{}': {}",
                    self.conversation_id,
                    err
                );
                }
        } else {
            let fallback_total = self.fallback_token_count.load(Ordering::Relaxed);
            self.visible_token_count
                .store(fallback_total, Ordering::Relaxed);
            let fallback_usage = ProviderUsage {
                prompt_tokens: fallback_total,
                completion_tokens: 0,
                total_tokens: fallback_total,
            };
            if self.jsonl_backup_enabled
                && let Err(err) = conversation_store::update_conversation_token_usage(
                    &self.agent_id,
                    &self.conversation_id,
                    &self.request_id,
                    &response.response_id,
                    "local_estimate",
                    "turn_estimate",
                    &fallback_usage,
                    None,
                    &[],
                ) {
                    log::warn!(
                        "Failed to persist estimated token usage for conversation '{}': {}",
                        self.conversation_id,
                        err
                    );
                }
        }

        self.visible_token_count.load(Ordering::Relaxed)
    }

    fn add_tool_call_argument_tokens(
        &self,
        name: &str,
        presentation: Option<&crate::tools::ToolPresentation>,
        arguments: &str,
    ) {
        let Some(model_id) = self.model_id.as_ref() else {
            return;
        };
        let Ok(arg_tokens) = conversation_store::count_text_tokens(model_id, arguments) else {
            return;
        };

        // Lightweight estimate for name / display_name / description.
        let meta_chars = name.len()
            + presentation
                .map(|meta| meta.display_name.len())
                .unwrap_or(0)
            + presentation.map(|meta| meta.description.len()).unwrap_or(0);
        self.add_fallback_tokens(arg_tokens.saturating_add(meta_chars.saturating_div(4)));
    }

    fn add_text_tokens(&self, text: &str) {
        let Some(model_id) = self.model_id.as_ref() else {
            return;
        };
        if let Ok(additional) = conversation_store::count_text_tokens(model_id, text) {
            self.add_fallback_tokens(additional);
        }
    }

    fn add_fallback_tokens(&self, tokens: usize) {
        self.fallback_token_count.store(
            self.fallback_token_count
                .load(Ordering::Relaxed)
                .saturating_add(tokens),
            Ordering::Relaxed,
        );
    }
}
