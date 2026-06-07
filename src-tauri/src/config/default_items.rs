use crate::config::schema::{McpServerConfig, ProviderConfig, ProviderModelConfig};
use serde_json::Value;

/// Register a config path prefix → default-item mapping.
///
/// Each entry maps a JSON pointer-like path (e.g. `"mcp.servers"`) to the
/// `Default::default()` serialization of the corresponding Rust config struct.
/// These are used by `inject_default_items` to populate the `defaultItem` field
/// of collection schema nodes, replacing hardcoded JSON defaults.
struct DefaultItemEntry {
    path_prefix: &'static str,
    default_value: Value,
}

/// Collect all registered default-item entries.
fn registered_default_items() -> Vec<DefaultItemEntry> {
    // NOTE: `serde_json::to_value` on a Default::default() struct serializes
    // all fields with their zero/empty/default values.  Fields annotated with
    // `#[serde(skip_serializing_if)]` will be absent — the frontend treats
    // absent keys the same as `null` in the UI (use the field-level default
    // from the schema).
    vec![
        DefaultItemEntry {
            path_prefix: "mcp.servers",
            default_value: serde_json::to_value(McpServerConfig::default())
                .expect("McpServerConfig::default() must serialize"),
        },
        DefaultItemEntry {
            path_prefix: "providers",
            default_value: serde_json::to_value(ProviderConfig::default())
                .expect("ProviderConfig::default() must serialize"),
        },
    ]
}

// ── Post-processing helpers ───────────────────────────────────────────────────

/// Try to find a `defaultItem` for the given collection path.
fn lookup_default_item(path: &str) -> Option<Value> {
    // Exact match first, then prefix match.
    for entry in &registered_default_items() {
        if path == entry.path_prefix {
            return Some(entry.default_value.clone());
        }
    }

    // For nested model collections (e.g. `providers.<key>.models`), match
    // any path ending in `.models`. Also match the relative path `"models"`
    // used inside a provider's collection children.
    if path == "models" || path.ends_with(".models") {
        return Some(
            serde_json::to_value(ProviderModelConfig::default())
                .expect("ProviderModelConfig::default() must serialize"),
        );
    }

    None
}

/// Walk a settings-section JSON tree and inject `defaultItem` into every
/// `collection` node whose path has a registered default but no explicit
/// `defaultItem` in the schema file.
pub fn inject_default_items(section: &mut Value) {
    let object = match section.as_object_mut() {
        Some(obj) => obj,
        None => return,
    };

    // If this node is a collection with a path but no defaultItem, try to inject.
    if let Some(kind) = object.get("kind").and_then(Value::as_str)
        && kind == "collection"
            && let Some(path) = object.get("path").and_then(Value::as_str)
                && !object.contains_key("defaultItem")
                    && let Some(default_item) = lookup_default_item(path) {
                        object.insert("defaultItem".to_string(), default_item);
                    }

    // Recurse into children.
    if let Some(children) = object.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            inject_default_items(child);
        }
    }

    // Recurse into tabs.
    if let Some(tabs) = object.get_mut("tabs").and_then(Value::as_array_mut) {
        for tab in tabs {
            if let Some(tab_children) = tab.get_mut("children").and_then(Value::as_array_mut) {
                for child in tab_children {
                    inject_default_items(child);
                }
            }
        }
    }

    // Recurse into itemTemplate.
    if let Some(item_template) = object.get_mut("itemTemplate") {
        inject_default_items(item_template);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_server_default_serializes_with_snake_case_keys() {
        let value = serde_json::to_value(McpServerConfig::default())
            .expect("McpServerConfig must serialize");
        let obj = value.as_object().expect("default must be an object");

        // Key fields that the frontend CollectionEditor expects.
        assert_eq!(obj.get("enabled").map(|v| v.as_bool()), Some(Some(true)));
        assert_eq!(obj.get("command").and_then(|v| v.as_str()), Some(""));
        assert!(obj.contains_key("args"));
        assert!(obj.contains_key("env"));
        assert!(obj.contains_key("headers"));
        assert_eq!(obj.get("transport").and_then(|v| v.as_str()), Some("stdio"));

        // The special-case path "mcp.servers" must resolve.
        let injected = lookup_default_item("mcp.servers");
        assert!(injected.is_some(), "mcp.servers must have a default item");
    }

    #[test]
    fn provider_default_serializes() {
        let value =
            serde_json::to_value(ProviderConfig::default()).expect("ProviderConfig must serialize");
        let obj = value.as_object().expect("default must be an object");
        assert_eq!(obj.get("kind").and_then(|v| v.as_str()), Some(""));
        assert!(obj.contains_key("models"));
    }

    #[test]
    fn provider_model_default_resolves_for_model_paths() {
        let injected = lookup_default_item("providers.my-provider.models");
        assert!(injected.is_some(), ".models paths must resolve");

        let value = serde_json::to_value(ProviderModelConfig::default())
            .expect("ProviderModelConfig must serialize");
        let obj = value.as_object().expect("default must be an object");
        assert_eq!(obj.get("enabled").and_then(|v| v.as_bool()), Some(true));

        // Fields that are None/empty should be absent (skip_serializing_if).
        assert!(!obj.contains_key("name"), "None name should be skipped");
    }

    #[test]
    fn inject_into_collection_node_adds_default_item() {
        let mut section = serde_json::json!({
            "id": "test-section",
            "children": [{
                "kind": "collection",
                "id": "test-collection",
                "path": "mcp.servers",
                "valueType": "object_collection",
                "children": []
            }]
        });
        inject_default_items(&mut section);

        let collection = &section["children"][0];
        assert!(
            collection.get("defaultItem").is_some(),
            "defaultItem should have been injected"
        );
        assert_eq!(
            collection["defaultItem"]["enabled"],
            serde_json::json!(true)
        );
    }

    #[test]
    fn existing_default_item_is_not_overwritten() {
        let mut section = serde_json::json!({
            "id": "test-section",
            "children": [{
                "kind": "collection",
                "id": "test-collection",
                "path": "mcp.servers",
                "valueType": "object_collection",
                "defaultItem": {"custom": "value"},
                "children": []
            }]
        });
        inject_default_items(&mut section);

        let collection = &section["children"][0];
        assert_eq!(
            collection["defaultItem"]["custom"],
            serde_json::json!("value"),
            "existing defaultItem must not be overwritten"
        );
    }
}
