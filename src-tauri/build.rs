use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    generate_builtin_plugin_manifest();
    tauri_build::build()
}

fn generate_builtin_plugin_manifest() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let plugins_root = Path::new(&manifest_dir).join("builtin-plugins");
    println!("cargo:rerun-if-changed={}", plugins_root.display());

    let mut plugin_dirs = Vec::new();
    if plugins_root.is_dir() {
        for entry in fs::read_dir(&plugins_root).expect("read builtin-plugins directory") {
            let entry = entry.expect("read builtin plugin directory entry");
            let path = entry.path();
            if path.join("plugin.json").is_file() && path.join("plugin.js").is_file() {
                plugin_dirs.push(path);
            }
        }
    }
    plugin_dirs.sort();

    for plugin_dir in &plugin_dirs {
        println!("cargo:rerun-if-changed={}", plugin_dir.display());
        println!(
            "cargo:rerun-if-changed={}",
            plugin_dir.join("plugin.json").display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            plugin_dir.join("plugin.js").display()
        );
    }

    let generated = format!(
        "&[\n{}]\n",
        plugin_dirs
            .iter()
            .map(|plugin_dir| builtin_plugin_tuple_source(plugin_dir))
            .collect::<Vec<_>>()
            .join("")
    );
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join("builtin_plugins.rs"), generated)
        .expect("write generated builtin plugin manifest");
}

fn builtin_plugin_tuple_source(plugin_dir: &Path) -> String {
    let package_dir = plugin_dir
        .file_name()
        .and_then(|value| value.to_str())
        .expect("builtin plugin directory name");
    let manifest_path = plugin_dir.join("plugin.json");
    let entrypoint_path = plugin_dir.join("plugin.js");

    format!(
        "    ({package_dir:?}, include_str!({manifest_path:?}), include_str!({entrypoint_path:?})),\n",
        manifest_path = manifest_path.display().to_string(),
        entrypoint_path = entrypoint_path.display().to_string(),
    )
}
