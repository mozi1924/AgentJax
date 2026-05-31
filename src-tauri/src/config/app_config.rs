use crate::config::constants::{DEFAULT_DEFAULT_MODEL_REF, DEFAULT_TIMEOUT_SECONDS};
use crate::config::model_ref::{model_ref, parse_model_ref};
use crate::config::prompt_composer::{compile_prompt_composer, normalize_prompt_composer};
use crate::config::schema::{
    AppConfig, McpRuntimeConfig, McpServerConfig, McpToolSourcePolicyConfig, McpTransportKind,
    ModelRequestConfig, ProviderConfig, ProviderModelConfig, ResolvedModelConfig,
    ToolEnabledConfig, ToolManagerConfig, ToolSourcePolicyConfig,
};
use crate::providers::registry;
use std::collections::BTreeMap;

impl ProviderConfig {
    pub fn normalize_for_key(mut self, provider_key: &str) -> Self {
        self.kind = self.kind.trim().to_lowercase();
        if self.kind.is_empty() {
            self.kind = provider_key.to_string();
        }

        self.normalize_legacy_custom_setting_keys();

        // Auto-complete custom_settings fields dynamically using registered config schema
        if let Some(definition) = registry::provider_definition(&self.kind) {
            if let Some(obj) = definition.config_schema.as_object() {
                if let Some(properties) = obj.get("properties").and_then(|p| p.as_object()) {
                    for (key, property_schema) in properties {
                        if !self.custom_settings.contains_key(key) {
                            let default_val = property_schema
                                .get("default")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null);
                            self.custom_settings.insert(key.clone(), default_val);
                        }
                    }
                }
            }

            // Auto-complete models if completely empty
            if self.models.is_empty() {
                for model_id in &definition.default_model_ids {
                    self.models.insert(
                        model_id.clone(),
                        ProviderModelConfig {
                            model: model_id.clone(),
                            enabled: true,
                            request: ModelRequestConfig::default(),
                        },
                    );
                }
            }
        }

        // Perform custom settings self-healing and normalization
        if let Some(serde_json::Value::String(api_endpoint)) =
            self.custom_settings.get("apiEndpoint")
        {
            let trimmed = api_endpoint.trim().trim_end_matches('/').to_string();
            self.custom_settings.insert(
                "apiEndpoint".to_string(),
                serde_json::Value::String(trimmed),
            );
        }

        if let Some(serde_json::Value::String(realtime_endpoint)) =
            self.custom_settings.get("realtimeEndpoint")
        {
            let trimmed = realtime_endpoint.trim().trim_end_matches('/').to_string();
            self.custom_settings.insert(
                "realtimeEndpoint".to_string(),
                serde_json::Value::String(trimmed),
            );
        }

        let supports_ws = self.supports_websockets();
        let mut transport = self.stream_transport().trim().to_lowercase();
        if transport != "websocket" && transport != "sse" {
            if let Some(definition) = registry::provider_definition(&self.kind) {
                transport = definition.default_config.stream_transport();
            } else {
                transport = if supports_ws {
                    "websocket".to_string()
                } else {
                    "sse".to_string()
                };
            }
        }
        if !supports_ws && transport == "websocket" {
            transport = "sse".to_string();
        }
        self.custom_settings.insert(
            "streamTransport".to_string(),
            serde_json::Value::String(transport),
        );

        for key in &["queryParams", "httpHeaders", "envHttpHeaders"] {
            if let Some(serde_json::Value::Object(obj)) = self.custom_settings.get(*key) {
                let mut normalized = serde_json::Map::new();
                for (k, v) in obj {
                    let k = k.trim().to_string();
                    if k.is_empty() {
                        continue;
                    }
                    if let serde_json::Value::String(s) = v {
                        normalized.insert(k, serde_json::Value::String(s.trim().to_string()));
                    }
                }
                self.custom_settings
                    .insert(key.to_string(), serde_json::Value::Object(normalized));
            }
        }

        // Normalize model configuration items
        let mut normalized_models = BTreeMap::new();
        for (raw_key, mut model_cfg) in std::mem::take(&mut self.models) {
            let model_key = raw_key.trim().to_string();
            if model_key.is_empty() {
                continue;
            }
            model_cfg.model = model_cfg.model.trim().to_string();
            if model_cfg.model.is_empty() {
                model_cfg.model = model_key.clone();
            }
            model_cfg.request.normalize();
            normalized_models.insert(model_key, model_cfg);
        }
        self.models = normalized_models;

