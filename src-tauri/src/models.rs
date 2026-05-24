use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{self, AppConfig};
use crate::providers;

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
    pub models: Vec<String>,
    pub source_api_endpoint: String,
}

impl Default for ModelCache {
    fn default() -> Self {
        Self {
            version: 2,
            providers: BTreeMap::new(),
        }
    }
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
        .map(|entry| dedup_models(entry.models.clone()))
        .unwrap_or_default();

    let mut effective_models = if configured_models.is_empty() {
        cached_models.clone()
    } else {
        configured_models.clone()
    };

    if !effective_models.iter().any(|m| m == &cfg.default_model) {
        effective_models.insert(0, cfg.default_model.clone());
    }

    let active_cache_entry = cache.as_ref().and_then(|c| c.providers.get(&cfg.active_provider));

    Ok(ModelCatalog {
        config_path: config_path.display().to_string(),
        cache_path: cache_path.display().to_string(),
        default_model: cfg.default_model.clone(),
        utility_small_model: cfg.utility_small_model.clone(),
        configured_models,
        cached_models,
        effective_models,
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
    cache.version = 2;

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
                        models: dedup_models(ids),
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

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
