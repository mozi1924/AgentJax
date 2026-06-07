//! Provider API — unified abstraction for model providers.
//!
//! This module is the central dispatch for all model interactions:
//!
//! - **Native protocol implementations** (`protocol/`) handle standard OpenAI
//!   API formats (Responses, Chat Completions, Embeddings) directly in Rust.
//! - **JS plugin adapters** (`streaming` module, kept for backward compat) handle
//!   non-standard API formats via deno_core.
//! - **Registry** manages provider metadata (capabilities, config schemas, plugins).
//!
//! The dispatch logic in `stream_response` and `embed` first checks if the
//! provider uses a known protocol. If yes, it routes to the native Rust
//! implementation. Otherwise, it falls back to the JS plugin path.

pub mod capabilities;
pub mod circuit_breaker;
pub mod core;
pub mod network;
pub(crate) mod plugin;
pub(crate) mod protocol;
pub mod registry;
pub mod retry;
pub mod types;

use crate::config::AgentConfig;
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::plugin_runtime::BuiltinModelDescriptor;
use serde_json::Value;

pub use capabilities::ProviderCapabilities;
pub use types::{
    EmbeddingRequest, EmbeddingResponse, ModelReasoningCapability, ProviderModelDescriptor,
    ProviderModelMetadata, ProviderPendingToolCall, ProviderStreamEvent, ResponseStreamRequest,
    ResponseStreamResult,
};

// ── Public API Functions ────────────────────────────────────────────────────

pub fn get_capabilities(provider_kind: &str) -> AgentJaxResult<ProviderCapabilities> {
    registry::provider_capabilities(provider_kind).ok_or_else(|| {
        AgentJaxError::config(format!(
            "Unsupported provider kind '{}'. Register a provider plugin or use kind 'openai-compatible' to enable it.",
            provider_kind
        ))
    })
}

pub fn get_tool_schema_format(
    provider_kind: &str,
) -> AgentJaxResult<crate::tools::ToolSchemaFormat> {
    registry::provider_tool_schema_format(provider_kind).ok_or_else(|| {
        AgentJaxError::config(format!(
            "Unsupported provider kind '{}'. Register a provider plugin or use kind 'openai-compatible' to enable it.",
            provider_kind
        ))
    })
}

pub fn extract_pending_tool_calls(
    _provider_kind: &str,
    output_items: &[Value],
) -> AgentJaxResult<Vec<ProviderPendingToolCall>> {
    Ok(core::extract_pending_tool_calls_from_output(output_items))
}

pub fn build_tool_result_input_item(
    _provider_kind: &str,
    call_id: &str,
    output: &str,
) -> AgentJaxResult<Value> {
    Ok(core::build_tool_result_input_item(call_id, output))
}

pub fn build_user_input_item(_provider_kind: &str, text: &str) -> AgentJaxResult<Value> {
    Ok(core::build_user_input_item(text))
}

pub fn compose_tool_continuation_input(
    _provider_kind: &str,
    output_items: &[Value],
    tool_results_items: Vec<Value>,
) -> AgentJaxResult<Vec<Value>> {
    Ok(core::compose_tool_continuation_input(
        output_items,
        tool_results_items,
    ))
}

// ── Streaming ──────────────────────────────────────────────────────────────

