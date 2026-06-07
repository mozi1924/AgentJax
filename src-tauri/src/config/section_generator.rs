//! Auto-generates simple settings UI sections from Rust config types.
//!
//! For config types with straightforward field layouts (no collections,
//! no conditional visibility), this module can produce a complete JSON
//! settings section by walking the `schemars` JSON Schema of the struct.
//!
//! # Type-to-control mapping
//!
//! | Rust type        | JSON Schema type  | UI control  |
//! |------------------|-------------------|-------------|
//! | `bool`           | boolean           | switch      |
//! | `String`         | string            | text        |
//! | `u32`/`u64`/etc  | integer           | number      |
//! | `f32`/`f64`      | number            | number      |
//! | enum (string)    | string + enum     | select      |
//! | `Option<T>`      | [T, null]         | same as T   |
//! | `BTreeMap`/map   | object + addl     | — skipped   |
//!
//! # i18n convention
//!
//! Title keys use the pattern: `settings.<section_id>.<field_name>.title`
//! Description keys use:       `settings.<section_id>.<field_name>.description`

use serde_json::Value;
use serde_json::json;

use crate::agentjax_err;
use crate::error::AgentJaxResult;

/// Metadata for a generated settings section.
pub struct SectionMeta {
    pub id: &'static str,
    pub title_key: &'static str,
    pub icon: &'static str,
    pub order: i64,
    pub description_key: &'static str,
}

/// Generate a simple settings section from a Rust config type.
///
/// `path_prefix` is the config path prefix for the fields (e.g., `"sub_agent"`
/// means fields will have paths like `"sub_agent.max_concurrent"`).
pub fn generate_simple_section<T: schemars::JsonSchema>(
    meta: &SectionMeta,
    path_prefix: &str,
) -> AgentJaxResult<Value> {
    let schema = schemars::schema_for!(T);
    let schema_val = serde_json::to_value(&schema)
        .map_err(|e| agentjax_err!(format!("serialize schema for {}: {e}", meta.id), Config))?;

    let children = build_children(&schema_val, path_prefix, &schema_val)?;

    let section = json!({
        "id": meta.id,
        "title": meta.title_key,
        "icon": meta.icon,
        "order": meta.order,
        "description": meta.description_key,
        "children": [{
            "kind": "group",
            "id": format!("{}-group", meta.id),
            "title": meta.title_key,
            "children": children
        }]
    });

    Ok(section)
}

/// Walk the properties of a JSON Schema value and generate field nodes.
fn build_children(
    schema_val: &Value,
    path_prefix: &str,
    root_schema: &Value,
) -> AgentJaxResult<Vec<Value>> {
    let mut fields = Vec::new();

    // Resolve $ref first
    let schema = follow_ref(schema_val, root_schema);

    let properties = match schema.get("properties").and_then(Value::as_object) {
        Some(props) => props,
        None => return Ok(fields),
    };

    for (field_name, prop_schema) in properties {
        let resolved = follow_ref(prop_schema, root_schema);

        // Skip map/collection types (BTreeMap/BTreeSet)
        if is_map_type(resolved) {
            continue;
        }

        let full_path = if path_prefix.is_empty() {
            field_name.clone()
        } else {
            format!("{path_prefix}.{field_name}")
        };

        if let Some(field_node) = build_field_node(field_name, &full_path, resolved) {
            fields.push(field_node);
        }
    }

    Ok(fields)
}

/// Check if a schema is a complex type that should be skipped.
///
/// Returns `true` for:
/// - Maps / BTreeMap (object + additionalProperties)
/// - Nested structs (object + properties — these have their own section)
/// - Arrays (not simple string arrays)
fn is_map_type(schema: &Value) -> bool {
    // Maps (BTreeMap → object with additionalProperties)
    if let Some(additional) = schema.get("additionalProperties") {
        return !matches!(additional, Value::Bool(false));
    }
    // Nested structs (object with named properties)
    if schema.get("type").and_then(Value::as_str) == Some("object") {
        if schema
            .get("properties")
            .and_then(Value::as_object)
            .is_some_and(|p| !p.is_empty())
        {
            return true;
        }
    }
    // Non-string arrays
    if schema.get("type").and_then(Value::as_str) == Some("array") {
        let items = schema.get("items");
        let is_string_array =
            items.and_then(|i| i.get("type")).and_then(Value::as_str) == Some("string");
        return !is_string_array;
    }
    false
}