        self
    }

    /// Preserve compatibility with older YAML files that stored provider
    /// extension settings as snake_case keys while the runtime schema now uses
    /// camelCase. Existing camelCase values win so a partially migrated config
    /// never has newer edits overwritten by legacy aliases.
    fn normalize_legacy_custom_setting_keys(&mut self) {
        for (legacy_key, canonical_key) in [
            ("api_endpoint", "apiEndpoint"),
            ("models_endpoint_candidates", "modelsEndpointCandidates"),
            ("query_params", "queryParams"),
            ("http_headers", "httpHeaders"),
            ("env_http_headers", "envHttpHeaders"),
            ("realtime_endpoint", "realtimeEndpoint"),
            ("supports_websockets", "supportsWebsockets"),
            ("stream_transport", "streamTransport"),
            ("credential_env", "credentialEnv"),
            ("request_timeout_seconds", "requestTimeoutSeconds"),
            ("request_max_retries", "requestMaxRetries"),
            ("stream_max_retries", "streamMaxRetries"),
            ("stream_idle_timeout_ms", "streamIdleTimeoutMs"),
            ("websocket_connect_timeout_ms", "websocketConnectTimeoutMs"),
        ] {
            if self.custom_settings.contains_key(canonical_key) {
                self.custom_settings.remove(legacy_key);
                continue;
            }
            if let Some(value) = self.custom_settings.remove(legacy_key) {
                self.custom_settings
                    .insert(canonical_key.to_string(), value);
            }
        }
    }

    pub fn resolved_credential(&self) -> Option<String> {
        let from_config = self
            .credential()
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);

        from_config.or_else(|| {
            std::env::var(&self.credential_env())
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
    }

    pub fn resolved_realtime_endpoint(&self) -> String {
        if let Some(url) = self.realtime_endpoint() {
            return url;
        }

        let api_endpoint = self.api_endpoint();
        if api_endpoint.starts_with("https://") {
            return format!("wss://{}", api_endpoint.trim_start_matches("https://"));
        }
        if api_endpoint.starts_with("http://") {
            return format!("ws://{}", api_endpoint.trim_start_matches("http://"));
        }

        format!("wss://{}", api_endpoint)
    }

    pub fn resolved_timeout_seconds(&self, global_default: u64) -> u64 {
        self.request_timeout_seconds().unwrap_or(global_default)
    }

    pub fn resolved_http_headers(&self) -> BTreeMap<String, String> {
        let mut headers = self.http_headers();

        for (header_name, env_key) in &self.env_http_headers() {
            let env_key = env_key.trim();
            if env_key.is_empty() {
                continue;
            }

            if let Ok(value) = std::env::var(env_key) {
                let value = value.trim();
                if !value.is_empty() {
                    headers.insert(header_name.clone(), value.to_string());
                }
            }
        }

        headers
    }
}

impl ModelRequestConfig {
    pub fn normalize(&mut self) {
        if let Some(value) = self.reasoning_effort.as_deref() {
            let trimmed = value.trim().to_lowercase();
            self.reasoning_effort = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            };
        }
    }
}

impl McpRuntimeConfig {
    pub fn normalize(mut self) -> Self {
        self.stdio.env = normalize_string_map(std::mem::take(&mut self.stdio.env));
        self
    }
}

impl McpServerConfig {
    pub fn normalize(mut self) -> Self {
        self.command = self.command.trim().to_string();
        self.args = self
            .args
            .iter()
            .map(|arg| arg.trim().to_string())
            .filter(|arg| !arg.is_empty())
            .collect();
        self.env = normalize_string_map(std::mem::take(&mut self.env));
        self.headers = normalize_string_map(std::mem::take(&mut self.headers));
        self.cwd = self
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        self.uri = self
            .uri
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        self.auth_header = self
            .auth_header
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        if matches!(self.channel_buffer_capacity, Some(0)) {
            self.channel_buffer_capacity = None;
        }

        match self.transport {
            McpTransportKind::Stdio => {
                self.uri = None;
                self.auth_header = None;
                self.headers.clear();
                self.allow_stateless = true;
                self.channel_buffer_capacity = None;
                self.reinit_on_expired_session = true;
            }
            McpTransportKind::StreamableHttp => {
                self.command.clear();
                self.args.clear();
                self.env.clear();
                self.cwd = None;
                self.use_global_stdio_env = true;
                self.inherit_parent_env = None;
            }
        }

        self
    }
}

