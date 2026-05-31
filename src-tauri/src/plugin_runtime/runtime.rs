use super::{
    PluginInvocationContext, PluginManifest, PluginPackage, PluginToolCall, PluginToolResult,
    RegisteredPluginTool, SandboxPolicy, registered_tools_for_manifest,
};
use deno_core::{JsRuntime, RuntimeOptions, v8};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
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

/// Host-level API for plugin registries.
///
/// The trait keeps the host boundary small while we wire the concrete deno_core
/// execution path in a later phase.
pub trait PluginRuntime {
    fn backend_name(&self) -> &'static str;
    fn default_sandbox_policy(&self) -> &SandboxPolicy;
    fn register_manifest(&mut self, manifest: PluginManifest) -> PluginRuntimeResult<()>;
    fn unregister_manifest(&mut self, plugin_id: &str) -> PluginRuntimeResult<PluginManifest>;
    fn manifest(&self, plugin_id: &str) -> Option<&PluginManifest>;
    fn manifests(&self) -> Vec<&PluginManifest>;
    fn sandbox_policy(&self, plugin_id: &str) -> Option<&SandboxPolicy> {
        self.manifest(plugin_id).map(|manifest| &manifest.sandbox)
    }

    fn registered_tools(&self) -> Vec<RegisteredPluginTool> {
        self.manifests()
            .into_iter()
            .flat_map(registered_tools_for_manifest)
            .collect()
    }

    /// Prepare a plugin invocation with validated plugin/tool identity and the
    /// sandbox policy that should be applied by the concrete runtime.
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

    fn execute_tool_call(
        &mut self,
        _call: PluginToolCall,
    ) -> PluginRuntimeResult<PluginToolResult> {
        Err(PluginRuntimeError::UnsupportedOperation(
            "execute_plugin_tool_call",
        ))
    }
}

/// Load provider definitions exported by a plugin package.
///
/// Provider plugins can declare static providers in `plugin.json` for portable
/// metadata, or export them from `globalThis.AgentJaxPlugin.providers` in their
/// JS entrypoint. The JS path is what built-in provider plugins use so provider
/// defaults and required config fields live with the plugin source instead of
/// in Rust registry code.
pub fn provider_definitions_for_package(
    package: &PluginPackage,
) -> PluginRuntimeResult<Vec<super::PluginProviderDefinition>> {
    let mut providers = package.manifest.providers.clone();
    providers.extend(execute_sync_js_provider_definitions(package)?);
    Ok(providers)
}

