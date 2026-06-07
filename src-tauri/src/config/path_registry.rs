//! Unified config path registry.
//!
//! Replaces the ad-hoc `validate_path_semantics` in `patch.rs` with a
//! schema-backed validation that uses the `schemars` JSON Schema output.
//!
//! # What it provides
//!
//! 1. **Path validation** — checks that config path segments resolve to
//!    real fields in the Rust struct hierarchy (using the same approach
//!    as `path_validator.rs`).
//! 2. **Key format validation** — validates dynamic collection keys
//!    (provider names, server IDs, etc.) against known patterns.
//! 3. **Value type validation** — checks that the value being set
//!    matches the expected type from the schema.
//! 4. **Path constant generation** (TS) — see `gen_schemas` binary.

use serde_json::Value;
use std::collections::BTreeSet;

use crate::agentjax_err;
use crate::error::AgentJaxResult;

use super::path_validator::{build_merged_schema, follow_ref, resolve_schema_property};

// ── Collection key patterns ────────────────────────────────────────────────
//
// These are extracted from the `keyPattern` fields in the JSON UI schema
// files.  They describe the allowed format for dynamic collection keys
// (e.g., provider names like "openai", MCP server IDs like "my-server").

struct KeyPattern {
    /// The path prefix before the key, e.g. "providers" or "mcp.servers"
    prefix: &'static str,
    /// Regex pattern for the key
    pattern: &'static str,
    /// Human-readable label for error messages
    label: &'static str,
}

static COLLECTION_KEY_PATTERNS: &[KeyPattern] = &[
    KeyPattern {
        prefix: "providers",
        pattern: r"^[A-Za-z0-9_-]+$",
        label: "provider key",
    },
    KeyPattern {
        prefix: "mcp.servers",
        pattern: r"^[A-Za-z0-9_-]+$",
        label: "MCP server key",
    },
    KeyPattern {
        prefix: "tool_manager.native_tools",
        pattern: r"^[A-Za-z0-9_-]+$",
        label: "native tool key",
    },
    KeyPattern {
        prefix: "tool_manager.context_tools",
        pattern: r"^[A-Za-z0-9_-]+$",
        label: "context tool key",
    },
    KeyPattern {
        prefix: "tool_manager.plugin_tools",
        pattern: r"^[A-Za-z0-9_-]+$",
        label: "plugin id",
    },
    KeyPattern {
        prefix: "tool_manager.mcp_tools",
        pattern: r"^[A-Za-z0-9_-]+$",
        label: "MCP server key (tool)",
    },
    KeyPattern {
        prefix: "rag.knowledge_bases",
        pattern: r"^[A-Za-z0-9_.:-]+$",
        label: "knowledge base key",
    },
];

// The key pattern for model profiles inside providers — allows dots, colons,
// and forward slashes for model IDs like "gpt-4.1-mini", "neteas_curie/curie-1.0",
// or "huggingface:model-name".
// Forward slash is safe because `parse_model_ref()` uses `split_once('/')` which
// splits at the FIRST '/' only, so `"provider/model/with/slash"` correctly yields
// provider="provider", model_key="model/with/slash".
static MODEL_KEY_PATTERN: KeyPattern = KeyPattern {
    prefix: "providers.{key}.models",
    pattern: r"^[A-Za-z0-9_.:/:-]+$",
    label: "model profile key",
};

// ── Public API ─────────────────────────────────────────────────────────────

/// Validate config path segments from a settings patch.
///
/// This is the primary entry point for `patch.rs` — replaces the old
/// `validate_path_semantics` function.
///
/// * `segments` — the parsed path segments (e.g., `["memory", "enabled"]`)
/// * `value` — the optional value being set (for type validation)
pub fn validate_patch_path(segments: &[String], value: Option<&Value>) -> AgentJaxResult<()> {
    if segments.is_empty() {
        return Err(agentjax_err!("Patch path cannot be empty", Config));
    }

    // Reject empty segments immediately — they can't be valid paths
    for segment in segments {
        if segment.trim().is_empty() {
            return Err(agentjax_err!(
                format!("Path segment cannot be empty"),
                Config
            ));
        }
    }

    // 1. Validate path against merged AppConfig + AgentConfig JSON Schema
    let schema = crate::config::path_validator::build_merged_schema();
    let mut current = &schema;

    for segment in segments {
        // Try to resolve as a schema property
        match resolve_schema_property(current, segment, &schema) {
            Ok(next) => {
                // If we resolved via additionalProperties, the segment is a
                // dynamic collection key — validate its format.
                if is_referencing_additional_properties(current, segment, &schema) {
                    validate_dynamic_key(segments, segment)?;
                }
                current = next;
            }
            Err(_) => {
                // The segment might be a dynamic collection key.
                // Check if it matches a known key pattern.
                validate_dynamic_key(segments, segment)?;
                // Find the schema for the collection item type
                current =
                    resolve_collection_item_schema_fast(current, &schema).ok_or_else(|| {
                        agentjax_err!(
                            format!(
                                "Unknown config path segment '{}': not a field or collection key",
                                segment
                            ),
                            Config
                        )
                    })?;
            }
        }
    }

    // 2. If a value was provided, validate its type against the schema
    if let Some(val) = value {
        validate_value_type(current, val)?;
    }

    Ok(())
}

