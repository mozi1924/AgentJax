//! Model cache subsystem.
//!
//! Each provider has its own cache directory under the config directory:
//! `~/.agentjax/models/<provider_key>/cache.json`
//!
//! The cache stores the raw JSON response from the provider's models API
//! endpoint, plus metadata about when it was synced. The framework reads
//! these files to build the model catalog displayed to the user.
//!
//! This replaces the old single-file YAML cache (`models-cache.yaml`) with
//! a per-provider JSON layout that stores the original API response.

use crate::config::{self, AgentConfig, AppConfig};
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::provider_api;
use crate::provider_api::types::ProviderModelDescriptor;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub const MODEL_CACHE_SYNC_INTERVAL_SECONDS: u64 = 30 * 60;

/// Per-provider cache file content.
///
/// Stores the raw API response (`raw_response`) alongside sync metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCache {
    /// Unix timestamp of the last successful sync.
    pub last_synced_unix: i64,
    /// The API endpoint used for the model list fetch.
    pub source_api_endpoint: String,
    /// The raw JSON response from the GET /models endpoint.
    /// Stored as-is so we can re-parse when model filtering logic changes.
    #[serde(default)]
    pub raw_response: Value,
}

/// A single file in the model cache that has been read and parsed.
#[derive(Debug, Clone)]
pub struct ParsedProviderCache {
    pub last_synced_unix: i64,
    pub models: Vec<ProviderModelDescriptor>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogEntry {
    pub profile_key: String,
    pub provider_key: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub supports_reasoning: bool,
    pub supported_reasoning_levels: Vec<String>,
    pub configured_reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalog {
    pub config_path: String,
    pub cache_base_path: String,
    pub default_model: String,
    pub utility_small_model: String,
    pub configured_models: Vec<String>,
    pub cached_models: Vec<String>,
    pub effective_models: Vec<String>,
    pub model_options: Vec<ModelCatalogEntry>,
    pub cache_stale: bool,
    pub last_synced_unix: Option<i64>,
}

// ── Public API ───────────────────────────────────────────────────────────────

pub async fn get_model_catalog(sync_if_stale: bool) -> AgentJaxResult<ModelCatalog> {
    let config_path = config::init_config_if_missing()?;
    let cfg = config::load_config()?;
    let cache_base_path = model_cache_base_path()?;
    // Load the active agent's config for per-agent model defaults.
    let agent = config::load_agent_config(&cfg.active_agent_id)
        .unwrap_or_default()
        .normalize();

    if sync_if_stale {
        let _ = sync_remote_model_cache_if_stale(&cfg).await;
    }

    let configured_models = dedup_strings(cfg.configured_models());
    let cached_models = load_cached_models_for_active(&cfg, &agent)?;
    let all_cached_models = load_all_provider_caches(&cfg)?;

    let effective_models = configured_models.clone();

    let active_cache_entry = all_cached_models.providers.get(&agent.active_provider);

    Ok(ModelCatalog {
        config_path: config_path.display().to_string(),
        cache_base_path: cache_base_path.display().to_string(),
        default_model: agent.default_model.clone(),
        utility_small_model: agent.utility_small_model.clone(),
        configured_models,
        cached_models,
        effective_models,
        model_options: build_model_catalog_entries(&cfg, &all_cached_models)?,
        cache_stale: active_cache_entry.map(is_cache_stale).unwrap_or(true),
        last_synced_unix: active_cache_entry.map(|entry| entry.last_synced_unix),
    })
}

pub fn get_model_catalog_entries_from_config(
    cfg: &AppConfig,
) -> AgentJaxResult<Vec<ModelCatalogEntry>> {
    let all_cached = load_all_provider_caches(cfg)?;
    build_model_catalog_entries(cfg, &all_cached)
}

pub async fn sync_remote_model_cache() -> AgentJaxResult<AllProviderCaches> {
    let cfg = config::load_config()?;
    sync_all_provider_caches(&cfg).await
}

pub async fn sync_remote_model_cache_if_stale(
    cfg: &AppConfig,
) -> AgentJaxResult<Option<AllProviderCaches>> {
    let all_cached = load_all_provider_caches(cfg)?;

    let has_stale = cfg.provider_keys().iter().any(|key| {
        all_cached
            .providers
            .get(key)
            .map(is_cache_stale)
            .unwrap_or(true)
    });

    if !has_stale {
        return Ok(None);
    }

    let refreshed = sync_all_provider_caches(cfg).await?;
    Ok(Some(refreshed))
}

// ── Cache I/O ────────────────────────────────────────────────────────────────

fn model_cache_base_path() -> AgentJaxResult<PathBuf> {
    Ok(config::config_dir_path()?.join("models"))
}

fn provider_cache_dir(provider_key: &str) -> AgentJaxResult<PathBuf> {
    Ok(model_cache_base_path()?.join(sanitize_dir_name(provider_key)))
}

fn provider_cache_path(provider_key: &str) -> AgentJaxResult<PathBuf> {
    Ok(provider_cache_dir(provider_key)?.join("cache.json"))
}

fn sanitize_dir_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn load_provider_cache(provider_key: &str) -> AgentJaxResult<Option<ProviderCache>> {
    let path = provider_cache_path(provider_key)?;
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).map_err(|e| {
        AgentJaxError::internal(format!(
            "Failed to read model cache {}: {e}",
            path.display()
        ))
    })?;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let cache: ProviderCache = serde_json::from_str(&raw).map_err(|e| {
        AgentJaxError::config(format!(
            "Invalid JSON in model cache {}: {e}",
            path.display()
        ))
    })?;
    Ok(Some(cache))
}

fn save_provider_cache(provider_key: &str, cache: &ProviderCache) -> AgentJaxResult<()> {
    let dir = provider_cache_dir(provider_key)?;
    fs::create_dir_all(&dir).map_err(|e| {
        AgentJaxError::internal(format!("Failed to create cache dir {}: {e}", dir.display()))
    })?;
    let path = dir.join("cache.json");
    let json = serde_json::to_string_pretty(cache)
        .map_err(|e| AgentJaxError::internal(format!("Failed to serialize model cache: {e}")))?;
    fs::write(&path, json).map_err(|e| {
        AgentJaxError::internal(format!(
            "Failed to write model cache {}: {e}",
            path.display()
        ))
    })?;
    Ok(())
}

/// All provider caches loaded into memory and parsed.
#[derive(Debug, Clone, Default)]
pub struct AllProviderCaches {
    pub providers: BTreeMap<String, ParsedProviderCache>,
}

fn load_all_provider_caches(cfg: &AppConfig) -> AgentJaxResult<AllProviderCaches> {
    let mut all = AllProviderCaches::default();

    for key in cfg.provider_keys() {
        let raw_cache = match load_provider_cache(&key)? {
            Some(c) => c,
            None => continue,
        };

        let models = parse_raw_models_response(&raw_cache.raw_response);

        all.providers.insert(
            key,
            ParsedProviderCache {
                last_synced_unix: raw_cache.last_synced_unix,
                models,
            },
        );
    }

    Ok(all)
}

/// Load cached model IDs for the active provider.
fn load_cached_models_for_active(
    cfg: &AppConfig,
    agent: &AgentConfig,
) -> AgentJaxResult<Vec<String>> {
    let all = load_all_provider_caches(cfg)?;
    Ok(all
        .providers
        .get(&agent.active_provider)
        .map(|p| dedup_strings(p.models.iter().map(|m| m.id.clone()).collect()))
        .unwrap_or_default())
}

/// Parse raw models API response into `ProviderModelDescriptor`s.
///
/// Handles both OpenAI format `{ data: [{ id: "...", ... }] }` and
/// format `{ models: [...] }`.
fn parse_raw_models_response(raw: &Value) -> Vec<ProviderModelDescriptor> {
    if raw.is_null() {
        return Vec::new();
    }

    let arr = raw
        .get("data")
        .or_else(|| raw.get("models"))
        .and_then(Value::as_array);

    let Some(arr) = arr else {
        return Vec::new();
    };

    arr.iter()
        .filter_map(|model| {
            let id = model
                .get("id")
                .or_else(|| model.get("name"))
                .and_then(Value::as_str)?;
            Some(ProviderModelDescriptor {
                id: id.to_string(),
                supported_reasoning_levels: model
                    .get("supported_reasoning_levels")
                    .or_else(|| model.get("supportedReasoningLevels"))
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                kind: None,
            })
        })
        .collect()
}

// ── Sync ─────────────────────────────────────────────────────────────────────

async fn sync_all_provider_caches(cfg: &AppConfig) -> AgentJaxResult<AllProviderCaches> {
    let mut successful = 0usize;
    let mut errors = Vec::new();

    for provider_key in cfg.provider_keys() {
        match sync_single_provider_cache(cfg, &provider_key).await {
            Ok(()) => {
                successful += 1;
            }
            Err(err) => {
                errors.push(format!("{provider_key}: {err}"));
            }
        }
    }

    if successful == 0 {
        return Err(AgentJaxError::internal(format!(
            "Failed to sync remote model cache for all providers: {}",
            errors.join(" | ")
        )));
    }

    load_all_provider_caches(cfg)
}

async fn sync_single_provider_cache(cfg: &AppConfig, provider_key: &str) -> AgentJaxResult<()> {
    let api_endpoint = cfg
        .resolved_provider(provider_key)
        .map(|p| p.api_endpoint())
        .unwrap_or_default();

    // Fetch raw models response via the protocol layer
    let raw_response = match provider_api::fetch_remote_models(cfg, provider_key).await {
        Ok(models) => {
            // Convert model descriptors back to a JSON response shape
            let data: Vec<Value> = models
                .iter()
                .map(|m| {
                    let mut entry = serde_json::json!({
                        "id": m.id,
                    });
                    if !m.supported_reasoning_levels.is_empty() {
                        entry["supported_reasoning_levels"] =
                            serde_json::json!(m.supported_reasoning_levels);
                    }
                    entry
                })
                .collect();
            serde_json::json!({ "data": data })
        }
        Err(err) => {
            log::warn!("Failed to fetch models for '{provider_key}': {err}");
            return Err(err);
        }
    };

    let now = now_unix();
    let cache = ProviderCache {
        last_synced_unix: now,
        source_api_endpoint: api_endpoint,
        raw_response,
    };

    save_provider_cache(provider_key, &cache)?;
    log::info!("Model cache synced for provider '{provider_key}'");
    Ok(())
}

fn is_cache_stale(cache: &ParsedProviderCache) -> bool {
    if cache.last_synced_unix <= 0 {
        return true;
    }
    let now = now_unix();
    now - cache.last_synced_unix >= MODEL_CACHE_SYNC_INTERVAL_SECONDS as i64
}

// ── Catalog Building ─────────────────────────────────────────────────────────

fn build_model_catalog_entries(
    cfg: &AppConfig,
    all_cached: &AllProviderCaches,
) -> AgentJaxResult<Vec<ModelCatalogEntry>> {
    let mut entries = Vec::new();

    for (provider_key, provider) in &cfg.providers {
        for (model_key, model_cfg) in &provider.models {
            if !model_cfg.enabled {
                continue;
            }

            // Resolve the model kind: user override takes precedence, then
            // the plugin's metadata table.
            let model_kind = model_cfg
                .kind
                .clone()
                .filter(|k| !k.is_empty())
                .or_else(|| {
                    provider_api::get_model_metadata(&provider.kind, model_key)
                        .ok()
                        .and_then(|meta| meta.kind)
                        .filter(|k| !k.is_empty())
                });

            // Skip non-chat models (e.g. embeddings) in the chat model selector.
            // Models without a declared kind are treated as chat for backward compat.
            if let Some(ref kind) = model_kind
                && kind != "chat"
            {
                continue;
            }

            let cached_levels = all_cached.providers.get(provider_key).and_then(|cached| {
                cached
                    .models
                    .iter()
                    .find(|m| &m.id == model_key)
                    .map(|m| m.supported_reasoning_levels.as_slice())
            });

            let reasoning =
                provider_api::get_reasoning_capability(&provider.kind, model_key, cached_levels)?;

            let configured_reasoning_effort = model_cfg
                .request
                .reasoning
                .as_ref()
                .filter(|r| r.enabled)
                .and_then(|r| r.effort)
                .map(|e| e.as_str().to_string())
                .filter(|value| {
                    reasoning
                        .supported_reasoning_levels
                        .iter()
                        .any(|level| level == value)
                });

            entries.push(ModelCatalogEntry {
                profile_key: format!("{provider_key}/{model_key}"),
                provider_key: provider_key.clone(),
                model_id: model_key.clone(),
                name: model_cfg.name.clone(),
                supports_reasoning: reasoning.supports_reasoning,
                supported_reasoning_levels: reasoning.supported_reasoning_levels,
                configured_reasoning_effort,
                kind: model_kind,
            });
        }
    }

    Ok(entries)
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn dedup_strings(items: Vec<String>) -> Vec<String> {
    let mut set = BTreeSet::new();
    for item in items {
        let trimmed = item.trim().to_string();
        if !trimmed.is_empty() {
            set.insert(trimmed);
        }
    }
    set.into_iter().collect()
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_raw_models_response() {
        let raw = json!({
            "data": [
                {"id": "gpt-5", "supported_reasoning_levels": ["low", "high"]},
                {"id": "gpt-5-mini"}
            ]
        });
        let models = parse_raw_models_response(&raw);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-5");
        assert_eq!(models[0].supported_reasoning_levels.len(), 2);
        assert_eq!(models[1].id, "gpt-5-mini");
        assert!(models[1].supported_reasoning_levels.is_empty());
    }

    #[test]
    fn test_parse_raw_models_response_empty() {
        let raw = json!({ "data": [] });
        let models = parse_raw_models_response(&raw);
        assert!(models.is_empty());
    }

    #[test]
    fn test_parse_raw_models_response_null() {
        let models = parse_raw_models_response(&Value::Null);
        assert!(models.is_empty());
    }

    #[test]
    fn test_sanitize_dir_name() {
        assert_eq!(sanitize_dir_name("openai"), "openai");
        assert_eq!(sanitize_dir_name("My Provider"), "My_Provider");
        assert_eq!(sanitize_dir_name("user/openai"), "user_openai");
    }
}
