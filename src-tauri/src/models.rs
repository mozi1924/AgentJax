use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{self, AppConfig};
use crate::providers;
use crate::providers::types::ProviderModelDescriptor;

const MODEL_CACHE_FILE_NAME: &str = "models-cache.yaml";
pub const MODEL_CACHE_SYNC_INTERVAL_SECONDS: u64 = 30 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelCache {
    pub version: u32,
    pub providers: BTreeMap<String, ProviderModelCache>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProviderModelCache {
    pub last_synced_unix: i64,
    #[serde(default, deserialize_with = "deserialize_cached_models")]
    pub models: Vec<ProviderModelDescriptor>,
    pub source_api_endpoint: String,
}

impl Default for ModelCache {
    fn default() -> Self {
        Self {
            version: 3,
            providers: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogEntry {
    pub profile_key: String,
    pub provider_key: String,
    pub model_id: String,
    pub supports_reasoning: bool,
    pub supported_reasoning_levels: Vec<String>,
    pub configured_reasoning_effort: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalog {
    pub config_path: String,
    pub cache_path: String,
    pub default_model: String,
    pub utility_small_model: String,
    pub configured_models: Vec<String>,
    pub cached_models: Vec<String>,
    pub effective_models: Vec<String>,
    pub model_options: Vec<ModelCatalogEntry>,
    pub cache_stale: bool,
    pub last_synced_unix: Option<i64>,
}

pub async fn get_model_catalog(sync_if_stale: bool) -> Result<ModelCatalog, String> {
    let config_path = config::init_config_if_missing()?;
    let cfg = config::load_config()?;
    let cache_path = model_cache_path()?;

    if sync_if_stale {
        let _ = sync_remote_model_cache_if_stale(&cfg).await;
    }

    let cache = load_model_cache().ok().flatten();
    let configured_models = dedup_models(cfg.configured_models());
    let cached_models = cache
        .as_ref()
        .and_then(|c| c.providers.get(&cfg.active_provider))
        .map(|entry| dedup_models(entry.models.iter().map(|model| model.id.clone()).collect()))
        .unwrap_or_default();

    let mut effective_models = if configured_models.is_empty() {
        cached_models.clone()
    } else {
        configured_models.clone()
    };

    if !effective_models.iter().any(|m| m == &cfg.default_model) {
        effective_models.insert(0, cfg.default_model.clone());
    }

    let active_cache_entry = cache
        .as_ref()
        .and_then(|c| c.providers.get(&cfg.active_provider));

    Ok(ModelCatalog {
        config_path: config_path.display().to_string(),
        cache_path: cache_path.display().to_string(),
        default_model: cfg.default_model.clone(),
        utility_small_model: cfg.utility_small_model.clone(),
        configured_models,
        cached_models,
        effective_models,
        model_options: build_model_catalog_entries(&cfg, cache.as_ref())?,
        cache_stale: active_cache_entry
            .map(provider_cache_is_stale)
            .unwrap_or(true),
        last_synced_unix: active_cache_entry.map(|entry| entry.last_synced_unix),
    })
}

pub async fn sync_remote_model_cache() -> Result<ModelCache, String> {
    let cfg = config::load_config()?;
    sync_remote_model_cache_with_config(&cfg).await
}

pub async fn sync_remote_model_cache_if_stale(
    cfg: &AppConfig,
) -> Result<Option<ModelCache>, String> {
    let cache = load_model_cache()?;

    let has_stale_provider = cfg.provider_keys().iter().any(|provider_key| {
        cache
            .as_ref()
            .and_then(|c| c.providers.get(provider_key))
            .map(provider_cache_is_stale)
            .unwrap_or(true)
    });

    if !has_stale_provider {
        return Ok(None);
    }

    let refreshed = sync_remote_model_cache_with_config(cfg).await?;
    Ok(Some(refreshed))
}

pub async fn sync_remote_model_cache_with_config(cfg: &AppConfig) -> Result<ModelCache, String> {
    let mut cache = load_model_cache()?.unwrap_or_default();
    cache.version = 3;

    let mut successful_providers = 0usize;
    let mut sync_errors = Vec::new();

    for provider_key in cfg.provider_keys() {
        match providers::fetch_remote_models(cfg, &provider_key).await {
            Ok(ids) => {
                successful_providers += 1;
                cache.providers.insert(
                    provider_key.clone(),
                    ProviderModelCache {
                        last_synced_unix: now_unix(),
                        models: dedup_model_descriptors(ids),
                        source_api_endpoint: cfg
                            .resolved_provider(&provider_key)
                            .map(|p| p.api_endpoint)
                            .unwrap_or_default(),
                    },
                );
            }
            Err(err) => {
                sync_errors.push(format!("{}: {}", provider_key, err));
            }
        }
    }

    if successful_providers == 0 {
        return Err(format!(
            "Failed to sync remote model cache for all providers: {}",
            sync_errors.join(" | ")
        ));
    }

    save_model_cache(&cache)?;
    Ok(cache)
}

fn model_cache_path() -> Result<std::path::PathBuf, String> {
    Ok(config::config_dir_path()?.join(MODEL_CACHE_FILE_NAME))
}

fn load_model_cache() -> Result<Option<ModelCache>, String> {
    let path = model_cache_path()?;
    if !path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read model cache {}: {e}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let parsed: ModelCache = serde_yaml::from_str(&raw)
        .map_err(|e| format!("Invalid YAML in model cache {}: {e}", path.display()))?;

    Ok(Some(parsed))
}

fn save_model_cache(cache: &ModelCache) -> Result<(), String> {
    let path = model_cache_path()?;
    let yaml = serde_yaml::to_string(cache)
        .map_err(|e| format!("Failed to serialize model cache: {e}"))?;

    fs::write(&path, yaml)
        .map_err(|e| format!("Failed to write model cache {}: {e}", path.display()))
}

fn provider_cache_is_stale(cache: &ProviderModelCache) -> bool {
    if cache.last_synced_unix <= 0 {
        return true;
    }
    let now = now_unix();
    now - cache.last_synced_unix >= MODEL_CACHE_SYNC_INTERVAL_SECONDS as i64
}

fn dedup_models(models: Vec<String>) -> Vec<String> {
    let mut set = BTreeSet::new();
    for model in models {
        let trimmed = model.trim();
        if !trimmed.is_empty() {
            set.insert(trimmed.to_string());
        }
    }
    set.into_iter().collect()
}

fn dedup_model_descriptors(models: Vec<ProviderModelDescriptor>) -> Vec<ProviderModelDescriptor> {
    let mut deduped: BTreeMap<String, ProviderModelDescriptor> = BTreeMap::new();

    for model in models {
        let id = model.id.trim().to_string();
        if id.is_empty() {
            continue;
        }

        let mut supported_reasoning_levels = Vec::new();
        for level in model.supported_reasoning_levels {
            let level = level.trim().to_lowercase();
            if level.is_empty()
                || supported_reasoning_levels
                    .iter()
                    .any(|existing| existing == &level)
            {
                continue;
            }
            supported_reasoning_levels.push(level);
        }

        if let Some(existing) = deduped.get_mut(&id) {
            // Preserve whichever entry carries richer reasoning metadata.
            if supported_reasoning_levels.len() > existing.supported_reasoning_levels.len() {
                existing.supported_reasoning_levels = supported_reasoning_levels;
            }
            continue;
        }

        deduped.insert(
            id.clone(),
            ProviderModelDescriptor {
                id,
                supported_reasoning_levels,
            },
        );
    }

    deduped.into_values().collect()
}

fn build_model_catalog_entries(
    cfg: &AppConfig,
    cache: Option<&ModelCache>,
) -> Result<Vec<ModelCatalogEntry>, String> {
    let mut entries = Vec::new();

    for (provider_key, provider) in &cfg.providers {
        for (model_key, model_cfg) in &provider.models {
            if !model_cfg.enabled {
                continue;
            }

            let cached_levels = cache
                .and_then(|cache| cache.providers.get(provider_key))
                .and_then(|provider_cache| {
                    provider_cache
                        .models
                        .iter()
                        .find(|model| model.id == model_cfg.model)
                        .map(|model| model.supported_reasoning_levels.as_slice())
                });

            let reasoning =
                providers::get_reasoning_capability(&provider.kind, &model_cfg.model, cached_levels)?;

            let configured_reasoning_effort = model_cfg
                .request
                .reasoning_effort
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .filter(|value| {
                    reasoning
                        .supported_reasoning_levels
                        .iter()
                        .any(|level| level == value)
                })
                .map(ToOwned::to_owned);

            entries.push(ModelCatalogEntry {
                profile_key: format!("{}/{}", provider_key, model_key),
                provider_key: provider_key.clone(),
                model_id: model_cfg.model.clone(),
                supports_reasoning: reasoning.supports_reasoning,
                supported_reasoning_levels: reasoning.supported_reasoning_levels,
                configured_reasoning_effort,
            });
        }
    }

    Ok(entries)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum CachedModelRecord {
    Id(String),
    Descriptor(ProviderModelDescriptor),
}

fn deserialize_cached_models<'de, D>(
    deserializer: D,
) -> Result<Vec<ProviderModelDescriptor>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let records = Vec::<CachedModelRecord>::deserialize(deserializer)?;
    let models = records
        .into_iter()
        .filter_map(|record| match record {
            CachedModelRecord::Id(id) => {
                let id = id.trim().to_string();
                if id.is_empty() {
                    None
                } else {
                    Some(ProviderModelDescriptor {
                        id,
                        supported_reasoning_levels: Vec::new(),
                    })
                }
            }
            CachedModelRecord::Descriptor(model) => Some(model),
        })
        .collect::<Vec<_>>();

    Ok(dedup_model_descriptors(models))
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{dedup_model_descriptors, ProviderModelDescriptor};

    #[test]
    fn dedup_prefers_descriptor_with_more_reasoning_levels() {
        let models = vec![
            ProviderModelDescriptor {
                id: "gpt-5.2-codex".to_string(),
                supported_reasoning_levels: Vec::new(),
            },
            ProviderModelDescriptor {
                id: "gpt-5.2-codex".to_string(),
                supported_reasoning_levels: vec![
                    "low".to_string(),
                    "medium".to_string(),
                    "high".to_string(),
                    "xhigh".to_string(),
                ],
            },
        ];

        let deduped = dedup_model_descriptors(models);
        assert_eq!(deduped.len(), 1);
        assert_eq!(
            deduped[0].supported_reasoning_levels,
            vec!["low", "medium", "high", "xhigh"]
        );
    }
}