/// Stream a response from a model provider.
///
/// Dispatches to the appropriate protocol implementation (native Rust) or
/// falls back to the JS plugin path for non-standard provider APIs.
pub async fn stream_response<F>(
    config: &crate::config::AppConfig,
    agent: &AgentConfig,
    req: &ResponseStreamRequest,
    cancel_rx: &mut tokio::sync::watch::Receiver<bool>,
    on_delta: F,
) -> AgentJaxResult<ResponseStreamResult>
where
    F: FnMut(ProviderStreamEvent) -> AgentJaxResult<()> + Send,
{
    // Resolve the model profile using the agent's config for correct model
    // references (default_model, utility_small_model, prompt_composer, timeout).
    let resolved = config.resolve_model_profile_with_agent(req.model.as_deref(), agent)?;

    // Merge extra_body from the resolved model config (user-configured) into
    // the request. This ensures provider-specific fields like DeepSeek's
    // `thinking` are injected for both native protocol and JS plugin paths.
    // Utility callers (title generation, LCM summarization) can opt out via
    // `skip_model_extra_body` to avoid wasting tokens on thinking mode.
    let mut req = req.clone();
    if !req.skip_model_extra_body {
        for (key, value) in &resolved.request.extra_body {
            req.extra_body
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
    }

    // Apply the provider's reasoning schema: inject any extra_body fields
    // declared in the provider's plugin.json `reasoningSchema.enabledExtraBody`
    // when reasoning mode is enabled. This replaces the old hardcoded DeepSeek
    // check with a fully declarative approach — any provider can declare what
    // extra fields are needed for reasoning mode.
    if !req.skip_model_extra_body && req.reasoning.as_ref().is_some_and(|r| r.enabled) {
        if let Some(reasoning_schema) = registry::provider_definition(&resolved.provider.kind)
            .and_then(|def| def.reasoning_schema)
        {
            for (key, value) in &reasoning_schema.enabled_extra_body {
                req.extra_body
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        }
    }

    // Determine which protocol to use
    let protocol = resolve_protocol(
        &resolved.provider.kind,
        &resolved.model_id,
        resolved.api_protocol.as_deref(),
    );
    if let Some(ref protocol) = protocol {
        let mut on_delta = on_delta;
        protocol::stream_response(
            protocol.as_str(),
            config,
            &resolved.provider_key,
            &resolved.provider,
            &resolved.model_id,
            &req,
            cancel_rx,
            &mut on_delta,
        )
        .await
    } else {
        // Fall back to JS plugin path for non-standard providers
        plugin::stream_response(config, agent, &req, cancel_rx, on_delta).await
    }
}

// ── Embedding ──────────────────────────────────────────────────────────────

/// Embed text using a model provider.
///
/// Dispatches to the native protocol implementation for known embedding
/// protocols, or returns an error for unsupported provider kinds.
pub async fn embed_text(
    config: &crate::config::AppConfig,
    provider_key: &str,
    model_id: &str,
    input: &EmbeddingRequest,
) -> AgentJaxResult<EmbeddingResponse> {
    let provider = config.resolved_provider(provider_key)?;

    // Determine the protocol to use, with increasing precedence:
    // 1. Model `kind == "embedding"` → auto-route to embeddings protocol
    // 2. Per-model `api_protocol` override → explicit user choice
    let model_config = provider.models.get(model_id);
    let is_embedding_kind = model_config
        .and_then(|m| m.kind.as_deref())
        .map(|k| k == "embedding")
        .unwrap_or(false);

    // Check api_protocol override, or force "embeddings" if kind is "embedding"
    let protocol = match model_config.and_then(|m| m.api_protocol.as_deref()) {
        Some(proto) => resolve_protocol(&provider.kind, model_id, Some(proto)),
        None if is_embedding_kind => {
            resolve_protocol(&provider.kind, model_id, Some("embeddings"))
        }
        _ => resolve_protocol(&provider.kind, model_id, None),
    };

    match protocol.as_deref() {
        Some("embeddings") => protocol::embed("embeddings", &provider, model_id, input).await,
        Some(other) => Err(AgentJaxError::config(format!(
            "Provider '{}' uses protocol '{other}' which does not support embedding",
            provider_key
        ))),
        None => Err(AgentJaxError::config(format!(
            "Provider '{}' does not declare a supported embedding protocol",
            provider_key
        ))),
    }
}

// ── Remote Model Fetching ──────────────────────────────────────────────────

pub async fn fetch_remote_models(
    config: &crate::config::AppConfig,
    provider_key: &str,
) -> AgentJaxResult<Vec<ProviderModelDescriptor>> {
    let provider = config.resolved_provider(provider_key)?;

    // Collect builtin_models from the provider definition (Phase 2).
    let builtin_models: Vec<ProviderModelDescriptor> =
        registry::provider_definition(&provider.kind)
            .map(|def| {
                def.builtin_models
                    .iter()
                    .map(|m| ProviderModelDescriptor {
                        id: m.id.clone(),
                        supported_reasoning_levels: m
                            .supported_reasoning_levels
                            .clone()
                            .unwrap_or_default(),
                        kind: m.kind.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();

    // Try protocol-based model fetching (generic HTTP GET).
    let protocol = resolve_protocol(&provider.kind, "", None);
    let mut fetched_models: Vec<ProviderModelDescriptor> = if protocol.is_some() {
        let endpoint = format!("{}/models", provider.api_endpoint().trim_end_matches('/'));
        protocol::fetch_remote_models(
            &provider,
            &endpoint,
            crate::config::constants::DEFAULT_TIMEOUT_SECONDS,
        )
        .await
        .unwrap_or_default()
    } else {
        Vec::new()
    };

    // Merge builtin_models into the result. Remote-fetched models take
    // precedence (they may have more up-to-date info), but builtin_models
    // fill in models that aren't returned by the endpoint.
    let fetched_ids: std::collections::HashSet<String> =
        fetched_models.iter().map(|m| m.id.clone()).collect();

    for builtin in &builtin_models {
        if !fetched_ids.contains(&builtin.id) {
            fetched_models.push(builtin.clone());
        }
    }

    // If we have models from either source, return them.
    if !fetched_models.is_empty() {
        return Ok(fetched_models);
    }

    // Fall back to JS plugin path for legacy providers that have a plugin
    // but no builtin_models and no protocol-based endpoint.
    if crate::provider_api::registry::provider_plugin_package(&provider.kind).is_some() {
        plugin::fetch_remote_models(config, provider_key).await
    } else {
        log::debug!(
            "No protocol, builtin_models, or plugin for provider '{}' (kind: {}), skip model fetch",
            provider_key,
            provider.kind
        );
        Ok(Vec::new())
    }
}

// ── Provider Metadata ──────────────────────────────────────────────────────

pub fn get_reasoning_capability(
    provider_kind: &str,
    model_id: &str,
    cached_levels: Option<&[String]>,
) -> AgentJaxResult<ModelReasoningCapability> {
    // Phase 2: check builtin_models first — no JS needed.
    if let Some(def) = registry::provider_definition(provider_kind) {
        if let Some(model) = lookup_builtin_model(&def.builtin_models, model_id) {
            if let Some(levels) = &model.supported_reasoning_levels {
                return Ok(ModelReasoningCapability {
                    supports_reasoning: !levels.is_empty(),
                    supported_reasoning_levels: levels.clone(),
                });
            }
        }
        // If provider has builtin_models but this specific model wasn't found,
        // return a sensible default rather than falling through to JS.
        if !def.builtin_models.is_empty() {
            return Ok(ModelReasoningCapability {
                supports_reasoning: false,
                supported_reasoning_levels: Vec::new(),
            });
        }
        // If the provider definition exists but has no plugin package and
        // no builtin_models (e.g. "openai-compatible"), return a default
        // rather than falling through to the plugin path which will fail.
        if def.plugin_package.is_none() {
            return Ok(ModelReasoningCapability {
                supports_reasoning: false,
                supported_reasoning_levels: Vec::new(),
            });
        }
    } else {
        // Provider kind is not registered at all — return a sensible default
        // instead of falling through to the plugin path which will fail with
        // "registered without an executable plugin package".
        log::debug!(
            "Provider kind '{provider_kind}' is not registered, returning default reasoning capability"
        );
        return Ok(ModelReasoningCapability {
            supports_reasoning: false,
            supported_reasoning_levels: Vec::new(),
        });
    }

    // Fall back to JS plugin for providers that still use the legacy path.
    plugin::get_reasoning_capability(provider_kind, model_id, cached_levels)
}

/// Look up a model in `builtin_models` using exact match first, then
/// prefix/substring fallback (matching the heuristic that the old JS
/// `resolveModelMetadata` used).
fn lookup_builtin_model<'a>(
    builtin_models: &'a [BuiltinModelDescriptor],
    model_id: &str,
) -> Option<&'a BuiltinModelDescriptor> {
    let normalized = model_id.trim().to_lowercase();

    // 1. Exact match
    if let Some(m) = builtin_models.iter().find(|m| m.id == normalized) {
        return Some(m);
    }

    // 2. Substring / prefix fallback (longest-id-first to prefer specificity)
    let mut candidates: Vec<&BuiltinModelDescriptor> = builtin_models
        .iter()
        .filter(|m| normalized.contains(&m.id) || m.id.contains(&normalized))
        .collect();
    candidates.sort_by(|a, b| b.id.len().cmp(&a.id.len()));
    candidates.into_iter().next()
}

pub fn get_model_metadata(
    provider_kind: &str,
    model_id: &str,
) -> AgentJaxResult<ProviderModelMetadata> {
    // Phase 2: check builtin_models first — no JS needed.
    if let Some(def) = registry::provider_definition(provider_kind) {
        if let Some(model) = lookup_builtin_model(&def.builtin_models, model_id) {
            return Ok(ProviderModelMetadata {
                context_window: model.context_window,
                kind: model.kind.clone(),
            });
        }
        // If provider has builtin_models but no match was found, return
        // a default rather than falling through to JS.
        if !def.builtin_models.is_empty() {
            return Ok(ProviderModelMetadata {
                context_window: None,
                kind: None,
            });
        }
        // If the provider definition exists but has no plugin package and
        // no builtin_models (e.g. "openai-compatible"), return a default
        // rather than falling through to the plugin path which will fail.
        if def.plugin_package.is_none() {
            return Ok(ProviderModelMetadata {
                context_window: None,
                kind: None,
            });
        }
    } else {
        // Provider kind is not registered at all — return a sensible default
        // instead of falling through to the plugin path which will fail with
        // "registered without an executable plugin package".
        log::debug!(
            "Provider kind '{provider_kind}' is not registered, returning default model metadata"
        );
        return Ok(ProviderModelMetadata {
            context_window: None,
            kind: None,
        });
    }

    // Fall back to JS plugin path for legacy providers.
    plugin::get_model_metadata(provider_kind, model_id)
}

// ── Internal Helpers ───────────────────────────────────────────────────────

/// Map a provider kind and optional model ID to a known protocol.
///
/// Resolution order:
/// 1. `api_protocol` override (per-model config) — highest precedence
/// 2. `model_routing` rules from plugin.json (declarative glob-based routing)
/// 3. Legacy `supports_protocols` + hardcoded model prefix heuristic (fallback)
fn resolve_protocol(
    provider_kind: &str,
    model_id: &str,
    api_protocol_override: Option<&str>,
) -> Option<String> {
    // 1. Explicit per-model apiProtocol takes highest precedence
    if let Some(proto) = api_protocol_override {
        let trimmed = proto.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string().to_lowercase());
        }
    }

    let normalized_id = model_id.trim().to_lowercase();

    // 2. Try model_routing rules (declarative glob-based, from plugin.json).
    //    Rules are evaluated in declaration order; the first match wins.
    let routing = crate::provider_api::registry::provider_model_routing(provider_kind);
    if !routing.is_empty() {
        // If the model_id is empty (e.g. catalog sync), use the catch-all "*"
        // rule instead of trying to match, so we return a sensible default.
        if !normalized_id.is_empty() {
            for rule in &routing {
                if glob_match(&rule.pattern, &normalized_id) {
                    return Some(rule.protocol.clone());
                }
            }
        }
        // No match — use the catch-all "*" rule if present, otherwise first rule.
        let catch_all = routing.iter().find(|r| r.pattern == "*");
        return catch_all
            .or_else(|| routing.first())
            .map(|r| r.protocol.clone());
    }

    // 3. Fall back to supports_protocols + model ID heuristics (legacy).
    let protocols = crate::provider_api::registry::provider_supports_protocols(provider_kind);
    if protocols.is_empty() {
        return None;
    }
    if protocols.len() == 1 {
        return protocols.first().cloned();
    }

    // Multiple protocols — use model ID heuristics.
    // Matches known embedding model naming patterns:
    //   "text-embedding-*" (OpenAI), "*embed*" (nomic-embed-text, mxbai-embed-large),
    //   "bge-*", "e5-*", "acge*", "gte-*", "jina-embeddings-*", "sentence-*"
    // Also catches model IDs containing "/" like "BAAI/bge-m3"
    let is_embedding_model = normalized_id.contains("embed")
        || normalized_id.starts_with("text-embedding-")
        || normalized_id.starts_with("bge-")
        || normalized_id.starts_with("e5-")
        || normalized_id.starts_with("acge")
        || normalized_id.starts_with("gte-")
        || normalized_id.starts_with("jina-")
        || normalized_id.starts_with("sentence-")
        || normalized_id.ends_with("-embed")
        || normalized_id.ends_with("-embedding");

    if is_embedding_model {
        return protocols
            .iter()
            .find(|p| *p == "embeddings")
            .cloned()
            .or_else(|| protocols.first().cloned());
    }

    let has_responses = protocols.iter().any(|p| p == "responses");
    let has_chat = protocols.iter().any(|p| p == "chat_completions");
    if has_responses && has_chat && !normalized_id.is_empty() {
        // Hardcoded OpenAI model prefixes — only used when provider has no
        // model_routing rules. New providers should use modelRouting instead.
        let is_openai_native = [
            "gpt-5", "gpt-4", "gpt-3.5", "o1-", "o1 ", "o3-", "o3 ", "o4-", "o4 ",
        ]
        .iter()
        .any(|prefix| normalized_id.starts_with(prefix));
        if !is_openai_native {
            return Some("chat_completions".to_string());
        }
    }

    protocols.first().cloned()
}

/// Simple glob pattern matching supporting `*` wildcard.
///
/// - `*` matches any sequence of characters (including empty).
/// - All other characters match literally (case-sensitive).
/// - No support for `?`, `[...]`, or other glob features — only `*`.
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern_bytes = pattern.as_bytes();
    let text_bytes = text.as_bytes();
    let mut pi = 0; // pattern index
    let mut ti = 0; // text index
    let mut star_pi: Option<usize> = None; // position of last '*' in pattern
    let mut star_ti: usize = 0; // position in text matched by last '*'

    while ti < text_bytes.len() {
        if pi < pattern_bytes.len() && (pattern_bytes[pi] == b'*') {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if pi < pattern_bytes.len()
            && (pattern_bytes[pi] == b'?' || pattern_bytes[pi] == text_bytes[ti])
        {
            pi += 1;
            ti += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }

    // Skip remaining '*' in pattern
    while pi < pattern_bytes.len() && pattern_bytes[pi] == b'*' {
        pi += 1;
    }

    pi == pattern_bytes.len()
}

#[cfg(test)]
mod tests {
    use super::glob_match;

    #[test]
    fn glob_match_exact() {
        assert!(glob_match("hello", "hello"));
    }

    #[test]
    fn glob_match_wildcard_prefix() {
        assert!(glob_match("gpt-5*", "gpt-5-mini"));
        assert!(glob_match("gpt-5*", "gpt-5"));
    }

    #[test]
    fn glob_match_wildcard_suffix() {
        assert!(glob_match("*embedding*", "text-embedding-3-small"));
    }

    #[test]
    fn glob_match_catch_all() {
        assert!(glob_match("*", "anything-really"));
    }

    #[test]
    fn glob_match_no_match() {
        assert!(!glob_match("gpt-5*", "claude-3"));
    }
}