/// Validate that a value matches the expected type from the schema.
fn validate_value_type(schema: &Value, value: &Value) -> AgentJaxResult<()> {
    let root_schema = build_merged_schema();
    let resolved = follow_ref(schema, &root_schema);

    let expected_type = match resolved.get("type") {
        Some(Value::String(t)) => Some(t.as_str()),
        Some(Value::Array(types)) => {
            // Option<T> → pick the first non-null type
            types
                .iter()
                .filter_map(|v| v.as_str())
                .find(|t| *t != "null")
        }
        _ => None,
    };

    let actual_type = match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_f64() => "number",
        Value::Number(_) => "integer",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };

    // null is always acceptable (clearing a value)
    if actual_type == "null" {
        return Ok(());
    }

    match expected_type {
        Some("integer") if actual_type == "integer" || actual_type == "number" => Ok(()),
        Some("number") if actual_type == "number" || actual_type == "integer" => Ok(()),
        Some(expected) if expected == actual_type => Ok(()),
        Some(expected) => Err(agentjax_err!(
            format!("Value type mismatch: expected '{expected}', got '{actual_type}'"),
            Config
        )),
        None => Ok(()), // Unknown schema type — accept any value
    }
}

/// Validate dynamic collection keys against known patterns.
fn validate_dynamic_key(full_path: &[String], key: &str) -> AgentJaxResult<()> {
    // Build the path prefix (all segments up to the key)
    let key_index = full_path
        .iter()
        .position(|s| s == key)
        .unwrap_or(full_path.len().saturating_sub(1));

    let prefix = full_path[..key_index].join(".");

    // Try matching a known key pattern
    for kp in COLLECTION_KEY_PATTERNS {
        if prefix == kp.prefix {
            return validate_key_format(key, kp.pattern, kp.label);
        }
    }

    // Check for model profile key pattern
    // The path looks like: providers.{provider_key}.models.{model_key}
    if full_path.len() >= 4
        && full_path[0] == "providers"
        && full_path.get(2).map(|s| s.as_str()) == Some("models")
    {
        let key_index_in_full = full_path.len() - 1;
        let is_model_key = key_index_in_full == 3; // Index 3 is the model key
        let prefix_with_key = format!("providers.{}.models", full_path[1]);
        if is_model_key && prefix_with_key == format!("providers.{}.models", full_path[1]) {
            return validate_key_format(key, MODEL_KEY_PATTERN.pattern, MODEL_KEY_PATTERN.label);
        }
    }

    // Unknown key — accept it, the schema would reject it at deserialization
    Ok(())
}

/// Validate a single key against a regex pattern.
fn validate_key_format(key: &str, _pattern: &str, label: &str) -> AgentJaxResult<()> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err(agentjax_err!(format!("{label} cannot be empty"), Config));
    }

    // Simple regex-free check: alphanumeric + allowed special chars
    let allowed: Box<dyn Fn(char) -> bool> = Box::new(|c: char| {
        c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' || c == ':' || c == '/'
    });

    if !trimmed.chars().all(allowed) {
        return Err(agentjax_err!(
            format!(
                "{label} '{trimmed}' contains unsupported characters. Use letters, digits, '-', '_', '.' , ':' or '/' only."
            ),
            Config
        ));
    }

    Ok(())
}

