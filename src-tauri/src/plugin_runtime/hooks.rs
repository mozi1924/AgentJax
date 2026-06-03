//! Plugin hook system for context assembly lifecycle.
//!
//! Hooks allow plugins to participate in the context assembly pipeline at
//! well-defined points. Plugins register callbacks (via the SDK) that the
//! host invokes during request processing.
//!
//! This module is an integration point being designed — the hook system is
//! partially wired and will be fully integrated in a future release.

#![allow(dead_code)] // Reserved for future use — integration point being designed
//!
//! # Hook points
//!
//! | Hook | When | What plugins can do |
//! |---|---|---|
//! | `OnContextAssemble` | After history is loaded, before model request | Inspect/modify the assembled item list |
//! | `OnContextItemTransform` | For each item during assembly | Transform individual items |
//! | `OnToolResult` | After a plugin tool call completes | Post-process tool output |
//! | `OnBeforeTruncation` | Before context is truncated (by count or budget) | Mark items as protected from truncation |

use serde_json::Value;
use std::collections::HashMap;

// ── Hook point identifiers ────────────────────────────────────────────────

/// Well-known points in the context assembly lifecycle where plugins can
/// register callbacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(clippy::enum_variant_names)]
pub enum ContextHookPoint {
    /// After conversation history is loaded and transformed to input items,
    /// but before truncation. Receives the full item list.
    OnContextAssemble,

    /// Before truncation by count or token budget. Items can be marked as
    /// protected (never dropped).
    OnBeforeTruncation,

    /// After a plugin-originated tool call completes, before its output is
    /// wrapped into a `function_call_output` item.
    OnToolResult,
}

impl ContextHookPoint {
    /// Human-readable label for debug / logging.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OnContextAssemble => "onContextAssemble",
            Self::OnBeforeTruncation => "onBeforeTruncation",
            Self::OnToolResult => "onToolResult",
        }
    }
}

// ── Hook callback types ───────────────────────────────────────────────────

/// Context data passed to `OnContextAssemble` hooks.
#[derive(Debug, Clone)]
pub struct ContextAssembleData {
    /// Full list of assembled input items (read-only view).
    pub items: Vec<Value>,
    /// The conversation identifier.
    pub conversation_id: Option<String>,
    /// The model identifier for this request.
    pub model_id: Option<String>,
    /// Estimated token count of `items`.
    pub estimated_tokens: usize,
}

/// Result returned by `OnContextAssemble` hooks.
#[derive(Debug, Clone, Default)]
pub struct ContextAssembleResult {
    /// Items the hook wants to add to the context (appended after all hooks).
    pub extra_items: Vec<Value>,
    /// Items the hook wants to remove (matched by a caller-chosen identity key
    /// such as `id` or `call_id`).
    pub remove_ids: Vec<String>,
}

/// Context data passed to `OnToolResult` hooks.
#[derive(Debug, Clone)]
pub struct ToolResultData {
    pub call_id: String,
    pub tool_name: String,
    pub plugin_id: String,
    pub arguments: Value,
    pub output: Value,
    pub ok: bool,
    pub conversation_id: Option<String>,
}

/// Result returned by `OnToolResult` hooks.
#[derive(Debug, Clone)]
pub struct ToolResultTransform {
    /// Modified output to use instead of the original. `None` means no change.
    pub transformed_output: Option<Value>,
}

// ── Hook trait ────────────────────────────────────────────────────────────

/// A single hook callback registered by a plugin.
///
/// Each variant carries the plugin's identifier so the host can attribute
/// hook activity and enforce sandbox policies.
#[derive(Debug, Clone)]
pub enum ContextHook {
    /// Callback invoked during context assembly.
    Assemble {
        plugin_id: String,
        handler: fn(&ContextAssembleData) -> ContextAssembleResult,
    },
    /// Callback invoked after a plugin tool call completes.
    ToolResult {
        plugin_id: String,
        handler: fn(&ToolResultData) -> Option<ToolResultTransform>,
    },
}

