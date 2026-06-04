//! Generates JSON Schema files from Rust config structs using `schemars`.
//!
//! Usage:
//!   cargo run --bin gen_schemas
//!
//! Output: Writes one JSON file per type to `gen/schemas/` relative to the
//! workspace root (CARGO_MANIFEST_DIR).

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let out_dir = resolve_out_dir();

    // Keep this list in sync with the config types the frontend cares about.
    // Each entry is a (file_name, schema_json_value) pair.
    macro_rules! schema_val {
        ($ty:ty) => {{
            let schema = schemars::schema_for!($ty);
            serde_json::to_value(&schema)
                .expect("serialize schema to JSON value")
        }};
    }

    let entries: Vec<(&str, serde_json::Value)> = vec![
        ("AppConfig", schema_val!(app_lib::config::AppConfig)),
        ("ProviderConfig", schema_val!(app_lib::config::ProviderConfig)),
        ("ProviderModelConfig", schema_val!(app_lib::config::ProviderModelConfig)),
        ("ModelRequestConfig", schema_val!(app_lib::config::ModelRequestConfig)),
        ("McpServerConfig", schema_val!(app_lib::config::McpServerConfig)),
        ("McpConfig", schema_val!(app_lib::config::McpConfig)),
        ("McpTransportKind", schema_val!(app_lib::config::McpTransportKind)),
        ("MemoryConfig", schema_val!(app_lib::config::MemoryConfig)),
        ("ContextManagementConfig", schema_val!(app_lib::config::ContextManagementConfig)),
        ("SubAgentConfig", schema_val!(app_lib::config::SubAgentConfig)),
        ("RagConfig", schema_val!(app_lib::config::RagConfig)),
        ("EmbeddingProviderConfig", schema_val!(app_lib::config::EmbeddingProviderConfig)),
        ("ToolManagerConfig", schema_val!(app_lib::config::ToolManagerConfig)),
        ("PluginManagerConfig", schema_val!(app_lib::config::PluginManagerConfig)),
        ("PluginEntryConfig", schema_val!(app_lib::config::PluginEntryConfig)),
        ("PluginPermissionOverride", schema_val!(app_lib::config::PluginPermissionOverride)),
        ("McpRuntimeConfig", schema_val!(app_lib::config::McpRuntimeConfig)),
        ("McpStdioRuntimeConfig", schema_val!(app_lib::config::McpStdioRuntimeConfig)),
        ("ToolEnabledConfig", schema_val!(app_lib::config::ToolEnabledConfig)),
        ("ToolSourcePolicyConfig", schema_val!(app_lib::config::ToolSourcePolicyConfig)),
        ("McpToolSourcePolicyConfig", schema_val!(app_lib::config::McpToolSourcePolicyConfig)),
        ("SettingsSnapshot", schema_val!(app_lib::config::SettingsSnapshot)),
        ("SettingsOption", schema_val!(app_lib::config::SettingsOption)),
        ("SecretStatus", schema_val!(app_lib::config::SecretStatus)),
        ("PromptComposerConfig", schema_val!(app_lib::config::PromptComposerConfig)),
        ("PromptBlock", schema_val!(app_lib::config::PromptBlock)),
        ("PromptBlockRole", schema_val!(app_lib::config::PromptBlockRole)),
        ("PromptBlockSource", schema_val!(app_lib::config::PromptBlockSource)),
    ];

    fs::create_dir_all(&out_dir).expect("create gen/schemas directory");

    let mut count = 0u32;
    for (name, schema) in &entries {
        let path = out_dir.join(format!("{name}.json"));
        let json = serde_json::to_string_pretty(schema)
            .unwrap_or_else(|e| panic!("serialize schema for {name}: {e}"));
        fs::write(&path, &json)
            .unwrap_or_else(|e| panic!("write {name}.json: {e}"));
        count += 1;
        println!("  ✓ {name}.json");
    }

    println!("\nGenerated {count} JSON Schema files in {:?}", out_dir);
}

/// Resolve `gen/schemas/` relative to `CARGO_MANIFEST_DIR`.
fn resolve_out_dir() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR must be set");
    let workspace_root = Path::new(&manifest)
        .parent()
        .expect("src-tauri should have a parent workspace root");
    workspace_root.join("gen").join("schemas")
}
