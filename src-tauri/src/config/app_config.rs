use crate::agentjax_err;
use crate::config::model_ref::{model_ref, parse_model_ref};
use crate::config::prompt_composer::compile_prompt_composer;
use crate::config::schema::{
    AppConfig, McpRuntimeConfig, McpServerConfig, McpToolSourcePolicyConfig, McpTransportKind,
    ModelRequestConfig, PluginManagerConfig, ProviderConfig, ProviderModelConfig,
    ResolvedModelConfig, ToolEnabledConfig, ToolManagerConfig, ToolSourcePolicyConfig,
};
use crate::error::AgentJaxResult;
use crate::provider_api::registry;
use std::collections::BTreeMap;

impl ProviderConfig {
    /// Normalize provider config: trim whitespace, fill defaults from registry schema,
    /// normalize models, and populate built-in models when none are configured.
    pub fn normalize_for_key(mut self, provider_key: &str) -> Self {
        self.kind = self.kind.trim().to_lowercase();
        if self.kind.is_empty() {
            self.kind = provider_key.to_string();
        }

        // Fill standard typed fields from the registered plugin schema defaults
        // only when the field is still at its default/empty value.
        if let Some(definition) = registry::provider_definition(&self.kind)
            && let Some(properties) = definition
                .config_schema
                .as_object()
                .and_then(|obj| obj.get("properties").and_then(|p| p.as_object()))
        {
            for (key, property_schema) in properties {
                let Some(default_val) = property_schema.get("default") else {
                    continue;
                };
                if default_val.is_null() {
                    continue;
                }
                self.fill_typed_field_from_schema(key, default_val);
            }
        }

        // Normalize api_endpoint: trim trailing slashes.
        self.api_endpoint = self.api_endpoint.trim().trim_end_matches('/').to_string();

        // Normalize realtime_endpoint: trim trailing slashes.
        if let Some(ref mut url) = self.realtime_endpoint {
            let trimmed = url.trim().trim_end_matches('/').to_string();
            if trimmed.is_empty() {
                self.realtime_endpoint = None;
            } else {
                *url = trimmed;
            }
        }

        // Normalize stream_transport: validate and fall back to provider default.
        let transport = self.stream_transport.trim().to_lowercase();
        if transport != "websocket" && transport != "sse" {
            self.stream_transport = registry::provider_definition(&self.kind)
                .map(|def| def.default_config.stream_transport.clone())
                .unwrap_or_else(|| {
                    if self.supports_websockets {
                        "websocket".to_string()
                    } else {
                        "sse".to_string()
                    }
                });
        } else {
            self.stream_transport = transport;
        }
        if !self.supports_websockets && self.stream_transport == "websocket" {
            self.stream_transport = "sse".to_string();
        }

        // Normalize map-typed fields (trim keys and values).
        let normalize_map = |m: BTreeMap<String, String>| -> BTreeMap<String, String> {
            m.into_iter()
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .filter(|(k, _)| !k.is_empty())
                .collect()
        };
        self.http_headers = normalize_map(std::mem::take(&mut self.http_headers));
        self.env_http_headers = normalize_map(std::mem::take(&mut self.env_http_headers));
        self.query_params = normalize_map(std::mem::take(&mut self.query_params));

        // Normalize model configs.
        let mut normalized_models = BTreeMap::new();
        for (raw_key, mut model_cfg) in std::mem::take(&mut self.models) {
            let model_key = raw_key.trim().to_string();
            if model_key.is_empty() {
                continue;
            }
            if let Some(ref n) = model_cfg.name {
                let trimmed = n.trim().to_string();
                model_cfg.name = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                };
            }
            model_cfg.request.normalize();
            normalized_models.insert(model_key, model_cfg);
        }
        self.models = normalized_models;

        // Auto-populate models from the provider definition's builtin_models
        // when none are configured yet.
        if self.models.is_empty() {
            if let Some(definition) = registry::provider_definition(&self.kind) {
                for model in &definition.builtin_models {
                    if !self.models.contains_key(&model.id) {
                        self.models.insert(
                            model.id.clone(),
                            ProviderModelConfig {
                                name: None,
                                api_protocol: None,
                                enabled: true,
                                request: ModelRequestConfig::default(),
                            },
                        );
                    }
                }
            }
        }

