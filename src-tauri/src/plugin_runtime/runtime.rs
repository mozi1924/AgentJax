use super::{PluginManifest, SandboxPolicy};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{Display, Formatter};

pub type PluginRuntimeResult<T> = Result<T, PluginRuntimeError>;

/// Errors surfaced by the plugin runtime boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginRuntimeError {
    InvalidManifest(String),
    DuplicatePlugin(String),
    UnknownPlugin(String),
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
}

/// A small deno_core-backed runtime shell.
///
/// The concrete JS execution path will live here once plugin loading and
/// sandboxing are ready to be wired into the agent loop.
pub struct DenoCorePluginRuntime {
    runtime_options: deno_core::RuntimeOptions,
    default_sandbox_policy: SandboxPolicy,
    manifests: BTreeMap<String, PluginManifest>,
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
        }
    }

    /// Expose the raw `deno_core` runtime options for future isolate setup.
    pub fn runtime_options(&self) -> &deno_core::RuntimeOptions {
        &self.runtime_options
    }

    fn insert_manifest(&mut self, manifest: PluginManifest) -> PluginRuntimeResult<()> {
        manifest
            .validate()
            .map_err(PluginRuntimeError::InvalidManifest)?;

        if self.manifests.contains_key(&manifest.id) {
            return Err(PluginRuntimeError::DuplicatePlugin(manifest.id));
        }

        self.manifests.insert(manifest.id.clone(), manifest);
        Ok(())
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
        self.insert_manifest(manifest)
    }

    fn unregister_manifest(&mut self, plugin_id: &str) -> PluginRuntimeResult<PluginManifest> {
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
}