impl ToolManagerConfig {
    pub fn normalize(mut self) -> Self {
        self.native_tools = normalize_tool_enabled_map(std::mem::take(&mut self.native_tools));
        self.plugin_tools =
            normalize_tool_source_policy_map(std::mem::take(&mut self.plugin_tools));
        self.mcp_tools = normalize_mcp_tool_source_policy_map(std::mem::take(&mut self.mcp_tools));
        self
    }
}

impl ToolSourcePolicyConfig {
    fn normalize(mut self) -> Self {
        self.tools = normalize_tool_enabled_map(std::mem::take(&mut self.tools));
        self
    }
}

impl McpToolSourcePolicyConfig {
    fn normalize(mut self) -> Self {
        self.exposure = self
            .exposure
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase());
        self.tools = normalize_tool_enabled_map(std::mem::take(&mut self.tools));
        self
    }
}

fn normalize_string_map(map: BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut normalized = BTreeMap::new();
    for (raw_key, raw_value) in map {
        let key = raw_key.trim().to_string();
        if key.is_empty() {
            continue;
        }
        normalized.insert(key, raw_value.trim().to_string());
    }
    normalized
}

fn normalize_tool_enabled_map(
    map: BTreeMap<String, ToolEnabledConfig>,
) -> BTreeMap<String, ToolEnabledConfig> {
    let mut normalized = BTreeMap::new();
    for (raw_key, policy) in map {
        let key = raw_key.trim().to_lowercase();
        if key.is_empty() {
            continue;
        }
        normalized.insert(key, policy);
    }
    normalized
}

fn normalize_tool_source_policy_map(
    map: BTreeMap<String, ToolSourcePolicyConfig>,
) -> BTreeMap<String, ToolSourcePolicyConfig> {
    let mut normalized = BTreeMap::new();
    for (raw_key, policy) in map {
        let key = raw_key.trim().to_lowercase();
        if key.is_empty() {
            continue;
        }
        normalized.insert(key, policy.normalize());
    }
    normalized
}

fn normalize_mcp_tool_source_policy_map(
    map: BTreeMap<String, McpToolSourcePolicyConfig>,
) -> BTreeMap<String, McpToolSourcePolicyConfig> {
    let mut normalized = BTreeMap::new();
    for (raw_key, policy) in map {
        let key = raw_key.trim().to_lowercase();
        if key.is_empty() {
            continue;
        }
        normalized.insert(key, policy.normalize());
    }
    normalized
}

impl AppConfig {
    pub fn compile_prompt_assembly(
        &self,
    ) -> crate::config::prompt_composer::CompiledPromptAssembly {
        compile_prompt_composer(&self.prompt_composer)
    }

    pub fn normalize(mut self) -> Self {
        self.active_provider = self.active_provider.trim().to_lowercase();
        self.prompt_composer = normalize_prompt_composer(self.prompt_composer);

        if self.request_timeout_seconds == 0 {
            self.request_timeout_seconds = DEFAULT_TIMEOUT_SECONDS;
        }

        let mut normalized_providers = BTreeMap::new();
        for (raw_key, provider) in std::mem::take(&mut self.providers) {
            let provider_key = raw_key.trim().to_lowercase();
            if provider_key.is_empty() {
                continue;
            }
            normalized_providers.insert(
                provider_key.clone(),
                provider.normalize_for_key(&provider_key),
            );
        }

        if normalized_providers.is_empty() {
            let default_provider = registry::default_provider_definition();
            normalized_providers.insert(
                default_provider.kind.clone(),
                default_provider.default_config.clone(),
            );
        }
        self.providers = normalized_providers;

        if self.active_provider.is_empty() || !self.providers.contains_key(&self.active_provider) {
            self.active_provider = self
                .providers
                .first_key_value()
                .map(|(k, _)| k.clone())
                .unwrap_or_else(|| registry::default_provider_kind().to_string());
        }

        let has_any_model = self
            .providers
            .values()
            .any(|provider| provider.models.values().any(|model| model.enabled));
        if !has_any_model {
            if let Some(provider) = self.providers.get_mut(&self.active_provider) {
                let fallback_model_id = registry::provider_definition(&provider.kind)
                    .and_then(|definition| definition.default_model_ids.first().cloned())
                    .unwrap_or_else(|| "gpt-5-mini".to_string());
                provider.models.insert(
                    fallback_model_id.to_string(),
                    ProviderModelConfig {
                        model: fallback_model_id.to_string(),
                        enabled: true,
                        request: ModelRequestConfig::default(),
                    },
                );
            }
        }

        self.default_model = self.default_model.trim().to_string();
        if self.default_model.is_empty() {
            self.default_model = DEFAULT_DEFAULT_MODEL_REF.to_string();
        }

        self.utility_small_model = self.utility_small_model.trim().to_string();
        if self.utility_small_model.is_empty() {
            self.utility_small_model = self.default_model.clone();
        }

        self.mcp_runtime = self.mcp_runtime.normalize();
        self.tool_manager = self.tool_manager.normalize();

        let mut normalized_mcp_servers = BTreeMap::new();
        for (raw_key, mcp_server) in std::mem::take(&mut self.mcp_servers) {
            let server_key = raw_key.trim().to_lowercase();
            if server_key.is_empty() {
                continue;
            }
            let server = mcp_server.normalize();
            normalized_mcp_servers.insert(server_key, server);
        }
        self.mcp_servers = normalized_mcp_servers;

        self
    }

