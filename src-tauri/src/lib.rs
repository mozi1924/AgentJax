use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatRequest {
  input: String,
  previous_response_id: Option<String>,
  model: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatResponse {
  response_id: String,
  output_text: String,
}

#[derive(Debug, Deserialize)]
struct OpenAIResponse {
  id: String,
  output_text: Option<String>,
}

#[tauri::command]
async fn chat_with_responses(req: ChatRequest) -> Result<ChatResponse, String> {
  let api_key = std::env::var("OPENAI_API_KEY")
    .map_err(|_| "OPENAI_API_KEY is not set. Please configure it before chatting.".to_string())?;

  let client = reqwest::Client::new();
  let model = req.model.unwrap_or_else(|| "gpt-5".to_string());

  let mut body = json!({
    "model": model,
    "input": req.input,
    "store": true
  });

  if let Some(prev_id) = req.previous_response_id {
    if !prev_id.is_empty() {
      body["previous_response_id"] = json!(prev_id);
    }
  }

  let response = client
    .post("https://api.openai.com/v1/responses")
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

  let parsed: OpenAIResponse = response
    .json()
    .await
    .map_err(|e| format!("Failed to parse OpenAI response: {e}"))?;

  Ok(ChatResponse {
    response_id: parsed.id,
    output_text: parsed.output_text.unwrap_or_else(|| "".to_string()),
  })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .setup(|app| {
      if cfg!(debug_assertions) {
        app.handle().plugin(
          tauri_plugin_log::Builder::default()
            .level(log::LevelFilter::Info)
            .build(),
        )?;
      }
      Ok(())
    })
    .invoke_handler(tauri::generate_handler![chat_with_responses])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