        self
    }

    /// Map a plugin config schema key to the corresponding typed field and fill it
    /// if it's still at its default value. Unknown keys go to `extension_fields`.
    fn fill_typed_field_from_schema(&mut self, key: &str, default_val: &serde_json::Value) {
        match key {
            "credential" | "credentialEnv" => {
                // These are sensitive — never fill from schema default at this stage.
                // The schema has default: null for credential and default: "DEEPSEEK_API_KEY"
                // for credentialEnv, but we should only set credentialEnv when it's empty
                // and the schema has a non-empty default.
                if self.credential_env.is_none() {
                    if let Some(s) = default_val.as_str().filter(|s| !s.is_empty()) {
                        self.credential_env = Some(s.to_string());
                    }
                }
            }
            "apiEndpoint" => {
                if self.api_endpoint.is_empty() {
                    if let Some(s) = default_val.as_str().filter(|s| !s.is_empty()) {
                        self.api_endpoint = s.trim_end_matches('/').to_string();
                    }
                }
            }
            "httpHeaders" => {
                if self.http_headers.is_empty() {
                    if let Some(obj) = default_val.as_object() {
                        self.http_headers = obj
                            .iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect();
                    }
                }
            }
            "envHttpHeaders" => {
                if self.env_http_headers.is_empty() {
                    if let Some(obj) = default_val.as_object() {
                        self.env_http_headers = obj
                            .iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect();
                    }
                }
            }
            "queryParams" => {
                if self.query_params.is_empty() {
                    if let Some(obj) = default_val.as_object() {
                        self.query_params = obj
                            .iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect();
                    }
                }
            }
            "modelsEndpointCandidates" => {
                if self.models_endpoint_candidates.is_empty() {
                    if let Some(arr) = default_val.as_array() {
                        self.models_endpoint_candidates = arr
                            .iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();
                    }
                }
            }
            "realtimeEndpoint" => {
                if self.realtime_endpoint.is_none() {
                    if let Some(s) = default_val.as_str().filter(|s| !s.is_empty()) {
                        self.realtime_endpoint = Some(s.trim_end_matches('/').to_string());
                    }
                }
            }
            "supportsWebsockets" => {
                // Keep the default true — only override if explicitly set.
            }
            "streamTransport" => {
                if self.stream_transport == "sse" {
                    if let Some(s) = default_val.as_str().filter(|s| !s.is_empty()) {
                        self.stream_transport = s.to_string();
                    }
                }
            }
            "requestTimeoutSeconds" => {
                if self.request_timeout_seconds.is_none() {
                    self.request_timeout_seconds = default_val.as_u64();
                }
            }
            "requestMaxRetries" => {
                if self.request_max_retries.is_none() {
                    self.request_max_retries = default_val.as_u64().map(|v| v as u32);
                }
            }
            "streamMaxRetries" => {
                if self.stream_max_retries.is_none() {
                    self.stream_max_retries = default_val.as_u64().map(|v| v as u32);
                }
            }
            "streamIdleTimeoutMs" => {
                if self.stream_idle_timeout_ms.is_none() {
                    self.stream_idle_timeout_ms = default_val.as_u64();
                }
            }
            "websocketConnectTimeoutMs" => {
                if self.websocket_connect_timeout_ms.is_none() {
                    self.websocket_connect_timeout_ms = default_val.as_u64();
                }
            }
            // Unknown key → store in extension_fields for provider-specific settings.
            other => {
                if !self.extension_fields.contains_key(other) {
                    self.extension_fields
                        .insert(other.to_string(), default_val.clone());
                }
            }
        }
    }
}