/// Build a single field node from a property schema.
fn build_field_node(field_name: &str, full_path: &str, schema: &Value) -> Option<Value> {
    let (value_type, control) = infer_control_type(schema)?;

    let mut node = json!({
        "kind": "field",
        "id": format!("{}-{}", full_path.replace('.', "-"), field_name),
        "title": format!("settings.{}.{}.title", full_path.replace('.', "_"), field_name),
        "path": full_path,
        "valueType": value_type,
        "control": control,
    });

    // Add helpText from schema description
    if let Some(description) = schema.get("description").and_then(Value::as_str) {
        if !description.is_empty() {
            node.as_object_mut().and_then(|o| {
                o.insert(
                    "helpText".to_string(),
                    Value::String(description.to_string()),
                )
            });
        }
    }

    // Add min/max for number types
    if let Some(obj) = schema.as_object() {
        if let Some(min) = obj.get("minimum") {
            node.as_object_mut()
                .and_then(|o| o.insert("min".to_string(), min.clone()));
        }
        if let Some(max) = obj.get("maximum") {
            node.as_object_mut()
                .and_then(|o| o.insert("max".to_string(), max.clone()));
        }
    }

    Some(node)
}

/// Map a JSON Schema type to (valueType, control).
///
/// Returns `None` for types that cannot be represented as a simple field
/// (e.g., complex objects, arrays, maps).
fn infer_control_type(schema: &Value) -> Option<(&'static str, &'static str)> {
    let resolved = schema;

    // Handle nullable types (Option<T>): ["type", "null"] or type + null
    let types = match resolved.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(arr)) => arr.iter().filter_map(|v| v.as_str()).collect(),
        _ => return None,
    };

    // If the only type is "null", skip
    if types.len() == 1 && types[0] == "null" {
        return None;
    }

    // Check for enum (string enums)
    if resolved.get("enum").is_some() || resolved.get("oneOf").is_some() {
        // Check if it's a string enum
        if let Some(enum_vals) = resolved.get("enum").and_then(Value::as_array) {
            if enum_vals.iter().all(|v| v.is_string()) {
                return Some(("enum", "select"));
            }
        }
        if let Some(one_of) = resolved.get("oneOf").and_then(Value::as_array) {
            if one_of.iter().all(|v| {
                v.get("type").and_then(Value::as_str) == Some("string") && v.get("enum").is_some()
            }) {
                return Some(("enum", "select"));
            }
        }
        // Non-string enum — skip (can't render)
        return None;
    }

    // Map primitives
    for t in &types {
        match *t {
            "boolean" => return Some(("boolean", "switch")),
            "integer" => return Some(("integer", "number")),
            "number" => return Some(("float", "number")),
            "string" => {
                // Check format hints
                if let Some(format) = resolved.get("format").and_then(Value::as_str) {
                    match format {
                        "uint32" | "uint64" | "int32" | "int64" => {
                            return Some(("integer", "number"));
                        }
                        "float" | "double" => return Some(("float", "number")),
                        _ => {}
                    }
                }
                return Some(("string", "text"));
            }
            "array" => {
                // Simple string arrays → tags control
                return Some(("string_list", "tags"));
            }
            "object" => {
                if is_map_type(resolved) {
                    // Maps are skipped by caller
                    return None;
                }
                // Nested objects → key_value or json
                return Some(("json_map", "json"));
            }
            _ => {}
        }
    }

    None
}

