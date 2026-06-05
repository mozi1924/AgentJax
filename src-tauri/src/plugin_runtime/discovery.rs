use super::{PluginManifest, PluginRuntimeError, PluginRuntimeResult};
use std::path::{Path, PathBuf};

pub const PLUGIN_MANIFEST_FILE: &str = "plugin.json";

/// A plugin manifest loaded from disk with the directory needed to resolve its
/// entrypoint. The serialized manifest remains portable; filesystem details are
/// kept in this host-only wrapper.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginPackage {
    pub manifest: PluginManifest,
    pub root_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub entrypoint_source: Option<String>,
    /// Whether this plugin is compiled into the binary (built-in) or installed
    /// later by the user under `$AGENTJAX_HOME/plugins`.
    pub is_builtin: bool,
}

/// Load and validate a plugin package from either a plugin directory or a
/// direct `plugin.json` path.
pub fn load_plugin_package(path: impl AsRef<Path>) -> PluginRuntimeResult<PluginPackage> {
    let path = path.as_ref();
    let manifest_path = if path.is_dir() {
        path.join(PLUGIN_MANIFEST_FILE)
    } else {
        path.to_path_buf()
    };
    let root_dir = manifest_path
        .parent()
        .ok_or_else(|| {
            PluginRuntimeError::InvalidManifest(format!(
                "plugin manifest path '{}' has no parent directory",
                manifest_path.display()
            ))
        })?
        .to_path_buf();
    let contents = std::fs::read_to_string(&manifest_path).map_err(|err| {
        PluginRuntimeError::Io(format!(
            "failed to read plugin manifest '{}': {}",
            manifest_path.display(),
            err
        ))
    })?;
    let manifest = serde_json::from_str::<PluginManifest>(&contents).map_err(|err| {
        PluginRuntimeError::ManifestParse(format!(
            "failed to parse plugin manifest '{}': {}",
            manifest_path.display(),
            err
        ))
    })?;

    manifest.validate()?;
    validate_entrypoint_path(&root_dir, &manifest)?;

    Ok(PluginPackage {
        manifest,
        root_dir,
        manifest_path,
        entrypoint_source: None,
        is_builtin: false,
    })
}

/// Discover one-level plugin packages under a root directory. Missing roots are
/// treated as empty so callers can scan optional user/plugin locations safely.
pub fn discover_plugin_packages(
    root_dir: impl AsRef<Path>,
) -> PluginRuntimeResult<Vec<PluginPackage>> {
    let root_dir = root_dir.as_ref();
    if !root_dir.exists() {
        return Ok(Vec::new());
    }
    if !root_dir.is_dir() {
        return Err(PluginRuntimeError::Io(format!(
            "plugin root '{}' is not a directory",
            root_dir.display()
        )));
    }

    let mut packages = Vec::new();
    let root_manifest = root_dir.join(PLUGIN_MANIFEST_FILE);
    if root_manifest.is_file() {
        packages.push(load_plugin_package(&root_manifest)?);
    }

    let entries = std::fs::read_dir(root_dir).map_err(|err| {
        PluginRuntimeError::Io(format!(
            "failed to read plugin root '{}': {}",
            root_dir.display(),
            err
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| {
            PluginRuntimeError::Io(format!(
                "failed to read plugin root entry '{}': {}",
                root_dir.display(),
                err
            ))
        })?;
        let candidate = entry.path().join(PLUGIN_MANIFEST_FILE);
        if candidate.is_file() {
            packages.push(load_plugin_package(candidate)?);
        }
    }

    packages.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    Ok(packages)
}

/// Discover plugin packages from `$AGENTJAX_HOME/plugins`.
pub fn discover_home_plugin_packages() -> PluginRuntimeResult<Vec<PluginPackage>> {
    let plugins_dir = crate::agentjax_home::ensure_plugins_dir()
        .map_err(|e| PluginRuntimeError::Io(e.to_string()))?;
    discover_plugin_packages(plugins_dir)
}

/// Discover all plugin packages available to the host, including packages that
/// are compiled into the binary and packages installed under `$AGENTJAX_HOME`.
pub fn discover_all_plugin_packages() -> PluginRuntimeResult<Vec<PluginPackage>> {
    let mut packages = crate::plugin_runtime::builtin_plugin_packages();
    packages.extend(discover_home_plugin_packages()?);
    Ok(packages)
}

fn validate_entrypoint_path(root_dir: &Path, manifest: &PluginManifest) -> PluginRuntimeResult<()> {
    let entrypoint = Path::new(&manifest.entrypoint);
    if entrypoint.is_absolute() {
        return Err(PluginRuntimeError::InvalidEntrypoint(format!(
            "plugin '{}' entrypoint must be relative to its plugin directory",
            manifest.id
        )));
    }
    if entrypoint
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(PluginRuntimeError::InvalidEntrypoint(format!(
            "plugin '{}' entrypoint cannot contain '..'",
            manifest.id
        )));
    }

    let resolved = root_dir.join(entrypoint);
    if !resolved.is_file() {
        return Err(PluginRuntimeError::InvalidEntrypoint(format!(
            "plugin '{}' entrypoint '{}' does not exist",
            manifest.id,
            resolved.display()
        )));
    }

    Ok(())
}
