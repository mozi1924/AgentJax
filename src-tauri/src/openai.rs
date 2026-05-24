use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

use crate::config::AppConfig;

#[derive(Debug, Deserialize)]
pub struct ResponsesApiResponse {
  pub id: String,
  pub output_text: Option<String>,
}

pub async fn create_response(
  config: &AppConfig,
  input: &str,
  requested_model: Option<&str>,
  previous_response_id: Option<&str>,
) -> Result<ResponsesApiResponse, String> {
  let api_key = config
    .resolved_api_key()
    .ok_or_else(|| "OPENAI API key is missing. Set api_key in config.yaml or OPENAI_API_KEY env.".to_string())?;

  let endpoint = format!("{}/responses", config.base_url.trim_end_matches('/'));
  let model = config.resolve_model(requested_model);

  let mut body = json!({
    "model": model,
    "input": input,
    "store": true
  });

  if let Some(previous_id) = previous_response_id.map(str::trim).filter(|s| !s.is_empty()) {
    body["previous_response_id"] = Value::String(previous_id.to_string());
  }

  let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(config.request_timeout_seconds))
    .build()
    .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?;

  let response = client
    .post(endpoint)
    .bearer_auth(api_key)
    .header("Content-Type", "application/json")
    .json(&body)
    .send()
    .await
    .map_err(|e| format!("Failed to reach OpenAI API: {e}"))?;

  if !response.status().is_success() {
    let status = response.status();
    let text = response
      .text()
      .await
      .unwrap_or_else(|_| "<unable to read error body>".to_string());
    return Err(format!("OpenAI API error ({status}): {text}"));
  }

  response
    .json::<ResponsesApiResponse>()
    .await
    .map_err(|e| format!("Failed to parse OpenAI response: {e}"))
}
