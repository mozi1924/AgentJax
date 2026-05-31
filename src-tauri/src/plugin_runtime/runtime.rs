use super::{
    PluginInvocationContext, PluginManifest, PluginPackage, PluginProviderDefinition,
    PluginToolCall, PluginToolResult, RegisteredPluginTool, SandboxPolicy,
    registered_tools_for_manifest, sdk::create_sdk_module_loader,
};
use deno_core::{JsRuntime, RuntimeOptions, serde_v8, v8};
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

pub type PluginRuntimeResult<T> = Result<T, PluginRuntimeError>;

/// Errors surfaced by the plugin runtime boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginRuntimeError {
    InvalidManifest(String),
    DuplicatePlugin(String),
    UnknownPlugin(String),
    UnknownPluginTool {
        plugin_id: String,
        tool_name: String,
    },
    UnknownProvider {
        plugin_id: String,
        provider_kind: String,
    },
    Io(String),
    ManifestParse(String),
    InvalidEntrypoint(String),
    JavaScript(String),
    UnsupportedOperation(&'static str),
}

impl Display for PluginRuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidManifest(message) => write!(f, "invalid plugin manifest: {message}"),
            Self::DuplicatePlugin(plugin_id) => {
                write!(f, "plugin '{plugin_id}' is already registered")
            }
            Self::UnknownPlugin(plugin_id) => write!(f, "plugin '{plugin_id}' is not registered"),
            Self::UnknownPluginTool {
                plugin_id,
                tool_name,
            } => write!(
                f,
                "plugin '{plugin_id}' does not export a tool named '{tool_name}'"
            ),
            Self::UnknownProvider {
                plugin_id,
                provider_kind,
            } => write!(
                f,
                "plugin '{plugin_id}' does not export a provider kind '{provider_kind}'"
            ),
            Self::Io(message) => write!(f, "plugin io error: {message}"),
            Self::ManifestParse(message) => write!(f, "plugin manifest parse error: {message}"),
            Self::InvalidEntrypoint(message) => write!(f, "invalid plugin entrypoint: {message}"),
            Self::JavaScript(message) => write!(f, "plugin JavaScript error: {message}"),
            Self::UnsupportedOperation(operation) => {
                write!(f, "unsupported plugin runtime operation: {operation}")
            }
        }
    }
}

impl Error for PluginRuntimeError {}

// ─────────────────────────────────────────────────────────────────────────────
// Unified PluginRuntime trait
// ─────────────────────────────────────────────────────────────────────────────

/// Host-level API for plugin registries.
///
/// This trait unifies the responsibilities that were previously split across
/// `runtime.rs` (tool plugins) and `plugin_host.rs` (provider plugins):
///
/// 1. Manifest registration, validation, and discovery
/// 2. Tool plugin execution (via `AgentJaxPlugin.tools`)
/// 3. Provider plugin function calls (via `AgentJaxPlugin.providers`)
///
/// Each plugin gets its own persistent `JsRuntime` instance on registration,
/// avoiding repeated V8 isolate creation overhead.
pub trait PluginRuntime: Send {
    fn backend_name(&self) -> &'static str;
    fn register_package(&mut self, package: PluginPackage) -> PluginRuntimeResult<()>;
    fn unregister(&mut self, plugin_id: &str) -> PluginRuntimeResult<PluginManifest>;
    fn manifest(&self, plugin_id: &str) -> Option<&PluginManifest>;
    fn manifests(&self) -> Vec<&PluginManifest>;

    // ── Tool plugin execution ────────────────────────────────────────────

    fn registered_tools(&self) -> Vec<RegisteredPluginTool>;
    fn execute_tool_call(&mut self, call: PluginToolCall) -> PluginRuntimeResult<PluginToolResult>;

    // ── Provider plugin execution ────────────────────────────────────────

    /// Extract provider definitions from a plugin's JS entrypoint.
    fn provider_definitions(&mut self, plugin_id: &str)
        -> PluginRuntimeResult<Vec<PluginProviderDefinition>>;