/// Follow a `$ref` to its definition in the root schema.
fn follow_ref<'a>(schema: &'a Value, root_schema: &'a Value) -> &'a Value {
    if let Some(ref_str) = schema.get("$ref").and_then(Value::as_str) {
        if let Some(def_path) = ref_str.strip_prefix("#/$defs/") {
            if let Some(defs) = root_schema.get("$defs").and_then(|d| d.as_object()) {
                if let Some(def_schema) = defs.get(def_path) {
                    return follow_ref(def_schema, root_schema);
                }
            }
        }
    }
    schema
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_sub_agent_section() {
        let meta = SectionMeta {
            id: "sub_agent",
            title_key: "settings.sub_agent.title",
            icon: "Bot",
            order: 30,
            description_key: "settings.sub_agent.description",
        };

        let section = generate_simple_section::<super::super::SubAgentConfig>(&meta, "sub_agent")
            .expect("generate sub_agent section");

        // Verify section structure
        assert_eq!(section["id"], "sub_agent");
        assert_eq!(section["title"], "settings.sub_agent.title");
        assert_eq!(section["icon"], "Bot");

        // Verify children
        let children = section["children"].as_array().expect("children array");
        assert_eq!(children.len(), 1, "should have one group");

        let group = &children[0];
        let fields = group["children"].as_array().expect("group children");

        // Collect field paths
        let paths: Vec<&str> = fields
            .iter()
            .filter_map(|f| f.get("path").and_then(Value::as_str))
            .collect();

        assert!(
            paths.contains(&"sub_agent.max_concurrent"),
            "should have max_concurrent"
        );
        assert!(
            paths.contains(&"sub_agent.default_max_turns"),
            "should have default_max_turns"
        );
        assert!(
            paths.contains(&"sub_agent.hard_max_turns"),
            "should have hard_max_turns"
        );
        assert!(
            paths.contains(&"sub_agent.timeout_secs"),
            "should have timeout_secs"
        );
        assert!(
            paths.contains(&"sub_agent.worktree_enabled"),
            "should have worktree_enabled"
        );

        // Verify control types
        for field in fields {
            match field.get("path").and_then(Value::as_str) {
                Some("sub_agent.worktree_enabled") => {
                    assert_eq!(
                        field["control"], "switch",
                        "worktree_enabled should be switch"
                    );
                    assert_eq!(field["valueType"], "boolean");
                }
                Some(p) if p.starts_with("sub_agent.") && p != "sub_agent.worktree_enabled" => {
                    assert_eq!(field["control"], "number", "{p} should be number");
                    assert_eq!(field["valueType"], "integer");
                }
                _ => {}
            }
        }
    }

    #[test]
    fn generated_section_passes_path_validation() {
        let meta = SectionMeta {
            id: "sub_agent",
            title_key: "settings.sub_agent.title",
            icon: "Bot",
            order: 30,
            description_key: "settings.sub_agent.description",
        };

        let section = generate_simple_section::<super::super::SubAgentConfig>(&meta, "sub_agent")
            .expect("generate section");

        // Validate paths against AppConfig schema
        let result = super::super::path_validator::validate_settings_paths(&[section]);
        assert!(
            result.is_ok(),
            "generated section paths should be valid: {:?}",
            result
        );
    }

    #[test]
    fn infer_boolean_switch() {
        let schema = json!({"type": "boolean"});
        assert_eq!(infer_control_type(&schema), Some(("boolean", "switch")));
    }

    #[test]
    fn infer_integer_number() {
        let schema = json!({"type": "integer"});
        assert_eq!(infer_control_type(&schema), Some(("integer", "number")));
    }

    #[test]
    fn infer_string_text() {
        let schema = json!({"type": "string"});
        assert_eq!(infer_control_type(&schema), Some(("string", "text")));
    }

    #[test]
    fn skip_null_type() {
        let schema = json!({"type": "null"});
        assert_eq!(infer_control_type(&schema), None);
    }

    #[test]
    fn skip_map_type() {
        let schema = json!({
            "type": "object",
            "additionalProperties": { "$ref": "#/$defs/SomeType" }
        });
        assert!(is_map_type(&schema));
    }
}