    pub fn configured_models(&self) -> Vec<String> {
        let mut models = Vec::new();
        for (provider_key, provider) in &self.providers {
            for (model_key, model) in &provider.models {
                if model.enabled {
                    models.push(model_ref(provider_key, model_key));
                }
            }
        }
        models.sort();
        models
    }

    fn resolve_model_ref(
        &self,
        full_ref: &str,
    ) -> Option<(String, ProviderConfig, String, ProviderModelConfig)> {
        let (provider_key, requested_model) = parse_model_ref(full_ref)?;
        let provider = self.providers.get(&provider_key)?.clone();
        if let Some(model_cfg) = provider.models.get(&requested_model).cloned() {
            if model_cfg.enabled {
                return Some((provider_key, provider, requested_model, model_cfg));
            }
        }

        let matched = provider.models.iter().find_map(|(model_key, model_cfg)| {
            if model_cfg.enabled && model_cfg.model == requested_model {
                Some((model_key.clone(), model_cfg.clone()))
            } else {
                None
            }
        });

        matched.map(|(model_key, model_cfg)| (provider_key, provider, model_key, model_cfg))
    }

    fn first_enabled_model_ref(
        &self,
    ) -> Option<(String, ProviderConfig, String, ProviderModelConfig)> {
        let first_ref = self.configured_models().into_iter().next()?;
        self.resolve_model_ref(&first_ref)
    }

    pub fn resolve_model_profile(
        &self,
        requested: Option<&str>,
    ) -> Result<ResolvedModelConfig, String> {
        let requested_ref = requested.map(str::trim).filter(|s| !s.is_empty());
        let chosen_ref = requested_ref.unwrap_or(&self.default_model).to_string();

        let resolved = requested_ref
            .and_then(|value| self.resolve_model_ref(value))
            .or_else(|| self.resolve_model_ref(&self.default_model))
            .or_else(|| self.resolve_model_ref(&self.utility_small_model))
            .or_else(|| self.first_enabled_model_ref())
            .ok_or_else(|| {
                format!(
                    "Model '{}' not found or disabled. Expected format: {{provider}}/{{model_id}}",
                    chosen_ref
                )
            })?;

        let (provider_key, provider, model_key, model_cfg) = resolved;
        let prompt_assembly = self.compile_prompt_assembly();

        let resolved_ref = model_ref(&provider_key, &model_key);
        Ok(ResolvedModelConfig {
            profile_key: resolved_ref.clone(),
            provider_key,
            provider: provider.clone(),
            model_id: model_cfg.model.clone(),
            model_ref: resolved_ref,
            system_prompt: prompt_assembly.instructions_text.clone(),
            prompt_assembly,
            request: model_cfg.request.clone(),
            timeout_seconds: provider.resolved_timeout_seconds(self.request_timeout_seconds),
        })
    }

    pub fn utility_small_model_key(&self) -> &str {
        &self.utility_small_model
    }

    pub fn provider_keys(&self) -> Vec<String> {
        let mut keys = self.providers.keys().cloned().collect::<Vec<_>>();
        keys.sort();
        keys
    }

    pub fn resolved_provider(&self, provider_key: &str) -> Result<ProviderConfig, String> {
        let key = provider_key.trim().to_lowercase();
        self.providers
            .get(&key)
            .cloned()
            .ok_or_else(|| format!("Provider '{}' not found in config", provider_key))
    }
}