impl ContextHook {
    pub fn plugin_id(&self) -> &str {
        match self {
            Self::Assemble { plugin_id, .. } => plugin_id,
            Self::ToolResult { plugin_id, .. } => plugin_id,
        }
    }

    pub fn hook_point(&self) -> ContextHookPoint {
        match self {
            Self::Assemble { .. } => ContextHookPoint::OnContextAssemble,
            Self::ToolResult { .. } => ContextHookPoint::OnToolResult,
        }
    }
}

// ── Hook registry ─────────────────────────────────────────────────────────

/// Central registry for plugin hooks.
///
/// The host creates one registry and passes it through the context assembly
/// pipeline. Plugins register hooks via the SDK (which calls into the host
/// through a bridge function).
#[derive(Debug, Clone)]
pub struct HookRegistry {
    /// Hooks organised by hook point, preserving registration order within
    /// each category.
    hooks: HashMap<ContextHookPoint, Vec<ContextHook>>,
}

impl HookRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            hooks: HashMap::new(),
        }
    }

    /// Register a hook callback.
    pub fn register(&mut self, hook: ContextHook) {
        let point = hook.hook_point();
        self.hooks.entry(point).or_default().push(hook);
    }

    /// Unregister all hooks for a given plugin.
    pub fn unregister_plugin(&mut self, plugin_id: &str) {
        for hooks in self.hooks.values_mut() {
            hooks.retain(|hook| hook.plugin_id() != plugin_id);
        }
    }

    /// Check whether any hooks are registered at the given point.
    pub fn has_hooks(&self, point: ContextHookPoint) -> bool {
        self.hooks.get(&point).is_some_and(|hooks| !hooks.is_empty())
    }

    /// Return all hooks registered at the given point.
    pub fn hooks_at(&self, point: ContextHookPoint) -> &[ContextHook] {
        self.hooks.get(&point).map_or(&[], |hooks| hooks.as_slice())
    }

    /// Invoke all `OnContextAssemble` hooks in registration order.
    ///
    /// Returns the aggregate result: all extra items appended in order, and
    /// all IDs to remove deduplicated.
    pub fn run_assemble_hooks(&self, data: &ContextAssembleData) -> ContextAssembleResult {
        let mut aggregate = ContextAssembleResult::default();
        for hook in self.hooks_at(ContextHookPoint::OnContextAssemble) {
            if let ContextHook::Assemble { handler, .. } = hook {
                let result = handler(data);
                aggregate.extra_items.extend(result.extra_items);
                aggregate.remove_ids.extend(result.remove_ids);
            }
        }
        aggregate
    }

    /// Invoke the `OnToolResult` hook for a specific plugin (if registered).
    ///
    /// Returns the transformed output if the hook returned one, or `None` to
    /// use the original output as-is.
    pub fn run_tool_result_hook(
        &self,
        plugin_id: &str,
        data: &ToolResultData,
    ) -> Option<ToolResultTransform> {
        for hook in self.hooks_at(ContextHookPoint::OnToolResult) {
            if let ContextHook::ToolResult { plugin_id: pid, handler } = hook
                && pid == plugin_id
                    && let Some(transform) = handler(data) {
                        return Some(transform);
                    }
        }
        None
    }

    /// Return a summary of registered hooks (for diagnostics).
    pub fn summary(&self) -> Vec<(ContextHookPoint, Vec<String>)> {
        let mut out = Vec::new();
        for (&point, hooks) in &self.hooks {
            let plugin_ids: Vec<String> = hooks.iter().map(|h| h.plugin_id().to_string()).collect();
            out.push((point, plugin_ids));
        }
        out
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── JS bridge helpers ─────────────────────────────────────────────────────

/// Serialize hook registration data for the JS bridge (used by the SDK).
///
/// Plugins call `AgentJax.registerHook(point, handler)` from JS, and the host
/// bridges that into a Rust-side `ContextHook`.
pub fn serialize_hook_registration(
    _plugin_id: &str,
    point: &str,
) -> super::PluginRuntimeResult<ContextHookPoint> {
    match point.trim() {
        "onContextAssemble" | "OnContextAssemble" => Ok(ContextHookPoint::OnContextAssemble),
        "onBeforeTruncation" | "OnBeforeTruncation" => Ok(ContextHookPoint::OnBeforeTruncation),
        "onToolResult" | "OnToolResult" => Ok(ContextHookPoint::OnToolResult),
        other => Err(super::PluginRuntimeError::InvalidManifest(format!("Unknown context hook point: '{other}'"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn register_and_run_assemble_hook() {
        let mut registry = HookRegistry::new();

        registry.register(ContextHook::Assemble {
            plugin_id: "test-plugin".to_string(),
            handler: |_data| ContextAssembleResult {
                extra_items: vec![json!({"role": "system", "content": [{"type": "input_text", "text": "Hook injected"}]})],
                remove_ids: vec![],
            },
        });

        assert!(registry.has_hooks(ContextHookPoint::OnContextAssemble));

        let data = ContextAssembleData {
            items: vec![],
            conversation_id: None,
            model_id: None,
            estimated_tokens: 0,
        };

        let result = registry.run_assemble_hooks(&data);
        assert_eq!(result.extra_items.len(), 1);
        assert_eq!(
            result.extra_items[0]["content"][0]["text"].as_str(),
            Some("Hook injected")
        );
    }

    #[test]
    fn register_and_run_tool_result_hook() {
        let mut registry = HookRegistry::new();

        registry.register(ContextHook::ToolResult {
            plugin_id: "plugin-a".to_string(),
            handler: |data| {
                if data.plugin_id == "plugin-a" {
                    Some(ToolResultTransform {
                        transformed_output: Some(json!({"hook": "transformed", "original": data.output})),
                    })
                } else {
                    None
                }
            },
        });

        let data = ToolResultData {
            call_id: "call_1".to_string(),
            tool_name: "my_tool".to_string(),
            plugin_id: "plugin-a".to_string(),
            arguments: json!({}),
            output: json!({"result": "original"}),
            ok: true,
            conversation_id: None,
        };

        let result = registry.run_tool_result_hook("plugin-a", &data);
        assert!(result.is_some());
        assert_eq!(
            result.unwrap().transformed_output.unwrap()["hook"].as_str(),
            Some("transformed")
        );
    }

    #[test]
    fn unregister_plugin_removes_all_its_hooks() {
        let mut registry = HookRegistry::new();

        registry.register(ContextHook::Assemble {
            plugin_id: "plugin-a".to_string(),
            handler: |_| ContextAssembleResult::default(),
        });
        registry.register(ContextHook::Assemble {
            plugin_id: "plugin-b".to_string(),
            handler: |_| ContextAssembleResult::default(),
        });

        assert_eq!(registry.hooks_at(ContextHookPoint::OnContextAssemble).len(), 2);

        registry.unregister_plugin("plugin-a");
        assert_eq!(registry.hooks_at(ContextHookPoint::OnContextAssemble).len(), 1);
        assert_eq!(
            registry.hooks_at(ContextHookPoint::OnContextAssemble)[0].plugin_id(),
            "plugin-b"
        );
    }

    #[test]
    fn empty_registry_has_no_hooks() {
        let registry = HookRegistry::new();
        assert!(!registry.has_hooks(ContextHookPoint::OnContextAssemble));
    }

    #[test]
    fn serialize_hook_point_names() {
        assert_eq!(
            serialize_hook_registration("p", "onContextAssemble").unwrap(),
            ContextHookPoint::OnContextAssemble
        );
        assert_eq!(
            serialize_hook_registration("p", "OnContextAssemble").unwrap(),
            ContextHookPoint::OnContextAssemble
        );
        assert!(serialize_hook_registration("p", "invalid").is_err());
    }
}