impl ModelRequestConfig {
    pub fn normalize(&mut self) {
        // Reasoning is now a structured ReasoningConfig with a ReasoningEffort
        // enum — no string normalization needed. The unused reasoning_effort
        // string field has been replaced.
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

impl PluginManagerConfig {
    pub fn normalize(mut self) -> Self {
        let mut normalized = BTreeMap::new();
        for (raw_key, entry) in std::mem::take(&mut self.plugins) {
            let key = raw_key.trim().to_lowercase();
            if key.is_empty() {
                continue;
            }
            normalized.insert(key, entry);
        }
        self.plugins = normalized;
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

use crate::config::agent_config::AgentConfig;

impl AgentConfig {
    pub fn compile_prompt_assembly(
        &self,
    ) -> crate::config::prompt_composer::CompiledPromptAssembly {
        compile_prompt_composer(&self.prompt_composer)
    }
}

impl AppConfig {
    pub fn normalize(mut self) -> Self {
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
        self.providers = normalized_providers;

        {
            let rt = self.mcp.runtime().normalize();
            self.mcp.stdio = rt.stdio;
        }
        self.plugin_manager = self.plugin_manager.normalize();

        let mut normalized_mcp_servers = BTreeMap::new();
        for (raw_key, mcp_server) in std::mem::take(&mut self.mcp.servers) {
            let server_key = raw_key.trim().to_lowercase();
            if server_key.is_empty() {
                continue;
            }
            let server = mcp_server.normalize();
            normalized_mcp_servers.insert(server_key, server);
        }
        self.mcp.servers = normalized_mcp_servers;

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
        if let Some(model_cfg) = provider.models.get(&requested_model).cloned()
            && model_cfg.enabled
        {
            return Some((provider_key, provider, requested_model, model_cfg));
        }

        let matched = provider.models.iter().find_map(|(model_key, model_cfg)| {
            if model_cfg.enabled && model_key == &requested_model {
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

    /// Resolve a model profile using shared providers, falling back to the
    /// main agent's defaults when no agent context is available.
    pub fn resolve_model_profile(
        &self,
        requested: Option<&str>,
    ) -> AgentJaxResult<ResolvedModelConfig> {
        // Fall back to loading the main agent config for model defaults.
        let agent = crate::config::load_agent_config(crate::config::constants::DEFAULT_AGENT_ID)
            .unwrap_or_default()
            .normalize();
        self.resolve_model_profile_with_agent(requested, &agent)
    }

    /// Resolve a model profile using shared providers + agent-specific settings.
    ///
    /// This is the bridge method used by `FullConfig` — it uses `AppConfig`
    /// for provider resolution but reads model references and prompt composer
    /// from the `AgentConfig`.
    pub fn resolve_model_profile_with_agent(
        &self,
        requested: Option<&str>,
        agent: &AgentConfig,
    ) -> AgentJaxResult<ResolvedModelConfig> {
        let requested_ref = requested.map(str::trim).filter(|s| !s.is_empty());
        let chosen_ref = requested_ref.unwrap_or(&agent.default_model).to_string();

        let resolved = requested_ref
            .and_then(|value| self.resolve_model_ref(value))
            .or_else(|| self.resolve_model_ref(&agent.default_model))
            .or_else(|| self.resolve_model_ref(&agent.utility_small_model))
            .or_else(|| self.first_enabled_model_ref())
            .ok_or_else(|| {
                agentjax_err!(
                    format!("Model '{}' not found or disabled. Expected format: {{provider}}/{{model_id}}", chosen_ref),
                    Config
                )
            })?;

        let (provider_key, provider, model_key, model_cfg) = resolved;
        let prompt_assembly = agent.compile_prompt_assembly();

        let resolved_ref = model_ref(&provider_key, &model_key);
        Ok(ResolvedModelConfig {
            profile_key: resolved_ref.clone(),
            provider_key,
            provider: provider.clone(),
            model_id: model_key.clone(),
            model_ref: resolved_ref,
            system_prompt: prompt_assembly.instructions_text.clone(),
            prompt_assembly,
            request: model_cfg.request.clone(),
            timeout_seconds: provider.resolved_timeout_seconds(agent.request_timeout_seconds),
            api_protocol: model_cfg.api_protocol.clone(),
        })
    }

    pub fn provider_keys(&self) -> Vec<String> {
        let mut keys = self.providers.keys().cloned().collect::<Vec<_>>();
        keys.sort();
        keys
    }

    pub fn resolved_provider(&self, provider_key: &str) -> AgentJaxResult<ProviderConfig> {
        let key = provider_key.trim().to_lowercase();
        self.providers.get(&key).cloned().ok_or_else(|| {
            agentjax_err!(
                format!("Provider '{}' not found in config", provider_key),
                Config
            )
        })
    }
}
