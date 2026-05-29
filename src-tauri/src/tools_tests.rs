#[cfg(test)]
mod tests {
    use crate::agentjax_home::AGENTJAX_HOME_ENV;
    use crate::config::{AppConfig, McpServerConfig};
    use crate::conversation_store;
    use crate::tools::{
        CalculatorTool, FileReaderTool, FileWriterTool, MountedToolDefinition,
        MountedToolSourceSession, MountedToolSourceSessions, SystemTimeTool, Tool, ToolCatalog,
        ToolExecutionContext, ToolRegistry, ToolSchemaFormat,
    };
    use serde_json::json;
    use std::sync::Arc;

    const DEFAULT_REGISTRY_TOOL_COUNT: usize = 7;
    const DEFAULT_CATALOG_TOOL_COUNT: usize = DEFAULT_REGISTRY_TOOL_COUNT + 4;

    struct TestHomeGuard {
        home: std::path::PathBuf,
    }

    fn assert_approx_eq(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "expected {expected}, got {actual} (tolerance {tolerance})"
        );
    }

    impl Drop for TestHomeGuard {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var(AGENTJAX_HOME_ENV);
            }
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }

    fn setup_test_home() -> TestHomeGuard {
        let home =
            std::env::temp_dir().join(format!("agentjax-tools-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("create test home");
        unsafe {
            std::env::set_var(AGENTJAX_HOME_ENV, &home);
        }
        TestHomeGuard { home }
    }

    #[test]
    fn test_calculator_success() {
        let calc = CalculatorTool;
        let ctx = ToolExecutionContext::default();

        // Basic arithmetic
        let args = json!({ "expression": "2 + 3 * 4" });
        let res = calc.execute(&args, &ctx).unwrap();
        assert_eq!(res["result"].as_f64().unwrap(), 14.0);
        assert_eq!(res["mode"], "evaluate");
        assert_eq!(res["exactValue"], "14");

        // Exponentiation
        let args = json!({ "expression": "2^3" });
        let res = calc.execute(&args, &ctx).unwrap();
        assert_eq!(res["result"].as_f64().unwrap(), 8.0);

        // Complex expressions with parentheses and sqrt
        let args = json!({ "expression": "2 * (3.5 + 4.5) / sqrt(16)" });
        let res = calc.execute(&args, &ctx).unwrap();
        assert_eq!(res["result"].as_f64().unwrap(), 4.0);

        // Negative numbers
        let args = json!({ "expression": "-3 + 5" });
        let res = calc.execute(&args, &ctx).unwrap();
        assert_eq!(res["result"].as_f64().unwrap(), 2.0);

        // Built-in constants and trigonometric functions
        let args = json!({ "expression": "sin(pi / 2) + cos(0)" });
        let res = calc.execute(&args, &ctx).unwrap();
        assert_approx_eq(res["result"].as_f64().unwrap(), 2.0, 1e-12);

        // Natural trigonometric syntax is now parsed directly by fend-core.
        let args = json!({ "expression": "sin pi/2" });
        let res = calc.execute(&args, &ctx).unwrap();
        assert_eq!(res["mode"], "evaluate");
        assert_approx_eq(res["result"].as_f64().unwrap(), 0.0, 1e-12);
        assert_eq!(res["exactValue"], "0");

        // Unit-aware arithmetic
        let args = json!({ "expression": "3 km + 500 m" });
        let res = calc.execute(&args, &ctx).unwrap();
        assert_eq!(res["mode"], "evaluate");
        assert_eq!(res["result"], "3.5 km");
        assert!(res["exactValue"].as_str().unwrap().contains("km"));
        if let Some(unit) = res["unit"].as_str() {
            assert!(unit.contains("km"));
        }

        let args = json!({ "expression": "3km + 500m" });
        let res = calc.execute(&args, &ctx).unwrap();
        assert_eq!(res["result"], "3.5 km");

        // Variable bindings should compile into native fend assignments.
        let args = json!({ "expression": "x + y", "variables": { "x": 2, "y": 5 } });
        let res = calc.execute(&args, &ctx).unwrap();
        assert_eq!(res["result"].as_f64().unwrap(), 7.0);
        assert!(res["warnings"].as_array().unwrap().iter().any(|warning| {
            warning
                .as_str()
                .unwrap_or_default()
                .contains("native fend variable assignments")
        }));

        // Native assignment semantics should preserve precedence.
        let args = json!({ "expression": "x^2", "variables": { "x": -2 } });
        let res = calc.execute(&args, &ctx).unwrap();
        assert_eq!(res["result"].as_f64().unwrap(), 4.0);

        // Variable bindings can also carry richer fend expressions with units.
        let args = json!({ "expression": "speed * duration + offset", "variables": {
            "speed": "60 km/h",
            "duration": "2 h",
            "offset": "4 km"
        }});
        let res = calc.execute(&args, &ctx).unwrap();
        assert_eq!(res["result"], "124 km");

        // Complex outputs should not leak into the unit field.
        let args = json!({ "expression": "exp(i*pi)" });
        let res = calc.execute(&args, &ctx).unwrap();
        if let Some(value) = res["result"].as_f64() {
            assert_approx_eq(value, -1.0, 1e-12);
        } else {
            assert!(res["result"].as_str().unwrap().starts_with("-1"));
        }
        assert!(res["unit"].is_null());

        // Capability discovery
        let args = json!({ "mode": "capabilities" });
        let res = calc.execute(&args, &ctx).unwrap();
        assert_eq!(res["mode"], "capabilities");
        assert!(
            !res["capabilities"]["supports"]["symbolicMath"]
                .as_bool()
                .unwrap()
        );
        assert!(res["capabilities"]["supports"]["units"].as_bool().unwrap());
        assert_eq!(
            res["capabilities"]["engine"]["name"].as_str().unwrap(),
            "fend-core"
        );
        assert!(
            res["capabilities"]["engine"]["policy"]
                .as_str()
                .unwrap()
                .contains("fend-core")
        );
    }

    #[test]
    fn test_calculator_legacy_modes_are_rejected() {
        let calc = CalculatorTool;
        let ctx = ToolExecutionContext::default();

        let args = json!({ "mode": "simplify", "expression": "2x + 3x" });
        let err = calc.execute(&args, &ctx).unwrap_err();
        assert!(err.contains("Unsupported calculator mode"));
    }

    #[test]
    fn test_calculator_errors() {
        let calc = CalculatorTool;
        let ctx = ToolExecutionContext::default();

        // Division by zero
        let args = json!({ "expression": "5 / 0" });
        let err = calc.execute(&args, &ctx).unwrap_err();
        assert!(err.contains("could not evaluate") || err.contains("failed"));

        // Sqrt of negative
        let args = json!({ "expression": "sqrt(-4)" });
        let res = calc.execute(&args, &ctx).unwrap();
        assert_eq!(res["result"], "2i");
        assert!(
            res["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|warning| warning.as_str().unwrap_or_default().contains("approximate"))
        );

        // fend-core should own parse behavior for malformed grouping.
        let args = json!({ "expression": "(2 + 3" });
        let res = calc.execute(&args, &ctx).unwrap();
        assert_eq!(res["result"].as_f64().unwrap(), 5.0);

        // Legacy symbolic calls are explicitly rejected.
        let args = json!({ "expression": "factor(x^2 - 1)" });
        let err = calc.execute(&args, &ctx).unwrap_err();
        assert!(err.contains("no longer supported"));
        assert!(err.contains("legacy symbolic engine was removed"));

        // Missing expression should surface a schema-level error.
        let args = json!({});
        let err = calc.execute(&args, &ctx).unwrap_err();
        assert!(err.contains("Missing calculator input"));
    }

    #[test]
    fn test_system_time() {
        let time_tool = SystemTimeTool;
        let args = json!({});
        let res = time_tool
            .execute(&args, &ToolExecutionContext::default())
            .unwrap();

        assert!(res.get("localTime").is_some());
        assert!(res["unixTimestampMs"].as_i64().unwrap() > 0);
    }

    #[test]
    fn test_file_tools_require_conversation_context() {
        let reader = FileReaderTool;
        let writer = FileWriterTool;
        let ctx = ToolExecutionContext::default();

        let write_err = writer
            .execute(&json!({"filename": "x.txt", "content": "x"}), &ctx)
            .unwrap_err();
        assert!(write_err.contains("Missing conversation context"));

        let read_err = reader
            .execute(&json!({"filename": "x.txt"}), &ctx)
            .unwrap_err();
        assert!(read_err.contains("Missing conversation context"));
    }

    #[test]
    fn test_file_tools_workspace_isolated_by_conversation() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let reader = FileReaderTool;
        let writer = FileWriterTool;
        let conversation_a = format!("test-workspace-a-{}", uuid::Uuid::new_v4());
        let conversation_b = format!("test-workspace-b-{}", uuid::Uuid::new_v4());

        conversation_store::ensure_conversation(&conversation_a).unwrap();
        conversation_store::ensure_conversation(&conversation_b).unwrap();

        let ctx_a = ToolExecutionContext {
            conversation_id: Some(conversation_a.clone()),
        };
        let ctx_b = ToolExecutionContext {
            conversation_id: Some(conversation_b.clone()),
        };

        let filename = "nested/same_name.txt";
        writer
            .execute(&json!({"path": filename, "content": "from-a"}), &ctx_a)
            .unwrap();
        writer
            .execute(&json!({"path": filename, "content": "from-b"}), &ctx_b)
            .unwrap();

        let read_a = reader.execute(&json!({"path": filename}), &ctx_a).unwrap();
        let read_b = reader.execute(&json!({"path": filename}), &ctx_b).unwrap();
        assert_eq!(read_a["content"], "from-a");
        assert_eq!(read_b["content"], "from-b");

        conversation_store::delete_conversation(&conversation_a).unwrap();
        conversation_store::delete_conversation(&conversation_b).unwrap();
    }

    #[test]
    fn test_file_tools_support_nested_paths_and_reject_workspace_escape() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let registry = ToolRegistry::new_with_defaults();
        let conversation_id = format!("test-file-paths-{}", uuid::Uuid::new_v4());
        conversation_store::ensure_conversation(&conversation_id).unwrap();
        let ctx = ToolExecutionContext {
            conversation_id: Some(conversation_id.clone()),
        };

        registry
            .execute(
                "write_file",
                &json!({"path": "src/components/Sidebar.tsx", "content": "export const sidebar = true;\n"}),
                &ctx,
            )
            .unwrap();

        let read_res = registry
            .execute(
                "read_file",
                &json!({"path": "src/components/Sidebar.tsx"}),
                &ctx,
            )
            .unwrap();
        assert_eq!(read_res["content"], "export const sidebar = true;\n");

        let err = registry
            .execute("read_file", &json!({"path": "../outside.txt"}), &ctx)
            .unwrap_err();
        assert!(err.contains("escapes the conversation workspace"));

        conversation_store::delete_conversation(&conversation_id).unwrap();
    }

    #[test]
    fn test_directory_tools_list_stat_and_mkdir() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let registry = ToolRegistry::new_with_defaults();
        let conversation_id = format!("test-directory-tools-{}", uuid::Uuid::new_v4());
        conversation_store::ensure_conversation(&conversation_id).unwrap();
        let ctx = ToolExecutionContext {
            conversation_id: Some(conversation_id.clone()),
        };

        registry
            .execute("mkdir", &json!({"path": "src/components"}), &ctx)
            .unwrap();
        registry
            .execute(
                "write_file",
                &json!({"path": "src/components/Button.tsx", "content": "export const Button = () => null;\n"}),
                &ctx,
            )
            .unwrap();
        registry
            .execute(
                "write_file",
                &json!({"path": "README.md", "content": "# Demo\n"}),
                &ctx,
            )
            .unwrap();

        let root_listing = registry.execute("list_files", &json!({}), &ctx).unwrap();
        let root_entries = root_listing["entries"].as_array().unwrap();
        assert!(
            root_entries
                .iter()
                .any(|entry| entry["path"] == "README.md")
        );
        assert!(root_entries.iter().any(|entry| entry["path"] == "src"));

        let recursive_listing = registry
            .execute(
                "list_files",
                &json!({"path": "src", "recursive": true}),
                &ctx,
            )
            .unwrap();
        let recursive_entries = recursive_listing["entries"].as_array().unwrap();
        assert!(
            recursive_entries
                .iter()
                .any(|entry| entry["path"] == "src/components/Button.tsx")
        );

        conversation_store::delete_conversation(&conversation_id).unwrap();
    }

    #[test]
    fn test_read_file_truncates_large_text_preview() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let registry = ToolRegistry::new_with_defaults();
        let conversation_id = format!("test-read-truncation-{}", uuid::Uuid::new_v4());
        conversation_store::ensure_conversation(&conversation_id).unwrap();
        let ctx = ToolExecutionContext {
            conversation_id: Some(conversation_id.clone()),
        };

        let content = "0123456789abcdef".repeat(4_096);
        registry
            .execute(
                "write_file",
                &json!({"path": "logs/large.txt", "content": content}),
                &ctx,
            )
            .unwrap();

        let preview = registry
            .execute(
                "read_file",
                &json!({"path": "logs/large.txt", "max_bytes": 1024}),
                &ctx,
            )
            .unwrap();

        assert_eq!(preview["truncated"], true);
        assert_eq!(preview["maxBytes"], 1024);
        assert_eq!(preview["bytesRead"], 1024);
        assert!(preview["totalBytes"].as_u64().unwrap() > 1024);
        let preview_content = preview["content"].as_str().unwrap();
        assert_eq!(preview_content.len(), 1024);
        assert_eq!(preview_content, &"0123456789abcdef".repeat(64));

        conversation_store::delete_conversation(&conversation_id).unwrap();
    }

    #[test]
    fn test_content_sniffing_detects_extensionless_text_files() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let registry = ToolRegistry::new_with_defaults();
        let conversation_id = format!("test-content-sniffing-text-{}", uuid::Uuid::new_v4());
        conversation_store::ensure_conversation(&conversation_id).unwrap();
        let ctx = ToolExecutionContext {
            conversation_id: Some(conversation_id.clone()),
        };

        registry
            .execute(
                "write_file",
                &json!({"path": "notes/README", "content": "hello from a file without an extension\n"}),
                &ctx,
            )
            .unwrap();

        let read = registry
            .execute("read_file", &json!({"path": "notes/README"}), &ctx)
            .unwrap();
        assert_eq!(read["contentKind"], "text");
        assert_eq!(read["mediaType"], "text/plain");

        conversation_store::delete_conversation(&conversation_id).unwrap();
    }

    #[test]
    fn test_list_files_truncates_by_output_size() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let registry = ToolRegistry::new_with_defaults();
        let conversation_id = format!("test-list-truncation-{}", uuid::Uuid::new_v4());
        conversation_store::ensure_conversation(&conversation_id).unwrap();
        let ctx = ToolExecutionContext {
            conversation_id: Some(conversation_id.clone()),
        };

        for index in 0..400 {
            registry
                .execute(
                    "write_file",
                    &json!({
                        "path": format!("many/nested_directory_with_a_very_long_name_{index:03}/file_name_that_is_also_quite_long_{index:03}.txt"),
                        "content": format!("file-{index}\n"),
                    }),
                    &ctx,
                )
                .unwrap();
        }

        let listing = registry
            .execute(
                "list_files",
                &json!({"path": "many", "recursive": true, "max_entries": 1000}),
                &ctx,
            )
            .unwrap();

        assert_eq!(listing["truncated"], true);
        assert!(listing["entryCount"].as_u64().unwrap() < 1000);
        let reasons = listing["truncationReasons"].as_array().unwrap();
        assert!(reasons.iter().any(|reason| reason == "max_output_chars"));
        assert!(listing["approxOutputChars"].as_u64().unwrap() > 0);

        conversation_store::delete_conversation(&conversation_id).unwrap();
    }

    #[test]
    fn test_content_sniffing_rejects_binary_files_disguised_as_text() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let registry = ToolRegistry::new_with_defaults();
        let conversation_id = format!("test-content-sniffing-binary-{}", uuid::Uuid::new_v4());
        conversation_store::ensure_conversation(&conversation_id).unwrap();
        let ctx = ToolExecutionContext {
            conversation_id: Some(conversation_id.clone()),
        };

        let workspace = conversation_store::conversation_workspace_path(&conversation_id).unwrap();
        let disguised_binary = workspace.join("assets/fake-notes.txt");
        std::fs::create_dir_all(disguised_binary.parent().unwrap()).unwrap();
        std::fs::write(
            &disguised_binary,
            [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
        )
        .unwrap();

        let read_err = registry
            .execute("read_file", &json!({"path": "assets/fake-notes.txt"}), &ctx)
            .unwrap_err();
        assert!(read_err.contains("Portable Network Graphics"));

        conversation_store::delete_conversation(&conversation_id).unwrap();
    }

    #[test]
    fn test_binary_files_are_rejected_by_text_tools() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let registry = ToolRegistry::new_with_defaults();
        let conversation_id = format!("test-binary-guards-{}", uuid::Uuid::new_v4());
        conversation_store::ensure_conversation(&conversation_id).unwrap();
        let ctx = ToolExecutionContext {
            conversation_id: Some(conversation_id.clone()),
        };

        let workspace = conversation_store::conversation_workspace_path(&conversation_id).unwrap();
        let binary_path = workspace.join("assets/image.bin");
        std::fs::create_dir_all(binary_path.parent().unwrap()).unwrap();
        std::fs::write(&binary_path, [0_u8, 159, 146, 150, 0]).unwrap();

        let read_err = registry
            .execute("read_file", &json!({"path": "assets/image.bin"}), &ctx)
            .unwrap_err();
        assert!(read_err.contains("non-text/binary"));

        let replace_err = registry
            .execute(
                "edit_file",
                &json!({
                    "path": "assets/image.bin",
                    "edits": [
                        {
                            "op": "replace",
                            "find": "a",
                            "replace": "b"
                        }
                    ]
                }),
                &ctx,
            )
            .unwrap_err();
        assert!(replace_err.contains("Refusing to edit"));

        let write_err = registry
            .execute(
                "write_file",
                &json!({"path": "assets/image.bin", "content": "hello"}),
                &ctx,
            )
            .unwrap_err();
        assert!(write_err.contains("Refusing to write"));

        conversation_store::delete_conversation(&conversation_id).unwrap();
    }

    #[test]
    fn test_text_edit_tools_and_structured_patch_are_deterministic() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let registry = ToolRegistry::new_with_defaults();
        let conversation_id = format!("test-text-edits-{}", uuid::Uuid::new_v4());
        conversation_store::ensure_conversation(&conversation_id).unwrap();
        let ctx = ToolExecutionContext {
            conversation_id: Some(conversation_id.clone()),
        };

        registry
            .execute(
                "write_file",
                &json!({
                    "path": "src/main.rs",
                    "content": "fn main() {\n    println!(\"hello\");\n}\n"
                }),
                &ctx,
            )
            .unwrap();

        registry
            .execute(
                "edit_file",
                &json!({
                    "path": "src/main.rs",
                    "edits": [
                        {
                            "op": "replace",
                            "find": "hello",
                            "replace": "hi"
                        }
                    ]
                }),
                &ctx,
            )
            .unwrap();

        registry
            .execute(
                "edit_file",
                &json!({
                    "path": "src/main.rs",
                    "edits": [
                        {
                            "op": "insert_after",
                            "anchor": "fn main() {",
                            "content": "\n    let value = 1;"
                        }
                    ]
                }),
                &ctx,
            )
            .unwrap();

        registry
            .execute(
                "edit_file",
                &json!({
                    "path": "src/main.rs",
                    "edits": [
                        {
                            "op": "insert_before",
                            "anchor": "    println!(\"hi\");",
                            "content": "    // greeting\n"
                        }
                    ]
                }),
                &ctx,
            )
            .unwrap();

        registry
            .execute(
                "edit_file",
                &json!({
                    "path": "src/main.rs",
                    "edits": [
                        {
                            "op": "replace",
                            "find": "    let value = 1;\n    // greeting\n    println!(\"hi\");",
                            "replace": "    let value = 2;\n    // greeting\n    println!(\"hi\");"
                        }
                    ]
                }),
                &ctx,
            )
            .unwrap();

        registry
            .execute(
                "edit_file",
                &json!({
                    "path": "src/main.rs",
                    "edits": [
                        {
                            "op": "replace",
                            "find": "value = 2",
                            "replace": "value = 3"
                        },
                        {
                            "op": "insert_before",
                            "anchor": "}\n",
                            "content": "    println!(\"done\");\n"
                        }
                    ]
                }),
                &ctx,
            )
            .unwrap();

        let final_file = registry
            .execute("read_file", &json!({"path": "src/main.rs"}), &ctx)
            .unwrap();
        assert_eq!(
            final_file["content"],
            "fn main() {\n    let value = 3;\n    // greeting\n    println!(\"hi\");\n    println!(\"done\");\n}\n"
        );

        conversation_store::delete_conversation(&conversation_id).unwrap();
    }

    #[test]
    fn test_apply_patch_is_atomic_when_a_later_edit_fails() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let registry = ToolRegistry::new_with_defaults();
        let conversation_id = format!("test-apply-patch-atomic-{}", uuid::Uuid::new_v4());
        conversation_store::ensure_conversation(&conversation_id).unwrap();
        let ctx = ToolExecutionContext {
            conversation_id: Some(conversation_id.clone()),
        };

        registry
            .execute(
                "write_file",
                &json!({
                    "path": "notes/example.txt",
                    "content": "alpha\nbeta\ngamma\n"
                }),
                &ctx,
            )
            .unwrap();

        let err = registry
            .execute(
                "edit_file",
                &json!({
                    "path": "notes/example.txt",
                    "edits": [
                        {
                            "op": "replace",
                            "find": "alpha",
                            "replace": "ALPHA"
                        },
                        {
                            "op": "insert_after",
                            "anchor": "missing\n",
                            "content": "delta\n"
                        }
                    ]
                }),
                &ctx,
            )
            .unwrap_err();
        assert!(err.contains("Patch edit 2 failed"));

        let final_file = registry
            .execute("read_file", &json!({"path": "notes/example.txt"}), &ctx)
            .unwrap();
        assert_eq!(final_file["content"], "alpha\nbeta\ngamma\n");

        conversation_store::delete_conversation(&conversation_id).unwrap();
    }

    #[test]
    fn test_tool_registry() {
        let registry = ToolRegistry::new_with_defaults();
        let schemas = registry.list_schemas();
        assert_eq!(schemas.len(), DEFAULT_REGISTRY_TOOL_COUNT);

        // Execute via registry
        let args = json!({ "expression": "100 * 2.5" });
        let res = registry
            .execute("calculator", &args, &ToolExecutionContext::default())
            .unwrap();
        assert_eq!(res["result"].as_f64().unwrap(), 250.0);
    }

    #[test]
    fn test_tool_schema_formats() {
        let registry = ToolRegistry::new_with_defaults();

        let responses_schemas = registry.list_schemas_with_format(ToolSchemaFormat::Responses);
        let cc_schemas = registry.list_schemas_with_format(ToolSchemaFormat::ChatCompletions);
        let gemini_schemas = registry.list_schemas_with_format(ToolSchemaFormat::Gemini);
        let anthropic_schemas = registry.list_schemas_with_format(ToolSchemaFormat::Anthropic);

        assert_eq!(responses_schemas.len(), DEFAULT_REGISTRY_TOOL_COUNT);
        assert_eq!(cc_schemas.len(), DEFAULT_REGISTRY_TOOL_COUNT);
        assert_eq!(gemini_schemas.len(), DEFAULT_REGISTRY_TOOL_COUNT);
        assert_eq!(anthropic_schemas.len(), DEFAULT_REGISTRY_TOOL_COUNT);

        let first_responses = &responses_schemas[0];
        assert_eq!(first_responses["type"], "function");
        assert!(first_responses.get("name").is_some());
        assert!(first_responses.get("function").is_none());

        let first_cc = &cc_schemas[0];
        assert_eq!(first_cc["type"], "function");
        assert!(first_cc.get("name").is_none());
        assert!(first_cc.get("function").is_some());
        assert!(first_cc["function"].get("name").is_some());
        assert!(first_cc["function"].get("parameters").is_some());

        let first_gemini = &gemini_schemas[0];
        assert!(first_gemini.get("type").is_none());
        assert!(first_gemini.get("name").is_some());
        assert!(first_gemini.get("parameters").is_some());

        let first_anthropic = &anthropic_schemas[0];
        assert!(first_anthropic.get("type").is_none());
        assert!(first_anthropic.get("name").is_some());
        assert!(first_anthropic.get("input_schema").is_some());

        for schema in &responses_schemas {
            let parameters = schema
                .get("parameters")
                .and_then(|value| value.as_object())
                .expect("responses schema parameters should be an object");
            assert_eq!(
                parameters.get("type").and_then(|value| value.as_str()),
                Some("object")
            );
            assert!(!parameters.contains_key("anyOf"));
            assert!(!parameters.contains_key("oneOf"));
            assert!(!parameters.contains_key("allOf"));
            assert!(!parameters.contains_key("not"));
        }

        for schema in &cc_schemas {
            let parameters = schema
                .get("function")
                .and_then(|value| value.get("parameters"))
                .and_then(|value| value.as_object())
                .expect("chat completions schema parameters should be an object");
            assert_eq!(
                parameters.get("type").and_then(|value| value.as_str()),
                Some("object")
            );
            assert!(!parameters.contains_key("anyOf"));
            assert!(!parameters.contains_key("oneOf"));
            assert!(!parameters.contains_key("allOf"));
            assert!(!parameters.contains_key("not"));
        }

        for schema in &gemini_schemas {
            let parameters = schema
                .get("parameters")
                .and_then(|value| value.as_object())
                .expect("gemini schema parameters should be an object");
            assert_eq!(
                parameters.get("type").and_then(|value| value.as_str()),
                Some("object")
            );
            assert!(!parameters.contains_key("anyOf"));
            assert!(!parameters.contains_key("oneOf"));
            assert!(!parameters.contains_key("allOf"));
            assert!(!parameters.contains_key("not"));
        }

        for schema in &anthropic_schemas {
            let parameters = schema
                .get("input_schema")
                .and_then(|value| value.as_object())
                .expect("anthropic schema input_schema should be an object");
            assert_eq!(
                parameters.get("type").and_then(|value| value.as_str()),
                Some("object")
            );
            assert!(!parameters.contains_key("anyOf"));
            assert!(!parameters.contains_key("oneOf"));
            assert!(!parameters.contains_key("allOf"));
            assert!(!parameters.contains_key("not"));
        }
    }

    #[tokio::test]
    async fn test_tool_catalog_snapshot_freezes_native_tool_view() {
        let config = crate::config::AppConfig::default();
        let catalog = ToolCatalog::new(Arc::new(crate::mcp::McpManager::new()), &config);
        let snapshot = catalog.snapshot(&ToolExecutionContext::default()).await;

        assert_eq!(snapshot.schemas().len(), DEFAULT_CATALOG_TOOL_COUNT);
        assert!(snapshot.active_tool_names().contains("calculator"));
        assert!(snapshot.active_tool_names().contains("get_system_time"));
        assert!(snapshot.active_tool_names().contains("list_files"));
        assert!(
            snapshot
                .active_tool_names()
                .contains("start_background_tool")
        );
        assert!(
            snapshot
                .active_tool_names()
                .contains("wait_background_tool")
        );
        assert!(
            snapshot
                .active_tool_names()
                .contains("cancel_background_tool")
        );
        assert!(
            snapshot
                .active_tool_names()
                .contains("list_background_tools")
        );

        let result = snapshot
            .execute(
                "calculator",
                &json!({ "expression": "6 * 7" }),
                &ToolExecutionContext::default(),
            )
            .await
            .expect("execute calculator from snapshot");
        assert_eq!(result["result"].as_f64(), Some(42.0));
    }

    #[tokio::test]
    async fn test_background_tool_sidecar_start_wait_and_list() {
        let config = crate::config::AppConfig::default();
        let catalog = ToolCatalog::new(Arc::new(crate::mcp::McpManager::new()), &config);
        let snapshot = catalog.snapshot(&ToolExecutionContext::default()).await;
        let ctx = ToolExecutionContext::default();

        let started = snapshot
            .execute(
                "start_background_tool",
                &json!({
                    "toolName": "calculator",
                    "arguments": { "expression": "21 * 2" }
                }),
                &ctx,
            )
            .await
            .expect("start background calculator");
        assert_eq!(started["ok"], true);
        assert_eq!(started["status"], "in_progress");
        let job_id = started["jobId"].as_str().expect("job id");

        let waited = snapshot
            .execute(
                "wait_background_tool",
                &json!({ "jobId": job_id, "timeoutMs": 10_000 }),
                &ctx,
            )
            .await
            .expect("wait for background calculator");
        assert_eq!(waited["timedOut"], false);
        assert_eq!(waited["job"]["status"], "completed");
        assert_eq!(waited["job"]["output"]["result"].as_f64(), Some(42.0));

        let jobs = snapshot
            .execute("list_background_tools", &json!({}), &ctx)
            .await
            .expect("list background jobs");
        assert!(jobs["jobs"].as_array().unwrap().iter().any(|job| {
            job.get("jobId").and_then(|value| value.as_str()) == Some(job_id)
                && job.get("status").and_then(|value| value.as_str()) == Some("completed")
        }));
    }

    #[tokio::test]
    async fn test_background_tool_sidecar_is_conversation_scoped() {
        let config = crate::config::AppConfig::default();
        let catalog = ToolCatalog::new(Arc::new(crate::mcp::McpManager::new()), &config);
        let ctx_a = ToolExecutionContext {
            conversation_id: Some(format!("test-bg-a-{}", uuid::Uuid::new_v4())),
        };
        let ctx_b = ToolExecutionContext {
            conversation_id: Some(format!("test-bg-b-{}", uuid::Uuid::new_v4())),
        };
        let snapshot = catalog.snapshot(&ctx_a).await;

        let started = snapshot
            .execute(
                "start_background_tool",
                &json!({
                    "toolName": "calculator",
                    "arguments": { "expression": "5 * 9" }
                }),
                &ctx_a,
            )
            .await
            .expect("start scoped background calculator");
        let job_id = started["jobId"].as_str().expect("job id");
        assert_eq!(
            started["job"]["conversationId"].as_str(),
            ctx_a.conversation_id.as_deref()
        );

        let other_conversation_jobs = snapshot
            .execute("list_background_tools", &json!({}), &ctx_b)
            .await
            .expect("list other conversation jobs");
        assert!(
            other_conversation_jobs["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|job| job.get("jobId").and_then(|value| value.as_str()) != Some(job_id))
        );

        let wrong_conversation_wait = snapshot
            .execute(
                "wait_background_tool",
                &json!({ "jobId": job_id, "timeoutMs": 100 }),
                &ctx_b,
            )
            .await;
        assert!(wrong_conversation_wait.is_err());

        let waited = snapshot
            .execute(
                "wait_background_tool",
                &json!({ "jobId": job_id, "timeoutMs": 10_000 }),
                &ctx_a,
            )
            .await
            .expect("wait for scoped background calculator");
        assert_eq!(waited["job"]["status"], "completed");
        assert_eq!(waited["job"]["output"]["result"].as_f64(), Some(45.0));
    }

    #[tokio::test]
    async fn test_background_tool_sidecar_cancel() {
        let config = crate::config::AppConfig::default();
        let catalog = ToolCatalog::new(Arc::new(crate::mcp::McpManager::new()), &config);
        let snapshot = catalog.snapshot(&ToolExecutionContext::default()).await;
        let ctx = ToolExecutionContext::default();

        let job = crate::tools::background_jobs::start_job_for_conversation("test_sleep", None);
        let job_id = crate::tools::background_jobs::job_id(&job);
        let job_for_task = job.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            crate::tools::background_jobs::complete_job(
                &job_for_task,
                Ok(json!({ "unexpected": true })),
            );
        });
        crate::tools::background_jobs::register_job_handle(&job, handle);

        let cancelled = snapshot
            .execute("cancel_background_tool", &json!({ "jobId": job_id }), &ctx)
            .await
            .expect("cancel background job");
        assert_eq!(cancelled["ok"], true);
        assert_eq!(cancelled["cancelled"], true);
        assert_eq!(cancelled["job"]["status"], "cancelled");

        let waited = snapshot
            .execute(
                "wait_background_tool",
                &json!({ "jobId": job_id, "timeoutMs": 1_000 }),
                &ctx,
            )
            .await
            .expect("wait for cancelled background job");
        assert_eq!(waited["timedOut"], false);
        assert_eq!(waited["job"]["status"], "cancelled");
        assert_eq!(waited["ok"], false);
    }

    #[tokio::test]
    async fn test_background_jobs_cancel_by_conversation() {
        let conversation_id = format!("test-bg-cancel-{}", uuid::Uuid::new_v4());
        let other_conversation_id = format!("test-bg-keep-{}", uuid::Uuid::new_v4());
        let job = crate::tools::background_jobs::start_job_for_conversation(
            "test_sleep",
            Some(conversation_id.clone()),
        );
        let other_job = crate::tools::background_jobs::start_job_for_conversation(
            "test_sleep",
            Some(other_conversation_id.clone()),
        );
        for candidate in [&job, &other_job] {
            let job_for_task = candidate.clone();
            let handle = tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                crate::tools::background_jobs::complete_job(
                    &job_for_task,
                    Ok(json!({ "unexpected": true })),
                );
            });
            crate::tools::background_jobs::register_job_handle(candidate, handle);
        }

        assert_eq!(
            crate::tools::background_jobs::cancel_conversation_jobs(&conversation_id),
            1
        );

        let cancelled = crate::tools::background_jobs::wait_for_job(
            &crate::tools::background_jobs::job_id(&job),
            Some(1_000),
            Some(&conversation_id),
        )
        .await
        .expect("wait cancelled conversation job");
        assert_eq!(cancelled["job"]["status"], "cancelled");

        let still_running = crate::tools::background_jobs::wait_for_job(
            &crate::tools::background_jobs::job_id(&other_job),
            Some(10),
            Some(&other_conversation_id),
        )
        .await
        .expect("wait other conversation job");
        assert_eq!(still_running["timedOut"], true);
        assert_eq!(still_running["job"]["status"], "in_progress");

        crate::tools::background_jobs::cancel_conversation_jobs(&other_conversation_id);
    }

    #[tokio::test]
    async fn test_tool_catalog_includes_conversation_dynamic_tool_alias() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let conversation_id = format!("test-dynamic-tools-{}", uuid::Uuid::new_v4());
        conversation_store::ensure_conversation(&conversation_id).expect("ensure conversation");
        conversation_store::update_conversation_dynamic_tools(
            &conversation_id,
            vec![conversation_store::ConversationDynamicTool {
                name: "math_alias".to_string(),
                display_name: None,
                description: "Alias to the native calculator tool".to_string(),
                icon: None,
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "expression": { "type": "string" }
                    },
                    "required": ["expression"]
                }),
                binding: conversation_store::ConversationDynamicToolBinding::Native {
                    tool: "calculator".to_string(),
                },
            }],
        )
        .expect("persist dynamic tools");

        let config = crate::config::AppConfig::default();
        let catalog = ToolCatalog::new(Arc::new(crate::mcp::McpManager::new()), &config);
        let snapshot = catalog
            .snapshot(&ToolExecutionContext {
                conversation_id: Some(conversation_id.clone()),
            })
            .await;

        assert!(snapshot.active_tool_names().contains("math_alias"));
        assert!(snapshot.schemas().iter().any(|schema| {
            schema.get("name").and_then(|value| value.as_str()) == Some("math_alias")
        }));
        assert_eq!(
            snapshot
                .presentation_for("math_alias")
                .and_then(|presentation| presentation.icon.as_deref()),
            Some("Calculator")
        );

        let result = snapshot
            .execute(
                "math_alias",
                &json!({ "expression": "9 + 10" }),
                &ToolExecutionContext {
                    conversation_id: Some(conversation_id.clone()),
                },
            )
            .await
            .expect("execute aliased tool");
        assert_eq!(result["result"].as_f64(), Some(19.0));

        conversation_store::delete_conversation(&conversation_id).ok();
    }

    #[tokio::test]
    async fn test_dynamic_mcp_tool_alias_defaults_to_layout_grid_icon() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let conversation_id = format!("test-dynamic-mcp-tools-{}", uuid::Uuid::new_v4());
        conversation_store::ensure_conversation(&conversation_id).expect("ensure conversation");
        conversation_store::update_conversation_dynamic_tools(
            &conversation_id,
            vec![conversation_store::ConversationDynamicTool {
                name: "docs_search".to_string(),
                display_name: Some("Docs Search".to_string()),
                description: "Alias to an MCP docs search tool".to_string(),
                icon: None,
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    },
                    "required": ["query"]
                }),
                binding: conversation_store::ConversationDynamicToolBinding::Mcp {
                    server_id: "openai_docs".to_string(),
                    tool: "search_openai_docs".to_string(),
                },
            }],
        )
        .expect("persist dynamic tools");

        let mut config = AppConfig::default();
        config
            .mcp_servers
            .insert("openai_docs".to_string(), McpServerConfig::default());
        let catalog = ToolCatalog::new(Arc::new(crate::mcp::McpManager::new()), &config);
        let snapshot = catalog
            .snapshot(&ToolExecutionContext {
                conversation_id: Some(conversation_id.clone()),
            })
            .await;

        assert_eq!(
            snapshot
                .presentation_for("docs_search")
                .and_then(|presentation| presentation.icon.as_deref()),
            Some("LayoutGrid")
        );

        conversation_store::delete_conversation(&conversation_id).ok();
    }

    #[tokio::test]
    async fn test_unmounted_mcp_server_exposes_only_mount_tool() {
        let mut config = AppConfig::default();
        config
            .mcp_servers
            .insert("openai_docs".to_string(), McpServerConfig::default());

        let catalog = ToolCatalog::new(Arc::new(crate::mcp::McpManager::new()), &config);
        let snapshot = catalog.snapshot(&ToolExecutionContext::default()).await;

        assert!(snapshot.schemas().iter().any(|schema| {
            schema.get("name").and_then(|value| value.as_str()) == Some("mcp_server__openai_docs")
        }));
        let control_schema = snapshot
            .schemas()
            .iter()
            .find(|schema| {
                schema.get("name").and_then(|value| value.as_str())
                    == Some("mcp_server__openai_docs")
            })
            .expect("control schema should exist");
        assert_eq!(
            control_schema["parameters"]["properties"]["action"]["enum"],
            json!(["mount", "unmount", "status"])
        );
        assert!(!snapshot.schemas().iter().any(|schema| {
            schema
                .get("name")
                .and_then(|value| value.as_str())
                .is_some_and(|name| name.starts_with("mcp__openai_docs__"))
        }));
    }

    #[tokio::test]
    async fn test_mounted_mcp_server_exposes_server_tools_and_control_tool() {
        let mut config = AppConfig::default();
        config
            .mcp_servers
            .insert("openai_docs".to_string(), McpServerConfig::default());

        let catalog = ToolCatalog::new(Arc::new(crate::mcp::McpManager::new()), &config);
        let mut mounted_servers = MountedToolSourceSessions::new();
        mounted_servers.insert(
            "openai_docs".to_string(),
            MountedToolSourceSession {
                source_id: "openai_docs".to_string(),
                source_type: "mcp".to_string(),
                tools: vec![MountedToolDefinition {
                    tool_name: "search_openai_docs".to_string(),
                    display_name: "Search Openai Docs".to_string(),
                    description: "Search docs".to_string(),
                    icon: Some("LayoutGrid".to_string()),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "query": { "type": "string" }
                        }
                    }),
                }],
                mcp_config: Some(McpServerConfig::default()),
            },
        );

        let snapshot = catalog
            .snapshot_with_format_and_mounted_servers(
                ToolSchemaFormat::Responses,
                &ToolExecutionContext::default(),
                &mounted_servers,
            )
            .await;

        assert!(snapshot.schemas().iter().any(|schema| {
            schema.get("name").and_then(|value| value.as_str())
                == Some("mcp__openai_docs__search_openai_docs")
        }));
        assert!(snapshot.schemas().iter().any(|schema| {
            schema.get("name").and_then(|value| value.as_str()) == Some("mcp_server__openai_docs")
        }));
    }

    #[tokio::test]
    async fn test_snapshot_restores_persisted_mounted_mcp_server_from_conversation_metadata() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let conversation_id = format!("test-mounted-mcp-{}", uuid::Uuid::new_v4());
        conversation_store::ensure_conversation(&conversation_id).expect("ensure conversation");
        conversation_store::update_conversation_mounted_tool_sources(
            &conversation_id,
            vec![conversation_store::ConversationMountedToolSource {
                source_id: "openai_docs".to_string(),
                source_type: "mcp".to_string(),
                tools: vec![conversation_store::ConversationMountedToolDefinition {
                    tool_name: "search_openai_docs".to_string(),
                    display_name: "Search Openai Docs".to_string(),
                    description: "Search docs".to_string(),
                    icon: Some("LayoutGrid".to_string()),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "query": { "type": "string" }
                        }
                    }),
                }],
            }],
        )
        .expect("persist mounted tool sources");

        let mut config = AppConfig::default();
        config
            .mcp_servers
            .insert("openai_docs".to_string(), McpServerConfig::default());
        let catalog = ToolCatalog::new(Arc::new(crate::mcp::McpManager::new()), &config);
        let snapshot = catalog
            .snapshot(&ToolExecutionContext {
                conversation_id: Some(conversation_id.clone()),
            })
            .await;

        assert!(
            snapshot
                .active_tool_names()
                .contains("mcp_server__openai_docs")
        );
        assert!(
            snapshot
                .active_tool_names()
                .contains("mcp__openai_docs__search_openai_docs")
        );

        conversation_store::delete_conversation(&conversation_id).ok();
    }

    #[tokio::test]
    async fn test_unfolded_mcp_server_bypasses_control_tool_and_exposes_tools() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();

        let mut config = AppConfig::default();
        config.mcp_servers.insert(
            "unfolded_server".to_string(),
            McpServerConfig {
                enabled: true,
                unfolded: true,
                ..McpServerConfig::default()
            },
        );

        let catalog = ToolCatalog::new(Arc::new(crate::mcp::McpManager::new()), &config);
        let snapshot = catalog.snapshot(&ToolExecutionContext::default()).await;

        assert!(
            !snapshot
                .active_tool_names()
                .contains("mcp_server__unfolded_server")
        );
    }
}
