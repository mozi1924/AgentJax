use crate::agentjax_err;
use crate::error::AgentJaxError;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokenizers::Tokenizer;

struct LocalTokenizerManager {
    cache: Mutex<HashMap<String, Arc<Tokenizer>>>,
    failed_loads: Mutex<HashMap<String, String>>,
}

impl LocalTokenizerManager {
    fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            failed_loads: Mutex::new(HashMap::new()),
        }
    }

    fn count_tokens(&self, model: &str, text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }

        let tokenizer = self.get_or_load_tokenizer(model);
        match tokenizer.and_then(|tokenizer| {
            tokenizer
                .encode(text, true)
                .map(|encoding| encoding.get_ids().len())
                .map_err(|err| {
                    AgentJaxError::internal(format!("Tokenization error: {err}"))
                        .with_source(err.to_string())
                })
        }) {
            Ok(count) => count,
            Err(err) => {
                log::warn!(
                    "Falling back to character-ratio token estimate for model '{}': {}",
                    model,
                    err
                );
                estimate_tokens_from_chars(text)
            }
        }
    }

    fn get_or_load_tokenizer(&self, model: &str) -> crate::error::AgentJaxResult<Arc<Tokenizer>> {
        let tokenizer_id = tokenizer_id_for_model(model)?;
        if let Ok(cache) = self.cache.lock()
            && let Some(tokenizer) = cache.get(tokenizer_id)
        {
            return Ok(Arc::clone(tokenizer));
        }
        if let Ok(failed_loads) = self.failed_loads.lock()
            && let Some(err) = failed_loads.get(tokenizer_id)
        {
            return Err(AgentJaxError::internal(err.clone()));
        }

        let tokenizer = match Tokenizer::from_pretrained(tokenizer_id, None) {
            Ok(tokenizer) => Arc::new(tokenizer),
            Err(err) => {
                let message = format!("Failed to load tokenizer '{tokenizer_id}': {err}");
                if let Ok(mut failed_loads) = self.failed_loads.lock() {
                    failed_loads.insert(tokenizer_id.to_string(), message.clone());
                }
                return Err(AgentJaxError::internal(message));
            }
        };

        let mut cache = self
            .cache
            .lock()
            .map_err(|_| "Failed to lock tokenizer cache".to_string())?;
        Ok(Arc::clone(
            cache
                .entry(tokenizer_id.to_string())
                .or_insert_with(|| Arc::clone(&tokenizer)),
        ))
    }
}

fn local_tokenizer_manager() -> &'static LocalTokenizerManager {
    static MANAGER: OnceLock<LocalTokenizerManager> = OnceLock::new();
    MANAGER.get_or_init(LocalTokenizerManager::new)
}

pub(super) fn count_model_tokens(model: &str, text: &str) -> usize {
    local_tokenizer_manager().count_tokens(model, text)
}

pub(super) fn count_serialized_tool_schema_tokens(
    model: &str,
    tools: &[Value],
) -> crate::error::AgentJaxResult<usize> {
    let mut total = 0usize;
    for tool in tools {
        let serialized = serde_json::to_string(tool)
            .map_err(|err| format!("Failed to serialize tool schema for token counting: {err}"))?;
        total = total.saturating_add(count_model_tokens(model, &serialized));
    }
    Ok(total)
}

fn normalize_model_name(model: &str) -> String {
    let trimmed = model.trim();
    if let Some((_, model_name)) = trimmed.rsplit_once('/') {
        let model_name = model_name.trim();
        if !model_name.is_empty() {
            return model_name.to_ascii_lowercase();
        }
    }
    trimmed.to_ascii_lowercase()
}

fn tokenizer_id_for_model(model: &str) -> crate::error::AgentJaxResult<&'static str> {
    let normalized = normalize_model_name(model).to_ascii_lowercase();
    if normalized.is_empty() {
        return Err(agentjax_err!(
            "Model name cannot be empty for token counting",
            Config
        ));
    }

    if normalized.contains("claude") {
        return Ok("Xenova/claude-3-tokenizer");
    }

    if normalized.contains("gemini") || normalized.contains("gemma") {
        return Ok("google/gemma-2b");
    }

    if normalized.starts_with("gpt-oss") {
        return Ok("openai/gpt-oss-20b");
    }

    // New GPT-5, GPT-4.1, GPT-4o and o-series models map to an o200k-style
    // tokenizer; older GPT-4 / GPT-3.5 style models stay on cl100k-like assets.
    if normalized.starts_with("gpt-3.5")
        || (normalized.starts_with("gpt-4")
            && !normalized.starts_with("gpt-4o")
            && !normalized.starts_with("gpt-4.1")
            && !normalized.starts_with("gpt-4.5"))
    {
        return Ok("Xenova/gpt-4");
    }

    Ok("Xenova/gpt-4o")
}

fn estimate_tokens_from_chars(text: &str) -> usize {
    text.chars().count().saturating_add(3) / 4
}