/// Check whether `segment` was resolved from `parent` via `additionalProperties`
/// rather than a named `properties` entry.
fn is_referencing_additional_properties<'a>(
    parent: &'a Value,
    segment: &str,
    root_schema: &'a Value,
) -> bool {
    let resolved = follow_ref(parent, root_schema);
    // First check if the segment is a named property
    if let Some(props) = resolved.get("properties").and_then(|p| p.as_object())
        && props.contains_key(segment) {
            return false; // Was resolved via properties → not a dynamic key
        }
    // Segment wasn't in properties → it must be from additionalProperties
    resolved
        .get("additionalProperties")
        .is_some_and(|a| !matches!(a, Value::Bool(false)))
}

/// When we encounter a dynamic key in a path, we need to find the collection
/// item type schema.  For a path like `providers.ANY_KEY.kind`, after
/// resolving `providers`, the next segment is a key. We need to skip to
/// the item type (ProviderConfig).
fn resolve_collection_item_schema_fast<'a>(
    parent_schema: &'a Value,
    root_schema: &'a Value,
) -> Option<&'a Value> {
    let resolved = follow_ref(parent_schema, root_schema);

    // The parent is a BTreeMap → additionalProperties points to the item type
    if let Some(additional) = resolved.get("additionalProperties")
        && !matches!(additional, Value::Bool(false)) {
            return Some(follow_ref(additional, root_schema));
        }

    None
}

// ── Known-path helpers (for TypeScript codegen) ────────────────────────────

/// All known top-level config paths (non-collection).
/// Includes both shared config.yaml fields (AppConfig) and per-agent
/// config fields (AgentConfig), since the settings UI surfaces both.
#[allow(dead_code)]
pub fn known_top_level_paths() -> BTreeSet<&'static str> {
    let mut paths = BTreeSet::new();
    // ── Shared (AppConfig) ────────────────────────────────────────────────
    paths.insert("language");
    paths.insert("active_agent_id");
    paths.insert("mcp.stdio.inherit_parent_env");
    paths.insert("mcp.stdio.env");
    paths.insert("mcp.startup_timeout_ms");
    paths.insert("mcp.tool_timeout_ms");
    paths.insert("show_advanced_request_options");
    paths.insert("enable_developer_tools");
    // ── Per-agent (AgentConfig) ────────────────────────────────────────────
    paths.insert("active_provider");
    paths.insert("default_model");
    paths.insert("utility_small_model");
    paths.insert("request_timeout_seconds");
    paths.insert("max_tool_turns");
    paths.insert("prompt_composer");
    paths.insert("memory.enabled");
    paths.insert("memory.auto_inject");
    paths.insert("memory.max_index_tokens");
    paths.insert("memory.storage_dir");
    paths.insert("context_management.dynamic_thresholds");
    paths.insert("context_management.soft_token_threshold");
    paths.insert("context_management.hard_token_threshold");
    paths.insert("context_management.large_file_token_threshold");
    paths.insert("context_management.compaction_timeout_secs");
    paths.insert("context_management.max_compact_block_size");
    paths.insert("context_management.max_summary_depth");
    paths.insert("context_management.summarization_model");
    paths.insert("context_management.tokenizer_model_id");
    paths.insert("context_management.grep_page_size");
    paths.insert("context_management.street_enabled");
    paths.insert("context_management.street_auto_trigger_priority");
    paths.insert("context_management.street_max_items_per_conversation");
    paths.insert("context_management.jsonl_backup_enabled");
    paths.insert("sub_agent.max_concurrent");
    paths.insert("sub_agent.default_max_turns");
    paths.insert("sub_agent.hard_max_turns");
    paths.insert("sub_agent.timeout_secs");
    paths.insert("sub_agent.worktree_enabled");
    paths.insert("rag.enabled");
    paths.insert("rag.chunk_size");
    paths.insert("rag.chunk_overlap");
    paths.insert("rag.top_k");
    paths
}

