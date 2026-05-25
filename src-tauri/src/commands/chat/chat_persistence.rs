use super::chat_utils::{now_unix_ms, run_blocking};
use crate::conversation_store;
use crate::providers::types::ResponseStreamResult;
use serde_json::Value;

pub async fn persist_completed_exchange(
    conversation_id: &str,
    request_id: &str,
    input_text: &str,
    response: &ResponseStreamResult,
    timeline_events: Option<Vec<Value>>,
    utility_model: &str,
) -> Result<(), String> {
    let conversation_id = conversation_id.to_string();
    let request_id = request_id.to_string();
    let input_text = input_text.to_string();
    let response = response.clone();
    let utility_model = utility_model.to_string();
    let now = now_unix_ms();

    run_blocking(move || {
        conversation_store::append_message(
            conversation_store::AppendMessageInput {
                conversation_id: conversation_id.clone(),
                entry_id: format!("msg-user-{request_id}"),
                role: "user".to_string(),
                text: input_text.clone(),
                created_at_unix_ms: now,
                response_id: None,
                provider: Some(response.provider_key.clone()),
                model_profile: Some(response.model_profile.clone()),
                model_id: Some(response.model_id.clone()),
                request_id: Some(request_id.clone()),
                context_items: conversation_store::build_user_input_items(&input_text),
                timeline_events: None,
                metadata: Default::default(),
            },
            &utility_model,
        )?;

        conversation_store::append_message(
            conversation_store::AppendMessageInput {
                conversation_id: conversation_id.clone(),
                entry_id: format!("msg-assistant-{request_id}"),
                role: "assistant".to_string(),
                text: response.output_text.clone(),
                created_at_unix_ms: now_unix_ms(),
                response_id: Some(response.response_id.clone()),
                provider: Some(response.provider_key.clone()),
                model_profile: Some(response.model_profile.clone()),
                model_id: Some(response.model_id.clone()),
                request_id: Some(request_id.clone()),
                context_items: response.output_items.clone(),
                timeline_events,
                metadata: Default::default(),
            },
            &utility_model,
        )
    })
    .await
}
