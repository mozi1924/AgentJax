use super::chat_events::ChatStreamEvent;
use super::chat_registry::ChatRequestRegistry;
use super::chat_utils::run_blocking;
use crate::config;
use crate::conversation_store;
use crate::provider_api;
use crate::provider_api::types::ResponseStreamRequest;
use tauri::{Emitter, Manager};
use tokio::sync::watch;

const TITLE_GENERATION_INSTRUCTIONS: &str = "You generate concise conversation titles. Return only the title text with no quotes, no markdown, and no explanation. Match the user's language when it is obvious. Keep it under 12 Chinese characters or under 8 English words.";

pub fn schedule_title_generation(
    window: tauri::Window,
    app_handle: tauri::AppHandle,
    config: config::AppConfig,
    conversation_id: String,
    request_id: String,
) {
    tauri::async_runtime::spawn(async move {
        if let Err(err) =
            generate_title_and_emit(window, app_handle, config, &conversation_id, &request_id).await
        {
            log::warn!(
                "Failed to generate conversation title for {}: {}",
                conversation_id,
                err
            );
        }
    });
}

async fn generate_title_and_emit(
    window: tauri::Window,
    app_handle: tauri::AppHandle,
    config: config::AppConfig,
    conversation_id: &str,
    request_id: &str,
) -> Result<(), String> {
    let registry = app_handle.state::<ChatRequestRegistry>();
    if registry.is_conversation_deleted(conversation_id)? {
        return Ok(());
    }

    let candidate = {
        let conversation_id = conversation_id.to_string();
        run_blocking(move || conversation_store::load_title_generation_candidate(&conversation_id))
            .await?
    };

    let Some(candidate) = candidate else {
        return Ok(());
    };

    let (title_cancel_tx, mut title_cancel_rx) = watch::channel(false);
    let job_id = registry.register_title_request(conversation_id, title_cancel_tx)?;

    let title_request = ResponseStreamRequest {
        input_items: vec![serde_json::json!({
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": build_title_generation_prompt(&candidate)
            }]
        })],
        model: Some(config.utility_small_model_key().to_string()),
        reasoning_effort: None,
        instructions_override: Some(TITLE_GENERATION_INSTRUCTIONS.to_string()),
        text: None,
        include: None,
        service_tier: None,
        prompt_cache_key: None,
        client_metadata: None,
        generate: None,
        tools: None,
        tool_choice: None,
    };

    let response =
        provider_api::stream_response(&config, &title_request, &mut title_cancel_rx, |_| Ok(())).await;

    let cancelled = *title_cancel_rx.borrow();
    registry.finish_title_request(conversation_id, &job_id)?;

    if cancelled || registry.is_conversation_deleted(conversation_id)? {
        return Ok(());
    }

    let response = response?;
    let title = sanitize_generated_title(&response.output_text);
    if title.is_empty() {
        return Ok(());
    }

    let updated_title = {
        let conversation_id = conversation_id.to_string();
        let title = title.clone();
        run_blocking(move || conversation_store::update_auto_title(&conversation_id, &title))
            .await?
    }
    .and_then(|summary| {
        let title = summary.title.trim().to_string();
        if title.is_empty() { None } else { Some(title) }
    });

    if let Some(conversation_title) = updated_title {
        window
            .emit(
                "chat_stream_event",
                ChatStreamEvent {
                    request_id: request_id.to_string(),
                    event_index: 0,
                    kind: "title".to_string(),
                    delta: None,
                    response_id: None,
                    conversation_id: Some(conversation_id.to_string()),
                    conversation_title: Some(conversation_title),
                    error: None,
                    tool_call_id: None,
                    tool_name: None,
                    tool_display_name: None,
                    tool_description: None,
                    tool_icon: None,
                    tool_arguments: None,
                    tool_output: None,
                    tool_status: None,
                    tool_started_ts: None,
                    tool_completed_ts: None,
                    tool_duration_ms: None,
                    context_token_count: None,
                    phase: None,
                    agent_id: None,
                },
            )
            .map_err(|e| format!("Failed to emit title update event: {e}"))?;
    }

    Ok(())
}

fn build_title_generation_prompt(
    candidate: &conversation_store::TitleGenerationCandidate,
) -> String {
    format!(
        "User message:\n{}\n\nAssistant reply:\n{}\n\nGenerate one concise conversation title.",
        candidate.user_text.trim(),
        candidate.assistant_text.trim()
    )
}

fn sanitize_generated_title(raw: &str) -> String {
    let first_line = raw
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");

    let cleaned = first_line
        .trim_matches('"')
        .trim_matches('“')
        .trim_matches('”')
        .trim_matches('`')
        .trim();

    if cleaned.is_empty() {
        return String::new();
    }

    let compact = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= 32 {
        compact
    } else {
        compact.chars().take(32).collect()
    }
}