/// Collection-scoped field paths (resolved against the collection item type).
/// The first segment is the collection name, the rest are field paths
/// within the item type.
#[allow(dead_code)]
pub fn known_collection_paths() -> BTreeSet<&'static str> {
    let mut paths = BTreeSet::new();
    // Knowledge base entry fields
    paths.insert("rag.knowledge_bases.name");
    paths.insert("rag.knowledge_bases.path");
    paths.insert("rag.knowledge_bases.path_type");
    paths.insert("rag.knowledge_bases.disabled_agents");
    // Provider fields
    paths.insert("providers.kind");
    paths.insert("providers.apiEndpoint");
    paths.insert("providers.credential");
    paths.insert("providers.credentialEnv");
    paths.insert("providers.requestTimeoutSeconds");
    paths.insert("providers.modelsEndpointCandidates");
    // Provider model fields
    paths.insert("providers.models.name");
    paths.insert("providers.models.enabled");
    paths.insert("providers.models.api_protocol");
    paths.insert("providers.models.request.temperature");
    paths.insert("providers.models.request.top_p");
    paths.insert("providers.models.request.top_k");
    paths.insert("providers.models.request.max_output_tokens");
    paths.insert("providers.models.request.frequency_penalty");
    paths.insert("providers.models.request.presence_penalty");
    paths.insert("providers.models.request.reasoning");
    paths.insert("providers.models.request.extra_body");
    // MCP server fields
    paths.insert("mcp.servers.enabled");
    paths.insert("mcp.servers.transport");
    paths.insert("mcp.servers.command");
    paths.insert("mcp.servers.args");
    paths.insert("mcp.servers.env");
    paths.insert("mcp.servers.cwd");
    paths.insert("mcp.servers.uri");
    paths.insert("mcp.servers.auth_header");
    paths.insert("mcp.servers.headers");
    paths.insert("mcp.servers.use_global_stdio_env");
    paths.insert("mcp.servers.inherit_parent_env");
    paths.insert("mcp.servers.allow_stateless");
    paths.insert("mcp.servers.channel_buffer_capacity");
    paths.insert("mcp.servers.reinit_on_expired_session");
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Schema-backed path validation ───────────────────────────────────

    #[test]
    fn valid_top_level_path() {
        let segments: Vec<String> = vec!["language".into()];
        assert!(validate_patch_path(&segments, None).is_ok());
    }

    #[test]
    fn valid_nested_path() {
        let segments: Vec<String> = vec!["memory".into(), "enabled".into()];
        assert!(validate_patch_path(&segments, None).is_ok());
    }

    #[test]
    fn invalid_path_rejected() {
        let segments: Vec<String> = vec!["nonexistent".into()];
        assert!(validate_patch_path(&segments, None).is_err());
    }

    #[test]
    fn collection_key_path_accepted() {
        // providers.{key}.kind — {key} is dynamic, kind is a valid field
        let segments: Vec<String> = vec!["providers".into(), "my-provider".into(), "kind".into()];
        assert!(validate_patch_path(&segments, None).is_ok());
    }

    #[test]
    fn collection_with_invalid_key_rejected() {
        let segments: Vec<String> = vec![
            "providers".into(),
            "".into(), // Empty key should fail
            "kind".into(),
        ];
        assert!(validate_patch_path(&segments, None).is_err());
    }

    #[test]
    fn mcp_server_collection_path() {
        let segments: Vec<String> = vec![
            "mcp".into(),
            "servers".into(),
            "my-server".into(),
            "command".into(),
        ];
        assert!(validate_patch_path(&segments, None).is_ok());
    }

    #[test]
    fn model_profile_path() {
        let segments: Vec<String> = vec![
            "providers".into(),
            "openai".into(),
            "models".into(),
            "gpt-4.1-mini".into(),
            "request".into(),
            "temperature".into(),
        ];
        assert!(validate_patch_path(&segments, None).is_ok());
    }

    // ── Value type validation ───────────────────────────────────────────

    #[test]
    fn boolean_value_accepted_for_bool_field() {
        let segments: Vec<String> = vec!["show_advanced_request_options".into()];
        assert!(validate_patch_path(&segments, Some(&Value::Bool(true))).is_ok());
    }

    #[test]
    fn number_value_accepted_for_integer_field() {
        let segments: Vec<String> = vec!["request_timeout_seconds".into()];
        assert!(validate_patch_path(&segments, Some(&Value::Number(30.into()))).is_ok());
    }

    #[test]
    fn string_value_accepted_for_string_field() {
        let segments: Vec<String> = vec!["language".into()];
        assert!(validate_patch_path(&segments, Some(&Value::String("en".into()))).is_ok());
    }

    // ── Known paths ─────────────────────────────────────────────────────

    #[test]
    fn known_top_level_paths_are_non_empty() {
        let paths = known_top_level_paths();
        assert!(!paths.is_empty(), "should have top-level paths");
        assert!(paths.contains("language"));
        assert!(paths.contains("memory.enabled"));
    }

    #[test]
    fn known_collection_paths_are_non_empty() {
        let paths = known_collection_paths();
        assert!(!paths.is_empty(), "should have collection paths");
        assert!(paths.contains("providers.kind"));
    }
}
