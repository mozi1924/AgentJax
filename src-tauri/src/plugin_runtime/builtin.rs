use super::{PluginManifest, PluginPackage, PluginRuntimeError, PluginRuntimeResult};
use std::path::PathBuf;

const BUILTIN_PLUGIN_SOURCES: &[(&str, &str, &str)] =
    include!(concat!(env!("OUT_DIR"), "/builtin_plugins.rs"));

/// Return plugin packages compiled into the AgentJax binary.
///
/// The source of truth for built-in plugins lives under
/// `src-tauri/builtin-plugins`. This loader intentionally compiles those
/// `plugin.json` and `plugin.js` files into the binary so built-in plugins and
/// user-installed plugins share the same manifest/package shape at runtime.
pub fn builtin_plugin_packages() -> Vec<PluginPackage> {
    BUILTIN_PLUGIN_SOURCES
        .iter()
        .map(|(package_dir, manifest_source, entrypoint_source)| {
            builtin_plugin_package(package_dir, manifest_source, entrypoint_source)
                .expect("built-in plugin package must be valid")
        })
        .collect()
}

fn builtin_plugin_package(
    package_dir: &str,
    manifest_source: &str,
    entrypoint_source: &str,
) -> PluginRuntimeResult<PluginPackage> {
    let manifest = serde_json::from_str::<PluginManifest>(manifest_source).map_err(|err| {
        PluginRuntimeError::ManifestParse(format!(
            "failed to parse built-in plugin manifest '{package_dir}/plugin.json': {err}"
        ))
    })?;

    manifest.validate().map_err(|err| {
        PluginRuntimeError::InvalidManifest(format!(
            "built-in plugin '{package_dir}' has an invalid manifest: {err}"
        ))
    })?;

    Ok(PluginPackage {
        manifest,
        root_dir: PathBuf::from(format!("src-tauri/builtin-plugins/{package_dir}")),
        manifest_path: PathBuf::from(format!(
            "src-tauri/builtin-plugins/{package_dir}/plugin.json"
        )),
        entrypoint_source: Some(entrypoint_source.to_string()),
    })
}
