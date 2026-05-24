use serde::{Deserialize, Serialize};

use crate::config;
use crate::openai;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
  pub input: String,
  pub previous_response_id: Option<String>,
  pub model: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatResponse {
  pub response_id: String,
  pub output_text: String,
}

#[tauri::command]
pub async fn chat_with_responses(req: ChatRequest) -> Result<ChatResponse, String> {
  let config = config::load_config()?;

  let response = openai::create_response(
    &config,
    &req.input,
    req.model.as_deref(),
    req.previous_response_id.as_deref(),
  )
  .await?;

  Ok(ChatResponse {
    response_id: response.id,
    output_text: response.output_text.unwrap_or_default(),
  })
}