    /// Call an arbitrary function on a specific provider within a plugin.
    fn call_provider_function<T: DeserializeOwned>(
        &mut self,
        plugin_id: &str,
        provider_kind: &str,
        function: &str,
        argument: serde_json::Value,
    ) -> PluginRuntimeResult<T>;

    /// Prepare a tool call with validated plugin/tool identity + sandbox.
    fn prepare_tool_call(
        &self,
        plugin_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
        context: PluginInvocationContext,
    ) -> PluginRuntimeResult<PluginToolCall> {
        let manifest = self
            .manifest(plugin_id)
            .ok_or_else(|| PluginRuntimeError::UnknownPlugin(plugin_id.to_string()))?;
        if !manifest.tools.iter().any(|tool| tool.name == tool_name) {
            return Err(PluginRuntimeError::UnknownPluginTool {
                plugin_id: plugin_id.to_string(),
                tool_name: tool_name.to_string(),
            });
        }
        Ok(PluginToolCall {
            plugin_id: plugin_id.to_string(),
            tool_name: tool_name.to_string(),
            arguments,
            context,
            sandbox: manifest.sandbox.clone(),
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PluginInstance — one persistent JsRuntime per registered plugin
// ─────────────────────────────────────────────────────────────────────────────

pub struct PluginInstance {
    manifest: PluginManifest,
    pub(crate) runtime: JsRuntime,
}

// Legacy trait methods kept as direct impl methods for backward compatibility.
// TODO(codex): migrate callers and remove these wrappers.
impl DenoCorePluginRuntime {
    /// Register a manifest directly (creates a temp JsRuntime).
    /// Prefer `register_package` instead.
    pub fn register_manifest(&mut self, manifest: PluginManifest) -> PluginRuntimeResult<()> {
        // Reuse the SDK module loader; no file root available.
        let module_loader = create_sdk_module_loader();
        let instance = PluginInstance::new(manifest, None, None, Some(module_loader))?;
        let plugin_id = instance.manifest.id.clone();
        self.plugins.insert(plugin_id, instance);
        Ok(())
    }

    /// Prepare a tool call with validation.
    pub fn prepare_tool_call(
        &self,
        plugin_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
        context: PluginInvocationContext,
    ) -> PluginRuntimeResult<PluginToolCall> {
        let manifest = self
            .manifest(plugin_id)
            .ok_or_else(|| PluginRuntimeError::UnknownPlugin(plugin_id.to_string()))?;
        if !manifest.tools.iter().any(|tool| tool.name == tool_name) {
            return Err(PluginRuntimeError::UnknownPluginTool {
                plugin_id: plugin_id.to_string(),
                tool_name: tool_name.to_string(),
            });
        }
        Ok(PluginToolCall {
            plugin_id: plugin_id.to_string(),
            tool_name: tool_name.to_string(),
            arguments,
            context,
            sandbox: manifest.sandbox.clone(),
        })
    }
}

impl PluginInstance {
    fn new(
        manifest: PluginManifest,
        root_dir: Option<PathBuf>,
        entrypoint_source: Option<String>,
        _module_loader: Option<Rc<dyn deno_core::ModuleLoader>>,
    ) -> PluginRuntimeResult<Self> {
        manifest.validate().map_err(PluginRuntimeError::InvalidManifest)?;

        let mut runtime = JsRuntime::new(RuntimeOptions::default());

        // 1. Evaluate the SDK bootstrap into the global scope so all shared
        //    functions (withQuery, event, headerMap, usageFrom, etc.) are
        //    available as globals inside the plugin entrypoint.
        // 1. Evaluate the SDK bootstrap so shared functions (withQuery, event,
        //    headerMap, usageFrom, etc.) are available as globals.
        let sdk_bootstrap: &'static str =
            include_str!("../../builtin-plugins/sdk/sdk-bootstrap.js");
        runtime.execute_script("<agentjax-sdk-bootstrap>", sdk_bootstrap).map_err(|err| {
            PluginRuntimeError::JavaScript(format!(
                "failed to evaluate SDK bootstrap: {err}"
            ))
        })?;

        // 2. Evaluate the plugin entrypoint into the persistent isolate so
        //    `globalThis.AgentJaxPlugin` stays alive for subsequent calls.
        let (entrypoint_name, source) = resolve_entrypoint_script(
            &manifest,
            root_dir.as_deref(),
            entrypoint_source,
        )?;
        runtime
            .execute_script(entrypoint_name, source)
            .map_err(|err| {
                PluginRuntimeError::JavaScript(format!(
                    "failed to evaluate plugin '{}' entrypoint: {err}",
                    manifest.id
                ))
            })?;

        Ok(Self {
            manifest,
            runtime,
        })
    }

    /// Call a JS function path: `AgentJaxPlugin.providers[kind][fn](arg)`.
    pub fn call_provider_function<T: DeserializeOwned>(
        &mut self,
        provider_kind: &str,
        function: &str,
        argument: serde_json::Value,
    ) -> PluginRuntimeResult<T> {
        let provider_kind_json = serde_json::to_string(provider_kind)
            .map_err(|e| PluginRuntimeError::JavaScript(e.to_string()))?;
        let function_json = serde_json::to_string(function)
            .map_err(|e| PluginRuntimeError::JavaScript(e.to_string()))?;
        let argument_json = serde_json::to_string(&argument)
            .map_err(|e| PluginRuntimeError::JavaScript(e.to_string()))?;

        let bridge = format!(
            r#"
(() => {{
  const plugin = globalThis.AgentJaxPlugin;
  if (!plugin || typeof plugin !== "object") {{
    throw new Error("AgentJaxPlugin is not defined.");
  }}
  const providers = plugin.providers;
  const providerKind = {provider_kind_json};
  const functionName = {function_json};
  const provider = Array.isArray(providers)
    ? providers.find((candidate) => candidate && candidate.kind === providerKind)
    : providers && providers[providerKind];
  if (!provider || typeof provider !== "object") {{
    throw new Error(`Provider '${{providerKind}}' is not exported by this plugin.`);
  }}
  const handler = provider[functionName];
  if (typeof handler !== "function") {{
    throw new Error(`Provider '${{providerKind}}' does not implement ${{functionName}}().`);
  }}
  const result = handler({argument_json});
  return result === undefined ? null : result;
}})()
"#
        );

        let result = self
            .runtime
            .execute_script("<agentjax-provider-call>", bridge)
            .map_err(|err| PluginRuntimeError::JavaScript(err.to_string()))?;

        deno_core::scope!(scope, &mut self.runtime);
        let local = v8::Local::new(scope, result);
        serde_v8::from_v8::<T>(scope, local)
            .map_err(|err| PluginRuntimeError::JavaScript(format!("invalid result: {err}")))
    }

    /// Call a tool handler: `AgentJaxPlugin.tools[name](args, context)`.
    ///
    /// Sets `globalThis.__agentjax_context__` before the call so the SDK's
    /// `getInvocationContext()` and related helpers can read context fields
    /// during tool execution.
    pub fn call_tool(
        &mut self,
        tool_name: &str,
        arguments: serde_json::Value,
        context: PluginInvocationContext,
    ) -> PluginRuntimeResult<PluginToolResult> {
        let tool_name_json = serde_json::to_string(tool_name)
            .map_err(|e| PluginRuntimeError::JavaScript(e.to_string()))?;
        let arguments_json = serde_json::to_string(&arguments)
            .map_err(|e| PluginRuntimeError::JavaScript(e.to_string()))?;
        // Serialize context for __agentjax_context__ injection
        let context_json = serde_json::to_string(&context)
            .map_err(|e| PluginRuntimeError::JavaScript(e.to_string()))?;

        let bridge = format!(
            r#"
(() => {{
  // Inject invocation context so SDK getInvocationContext() works.
  globalThis.__agentjax_context__ = {context_json};

  const plugin = globalThis.AgentJaxPlugin;
  if (!plugin || typeof plugin !== "object") {{
    throw new Error("AgentJaxPlugin is not defined.");
  }}
  const tools = plugin.tools;
  const toolName = {tool_name_json};
  const handler = tools && tools[toolName];
  if (typeof handler !== "function") {{
    throw new Error(`Tool '${{toolName}}' is not a function.`);
  }}
  const value = handler({arguments_json}, {context_json});
  if (value && typeof value === "object" && value.hasOwnProperty("ok")) {{
    return {{
      ok: Boolean(value.ok),
      output: value.hasOwnProperty("output") ? (value.output !== undefined && value.output !== null ? value.output : null) : null,
      error: value.error == null ? null : String(value.error),
    }};
  }}
  return {{ ok: true, output: value === undefined ? null : value, error: null }};
}})()
"#
        );

        let result = self
            .runtime
            .execute_script("<agentjax-tool-call>", bridge)
            .map_err(|err| PluginRuntimeError::JavaScript(err.to_string()))?;

        deno_core::scope!(scope, &mut self.runtime);
        let local = v8::Local::new(scope, result);
        serde_v8::from_v8::<PluginToolResult>(scope, local)
            .map_err(|err| PluginRuntimeError::JavaScript(format!("invalid result: {err}")))
    }

    /// Extract provider definitions from the plugin's entrypoint.
    pub fn extract_provider_definitions(&mut self) -> PluginRuntimeResult<Vec<PluginProviderDefinition>> {
        let result = self
            .runtime
            .execute_script(
                "<agentjax-provider-discovery>",
                r#"
(() => {
  const plugin = globalThis.AgentJaxPlugin;
  if (!plugin || typeof plugin !== "object" || plugin.providers == null) {
    return [];
  }
  const providers = plugin.providers;
  const metadata = (definition, fallbackKind) => {
    const value = definition && typeof definition === "object" ? definition : {};
    return {
      kind: value.kind || fallbackKind || "",
      displayName: value.displayName || "",
      configSchema: value.configSchema || { type: "object", properties: {} },
      defaultModelIds: Array.isArray(value.defaultModelIds) ? value.defaultModelIds : [],
      defaultPriority: value.defaultPriority,
      capabilities: value.capabilities,
      toolSchemaFormat: value.toolSchemaFormat,
    };
  };
  if (Array.isArray(providers)) {
    return providers.map((provider) => metadata(provider));
  }
  if (typeof providers === "object") {
    return Object.entries(providers).map(([kind, definition]) => metadata(definition, kind));
  }
  throw new Error("AgentJaxPlugin.providers must be an array or object.");
})()
"#,
            )
            .map_err(|err| PluginRuntimeError::JavaScript(err.to_string()))?;

        deno_core::scope!(scope, &mut self.runtime);
        let local = v8::Local::new(scope, result);
        serde_v8::from_v8::<Vec<PluginProviderDefinition>>(scope, local)
            .map_err(|err| PluginRuntimeError::JavaScript(format!("invalid provider definitions: {err}")))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DenoCorePluginRuntime — concrete implementation
// ─────────────────────────────────────────────────────────────────────────────

/// deno_core-backed plugin runtime that holds one persistent `JsRuntime` per
/// registered plugin and exposes a unified API for both provider and tool calls.
pub struct DenoCorePluginRuntime {
    default_sandbox_policy: SandboxPolicy,
    plugins: BTreeMap<String, PluginInstance>,
    /// Shared module loader pre-populated with SDK modules. Cloned for each new
    /// plugin instance so they all see the same `@agentjax/sdk` module.
    module_loader: Rc<dyn deno_core::ModuleLoader>,
}

impl DenoCorePluginRuntime {
    pub fn new(default_sandbox_policy: SandboxPolicy) -> Self {
        let module_loader = create_sdk_module_loader();
        Self {
            default_sandbox_policy,
            plugins: BTreeMap::new(),
            module_loader,
        }
    }

    /// Register a plugin package — creates a persistent JsRuntime for it.
    pub fn register_package(&mut self, package: PluginPackage) -> PluginRuntimeResult<()> {
        if self.plugins.contains_key(&package.manifest.id) {
            return Err(PluginRuntimeError::DuplicatePlugin(
                package.manifest.id.clone(),
            ));
        }

        let instance = PluginInstance::new(
            package.manifest,
            Some(package.root_dir),
            package.entrypoint_source,
            Some(self.module_loader.clone()),
        )?;

        let plugin_id = instance.manifest.id.clone();
        self.plugins.insert(plugin_id, instance);
        Ok(())
    }

    /// Unregister a plugin and drop its JsRuntime.
    pub fn unregister(&mut self, plugin_id: &str) -> PluginRuntimeResult<PluginManifest> {
        let instance = self
            .plugins
            .remove(plugin_id)
            .ok_or_else(|| PluginRuntimeError::UnknownPlugin(plugin_id.to_string()))?;
        Ok(instance.manifest)
    }

    pub fn manifest(&self, plugin_id: &str) -> Option<&PluginManifest> {
        self.plugins.get(plugin_id).map(|inst| &inst.manifest)
    }

    pub fn manifests(&self) -> Vec<&PluginManifest> {
        self.plugins.values().map(|inst| &inst.manifest).collect()
    }

    pub fn registered_tools(&self) -> Vec<RegisteredPluginTool> {
        self.plugins
            .values()
            .flat_map(|inst| registered_tools_for_manifest(&inst.manifest))
            .collect()
    }

    /// Extract provider definitions from a registered plugin.
    pub fn provider_definitions(
        &mut self,
        plugin_id: &str,
    ) -> PluginRuntimeResult<Vec<PluginProviderDefinition>> {
        let mut providers = self
            .plugins
            .get(plugin_id)
            .map(|inst| inst.manifest.providers.clone())
            .ok_or_else(|| PluginRuntimeError::UnknownPlugin(plugin_id.to_string()))?;

        if let Some(instance) = self.plugins.get_mut(plugin_id) {
            let js_providers = instance.extract_provider_definitions()?;
            providers.extend(js_providers);
        }

        Ok(providers)
    }

    /// Call a function on a specific provider within a registered plugin.
    pub fn call_provider_function<T: DeserializeOwned>(
        &mut self,
        plugin_id: &str,
        provider_kind: &str,
        function: &str,
        argument: serde_json::Value,
    ) -> PluginRuntimeResult<T> {
        let instance = self
            .plugins
            .get_mut(plugin_id)
            .ok_or_else(|| PluginRuntimeError::UnknownPlugin(plugin_id.to_string()))?;
        instance.call_provider_function(provider_kind, function, argument)
    }

    /// Execute a tool call on a registered plugin.
    pub fn execute_tool_call(&mut self, call: PluginToolCall) -> PluginRuntimeResult<PluginToolResult> {
        let instance = self
            .plugins
            .get_mut(&call.plugin_id)
            .ok_or_else(|| PluginRuntimeError::UnknownPlugin(call.plugin_id.clone()))?;

        // Apply execution timeout guard
        let _timeout_guard = install_execution_timeout(
            &mut instance.runtime,
            call.sandbox.max_execution_ms,
        );

        // IMPORTANT: deno_core module_loader causes HandleScope issues in
        // temporary async contexts. We evaluate the call synchronously but
        // avoid exposing the module_loader to this method.
        instance.call_tool(&call.tool_name, call.arguments, call.context)
    }

    pub fn backend_name(&self) -> &'static str {
        "deno_core"
    }

    pub fn default_sandbox_policy(&self) -> &SandboxPolicy {
        &self.default_sandbox_policy
    }

    /// Return the sandbox policy for a registered plugin.
    pub fn sandbox_policy(&self, plugin_id: &str) -> Option<&SandboxPolicy> {
        self.manifest(plugin_id).map(|m| &m.sandbox)
    }

    /// Check whether a plugin's sandbox allows network access to the given host.
    /// Host may be `None` for generic network checks.
    pub fn check_plugin_network(
        &self,
        plugin_id: &str,
        host: Option<&str>,
    ) -> PluginRuntimeResult<()> {
        let policy = self
            .sandbox_policy(plugin_id)
            .ok_or_else(|| PluginRuntimeError::UnknownPlugin(plugin_id.to_string()))?;
        policy
            .check_network(host)
            .map_err(|violation| PluginRuntimeError::JavaScript(violation.to_string()))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Legacy migration helpers (for provider_api compatibility)
// ─────────────────────────────────────────────────────────────────────────────

/// Legacy wrapper: load provider definitions from a `PluginPackage` before it's
/// registered in the runtime. Used by `provider_api::registry` during startup.
pub fn provider_definitions_for_package(
    package: &PluginPackage,
) -> PluginRuntimeResult<Vec<PluginProviderDefinition>> {
    let mut providers = package.manifest.providers.clone();

    let mut instance = create_temp_plugin_instance(package)?;
    let js_providers = instance.extract_provider_definitions()?;
    providers.extend(js_providers);
    Ok(providers)
}

/// Create a temporary `PluginInstance` from a `PluginPackage` (no persistent
/// registration). Used by legacy `plugin_host.rs` and `provider_api::registry`.
///
/// Unlike `DenoCorePluginRuntime::register_package`, each call creates a fresh
/// `JsRuntime`. This is safe for async callers that need synchronous JS calls
/// before/after `.await` boundaries since `JsRuntime` is not `Send`.
pub fn create_temp_plugin_instance(
    package: &PluginPackage,
) -> PluginRuntimeResult<PluginInstance> {
    let module_loader = create_sdk_module_loader();
    PluginInstance::new(
        package.manifest.clone(),
        Some(package.root_dir.clone()),
        package.entrypoint_source.clone(),
        Some(module_loader),
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn resolve_entrypoint_script(
    manifest: &PluginManifest,
    root_dir: Option<&Path>,
    entrypoint_source: Option<String>,
) -> PluginRuntimeResult<(String, String)> {
    if let Some(source) = entrypoint_source {
        return Ok((
            format!("<agentjax-plugin:{}>", manifest.id),
            source,
        ));
    }

    let root = root_dir.ok_or_else(|| {
        PluginRuntimeError::InvalidEntrypoint(format!(
            "plugin '{}' has no root directory and no embedded source",
            manifest.id
        ))
    })?;

    let entrypoint = Path::new(&manifest.entrypoint);
    let resolved = if entrypoint.is_absolute() {
        entrypoint.to_path_buf()
    } else {
        root.join(entrypoint)
    };

    let source = std::fs::read_to_string(&resolved).map_err(|err| {
        PluginRuntimeError::Io(format!(
            "failed to read plugin entrypoint '{}': {}",
            resolved.display(),
            err
        ))
    })?;

    Ok((resolved.to_string_lossy().to_string(), source))
}

// ─────────────────────────────────────────────────────────────────────────────
// Execution timeout guard
// ─────────────────────────────────────────────────────────────────────────────

struct ExecutionTimeoutGuard {
    done: Arc<AtomicBool>,
}

impl Drop for ExecutionTimeoutGuard {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
    }
}

fn install_execution_timeout(
    runtime: &mut JsRuntime,
    max_execution_ms: Option<u64>,
) -> Option<ExecutionTimeoutGuard> {
    let Some(max_execution_ms) = max_execution_ms else {
        return None;
    };
    if max_execution_ms == 0 {
        return None;
    }

    let done = Arc::new(AtomicBool::new(false));
    let done_for_timeout = done.clone();
    let handle = runtime.v8_isolate().thread_safe_handle();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(max_execution_ms));
        if !done_for_timeout.load(Ordering::SeqCst) {
            handle.terminate_execution();
        }
    });
    Some(ExecutionTimeoutGuard { done })
}