fn execute_sync_js_provider_definitions(
    package: &PluginPackage,
) -> PluginRuntimeResult<Vec<super::PluginProviderDefinition>> {
    let (entrypoint_name, source) = package_entrypoint_script(package)?;
    let mut runtime = JsRuntime::new(RuntimeOptions::default());
    runtime
        .execute_script(entrypoint_name, source)
        .map_err(|err| PluginRuntimeError::JavaScript(err.to_string()))?;

    let result = runtime
        .execute_script(
            "<agentjax-provider-plugin-discovery>",
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

    deno_core::scope!(scope, &mut runtime);
    let local = v8::Local::new(scope, result);
    deno_core::serde_v8::from_v8::<Vec<super::PluginProviderDefinition>>(scope, local).map_err(
        |err| {
            PluginRuntimeError::JavaScript(format!(
                "invalid provider definitions exported by plugin '{}': {err}",
                package.manifest.id
            ))
        },
    )
}

fn package_entrypoint_script(package: &PluginPackage) -> PluginRuntimeResult<(String, String)> {
    if let Some(source) = &package.entrypoint_source {
        return Ok((
            format!(
                "<agentjax-plugin:{}:{}>",
                package.manifest.id, package.manifest.entrypoint
            ),
            source.clone(),
        ));
    }

    let entrypoint_path = package.root_dir.join(&package.manifest.entrypoint);
    let source = std::fs::read_to_string(&entrypoint_path).map_err(|err| {
        PluginRuntimeError::Io(format!(
            "failed to read plugin entrypoint '{}': {}",
            entrypoint_path.display(),
            err
        ))
    })?;
    Ok((entrypoint_path.to_string_lossy().to_string(), source))
}

/// A small deno_core-backed runtime shell.
///
/// The concrete JS execution path will live here once plugin loading and
/// sandboxing are ready to be wired into the agent loop.
pub struct DenoCorePluginRuntime {
    runtime_options: deno_core::RuntimeOptions,
    default_sandbox_policy: SandboxPolicy,
    manifests: BTreeMap<String, PluginManifest>,
    plugin_roots: BTreeMap<String, PathBuf>,
    plugin_entrypoint_sources: BTreeMap<String, String>,
}

impl DenoCorePluginRuntime {
    /// Create a new runtime shell with a caller-provided `deno_core` config.
    pub fn new(
        runtime_options: deno_core::RuntimeOptions,
        default_sandbox_policy: SandboxPolicy,
    ) -> Self {
        Self {
            runtime_options,
            default_sandbox_policy,
            manifests: BTreeMap::new(),
            plugin_roots: BTreeMap::new(),
            plugin_entrypoint_sources: BTreeMap::new(),
        }
    }

    /// Expose the raw `deno_core` runtime options for future isolate setup.
    pub fn runtime_options(&self) -> &deno_core::RuntimeOptions {
        &self.runtime_options
    }

    pub fn register_package(&mut self, package: PluginPackage) -> PluginRuntimeResult<()> {
        self.insert_manifest(
            package.manifest,
            Some(package.root_dir),
            package.entrypoint_source,
        )
    }

    fn insert_manifest(
        &mut self,
        manifest: PluginManifest,
        root_dir: Option<PathBuf>,
        entrypoint_source: Option<String>,
    ) -> PluginRuntimeResult<()> {
        manifest
            .validate()
            .map_err(PluginRuntimeError::InvalidManifest)?;

        if self.manifests.contains_key(&manifest.id) {
            return Err(PluginRuntimeError::DuplicatePlugin(manifest.id));
        }

        if let Some(root_dir) = root_dir {
            self.plugin_roots.insert(manifest.id.clone(), root_dir);
        }
        if let Some(entrypoint_source) = entrypoint_source {
            self.plugin_entrypoint_sources
                .insert(manifest.id.clone(), entrypoint_source);
        }
        self.manifests.insert(manifest.id.clone(), manifest);
        Ok(())
    }

    fn entrypoint_path(&self, manifest: &PluginManifest) -> PluginRuntimeResult<PathBuf> {
        let entrypoint = Path::new(&manifest.entrypoint);
        if entrypoint.is_absolute() {
            return Ok(entrypoint.to_path_buf());
        }

        let root_dir = self.plugin_roots.get(&manifest.id).ok_or_else(|| {
            PluginRuntimeError::InvalidEntrypoint(format!(
                "plugin '{}' was registered without a root directory; use an absolute entrypoint or register a PluginPackage",
                manifest.id
            ))
        })?;
        Ok(root_dir.join(entrypoint))
    }

    fn entrypoint_script(
        &self,
        manifest: &PluginManifest,
    ) -> PluginRuntimeResult<(String, String)> {
        if let Some(source) = self.plugin_entrypoint_sources.get(&manifest.id) {
            return Ok((
                format!("<agentjax-builtin:{}:{}>", manifest.id, manifest.entrypoint),
                source.clone(),
            ));
        }

        let entrypoint_path = self.entrypoint_path(manifest)?;
        let source = std::fs::read_to_string(&entrypoint_path).map_err(|err| {
            PluginRuntimeError::Io(format!(
                "failed to read plugin entrypoint '{}': {}",
                entrypoint_path.display(),
                err
            ))
        })?;
        Ok((entrypoint_path.to_string_lossy().to_string(), source))
    }

    fn execute_sync_js_tool(
        &self,
        manifest: &PluginManifest,
        call: PluginToolCall,
    ) -> PluginRuntimeResult<PluginToolResult> {
        let (entrypoint_name, source) = self.entrypoint_script(manifest)?;
        let mut runtime = JsRuntime::new(RuntimeOptions::default());
        let _timeout_guard = install_execution_timeout(&mut runtime, call.sandbox.max_execution_ms);
        runtime
            .execute_script(entrypoint_name, source)
            .map_err(|err| PluginRuntimeError::JavaScript(err.to_string()))?;

        let tool_name = serde_json::to_string(&call.tool_name)
            .map_err(|err| PluginRuntimeError::JavaScript(err.to_string()))?;
        let arguments = serde_json::to_string(&call.arguments)
            .map_err(|err| PluginRuntimeError::JavaScript(err.to_string()))?;
        let context = serde_json::to_string(&call.context)
            .map_err(|err| PluginRuntimeError::JavaScript(err.to_string()))?;
        let bridge_source = format!(
            r#"
(() => {{
  const plugin = globalThis.AgentJaxPlugin;
  if (!plugin || typeof plugin !== "object") {{
    throw new Error("Plugin entrypoint must set globalThis.AgentJaxPlugin to an object.");
  }}
  const tools = plugin.tools;
  const toolName = {tool_name};
  const handler = tools && tools[toolName];
  if (typeof handler !== "function") {{
    throw new Error(`Plugin tool '${{toolName}}' is not a function.`);
  }}
  const value = handler({arguments}, {context});
  if (value && typeof value === "object" && Object.prototype.hasOwnProperty.call(value, "ok")) {{
    return {{
      ok: Boolean(value.ok),
      output: Object.prototype.hasOwnProperty.call(value, "output") && value.output !== undefined ? value.output : null,
      error: value.error === undefined || value.error === null ? null : String(value.error),
    }};
  }}
  return {{
    ok: true,
    output: value === undefined ? null : value,
    error: null,
  }};
}})()
"#
        );
        let result = runtime
            .execute_script("<agentjax-plugin-call>", bridge_source)
            .map_err(|err| PluginRuntimeError::JavaScript(err.to_string()))?;

        deno_core::scope!(scope, &mut runtime);
        let local = v8::Local::new(scope, result);
        deno_core::serde_v8::from_v8::<PluginToolResult>(scope, local)
            .map_err(|err| PluginRuntimeError::JavaScript(format!("invalid plugin result: {err}")))
    }
}

impl PluginRuntime for DenoCorePluginRuntime {
    fn backend_name(&self) -> &'static str {
        "deno_core"
    }

    fn default_sandbox_policy(&self) -> &SandboxPolicy {
        &self.default_sandbox_policy
    }

    fn register_manifest(&mut self, manifest: PluginManifest) -> PluginRuntimeResult<()> {
        self.insert_manifest(manifest, None, None)
    }

    fn unregister_manifest(&mut self, plugin_id: &str) -> PluginRuntimeResult<PluginManifest> {
        self.plugin_roots.remove(plugin_id);
        self.plugin_entrypoint_sources.remove(plugin_id);
        self.manifests
            .remove(plugin_id)
            .ok_or_else(|| PluginRuntimeError::UnknownPlugin(plugin_id.to_string()))
    }

    fn manifest(&self, plugin_id: &str) -> Option<&PluginManifest> {
        self.manifests.get(plugin_id)
    }

    fn manifests(&self) -> Vec<&PluginManifest> {
        self.manifests.values().collect()
    }

    fn execute_tool_call(&mut self, call: PluginToolCall) -> PluginRuntimeResult<PluginToolResult> {
        let manifest = self
            .manifest(&call.plugin_id)
            .ok_or_else(|| PluginRuntimeError::UnknownPlugin(call.plugin_id.clone()))?;
        self.execute_sync_js_tool(manifest, call)
    }
}

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
