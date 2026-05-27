#[cfg(test)]
mod tests {
    use crate::agentjax_home::AGENTJAX_HOME_ENV;
    use crate::config::{AppConfig, McpServerConfig};
    use crate::conversation_store;
    use crate::tools::{
        CalculatorTool, FileReaderTool, FileWriterTool, MountedMcpServerSession,
        MountedMcpServerSessions, MountedMcpToolDefinition, SystemTimeTool, Tool, ToolCatalog,
        ToolExecutionContext, ToolRegistry, ToolSchemaFormat,
    };
    use serde_json::json;
    use std::sync::Arc;

    const DEFAULT_NATIVE_TOOL_COUNT: usize = 12;

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

        // Statistical and special functions backed by external math crates
        let args = json!({ "expression": "gamma(5) + ncr(6, 2) + mean(2, 4, 6)" });
        let res = calc.execute(&args, &ctx).unwrap();
        assert_approx_eq(res["result"].as_f64().unwrap(), 43.0, 1e-12);

        let args = json!({ "expression": "beta(2, 3) + harmonic(4) + logistic(0)" });
        let res = calc.execute(&args, &ctx).unwrap();
        assert_approx_eq(
            res["result"].as_f64().unwrap(),
            2.666_666_666_666_666_5,
            1e-12,
        );

        let args = json!({ "expression": "erf(1)" });
        let res = calc.execute(&args, &ctx).unwrap();
        assert_approx_eq(
            res["result"].as_f64().unwrap(),
            0.842_700_792_949_714_9,
            1e-11,
        );
    }

    #[test]
    fn test_calculator_errors() {
        let calc = CalculatorTool;
        let ctx = ToolExecutionContext::default();

        // Division by zero
        let args = json!({ "expression": "5 / 0" });
        let err = calc.execute(&args, &ctx).unwrap_err();
        assert!(err.contains("non-finite result"));

        // Sqrt of negative
        let args = json!({ "expression": "sqrt(-4)" });
        let err = calc.execute(&args, &ctx).unwrap_err();
        assert!(err.contains("non-finite result"));

        // Unbalanced parentheses
        let args = json!({ "expression": "(2 + 3" });
        let err = calc.execute(&args, &ctx).unwrap_err();
        assert!(
            err.contains("Failed to parse expression")
                || err.contains("Missing matching closing parenthesis")
                || err.contains("Unexpected end of expression")
        );

        // Invalid factorial domain
        let args = json!({ "expression": "factorial(3.2)" });
        let err = calc.execute(&args, &ctx).unwrap_err();
        assert!(err.contains("factorial requires an integer input"));

        // Invalid logit domain
        let args = json!({ "expression": "logit(1)" });
        let err = calc.execute(&args, &ctx).unwrap_err();
        assert!(err.contains("logit requires 0 < p < 1"));
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
        assert!(root_entries
            .iter()
            .any(|entry| entry["path"] == "README.md"));
        assert!(root_entries.iter().any(|entry| entry["path"] == "src"));

        let recursive_listing = registry
            .execute(
                "list_files",
                &json!({"path": "src", "recursive": true}),
                &ctx,
            )
            .unwrap();
        let recursive_entries = recursive_listing["entries"].as_array().unwrap();
        assert!(recursive_entries
            .iter()
            .any(|entry| entry["path"] == "src/components/Button.tsx"));

        let file_stat = registry
            .execute(
                "stat_file",
                &json!({"path": "src/components/Button.tsx"}),
                &ctx,
            )
            .unwrap();
        assert_eq!(file_stat["isFile"], true);
        assert_eq!(file_stat["kind"], "file");

        let dir_stat = registry
            .execute("stat_file", &json!({"path": "src/components"}), &ctx)
            .unwrap();
        assert_eq!(dir_stat["isDirectory"], true);
        assert_eq!(dir_stat["kind"], "directory");

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

        let stat = registry
            .execute("stat_file", &json!({"path": "notes/README"}), &ctx)
            .unwrap();
        assert_eq!(stat["contentKind"], "text");
        assert_eq!(stat["textReadable"], true);
        assert_eq!(stat["mediaType"], "text/plain");
        assert_eq!(stat["detectedFormat"], "Plain Text");
        assert_eq!(stat["typeDetectionSource"], "content_sniffing");

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

        let stat = registry
            .execute("stat_file", &json!({"path": "assets/fake-notes.txt"}), &ctx)
            .unwrap();
        assert_eq!(stat["contentKind"], "binary");
        assert_eq!(stat["textReadable"], false);
        assert_eq!(stat["mediaType"], "image/png");
        assert_eq!(stat["detectedExtension"], "png");

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
                "replace_text",
                &json!({
                    "path": "assets/image.bin",
                    "old_text": "a",
                    "new_text": "b"
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

        let new_binary_err = registry
            .execute(
                "write_file",
                &json!({"path": "assets/new-image.png", "content": "not a real png"}),
                &ctx,
            )
            .unwrap_err();
        assert!(new_binary_err.contains(".png"));

        let stat = registry
            .execute("stat_file", &json!({"path": "assets/image.bin"}), &ctx)
            .unwrap();
        assert_eq!(stat["contentKind"], "binary");
        assert_eq!(stat["textReadable"], false);

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
                "replace_text",
                &json!({
                    "path": "src/main.rs",
                    "old_text": "hello",
                    "new_text": "hi"
                }),
                &ctx,
            )
            .unwrap();

        registry
            .execute(
                "insert_after",
                &json!({
                    "path": "src/main.rs",
                    "anchor": "fn main() {",
                    "content": "\n    let value = 1;"
                }),
                &ctx,
            )
            .unwrap();

        registry
            .execute(
                "insert_before",
                &json!({
                    "path": "src/main.rs",
                    "anchor": "    println!(\"hi\");",
                    "content": "    // greeting\n"
                }),
                &ctx,
            )
            .unwrap();

        registry
            .execute(
                "replace_block",
                &json!({
                    "path": "src/main.rs",
                    "old_block": "    let value = 1;\n    // greeting\n    println!(\"hi\");",
                    "new_block": "    let value = 2;\n    // greeting\n    println!(\"hi\");"
                }),
                &ctx,
            )
            .unwrap();

        registry
            .execute(
                "apply_patch",
                &json!({
                    "path": "src/main.rs",
                    "edits": [
                        {
                            "op": "replace_text",
                            "old_text": "value = 2",
                            "new_text": "value = 3"
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
                "apply_patch",
                &json!({
                    "path": "notes/example.txt",
                    "edits": [
                        {
                            "op": "replace_text",
                            "old_text": "alpha",
                            "new_text": "ALPHA"
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
        assert_eq!(schemas.len(), DEFAULT_NATIVE_TOOL_COUNT);

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

        assert_eq!(responses_schemas.len(), DEFAULT_NATIVE_TOOL_COUNT);
        assert_eq!(cc_schemas.len(), DEFAULT_NATIVE_TOOL_COUNT);

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
    }

    #[tokio::test]
    async fn test_tool_catalog_snapshot_freezes_native_tool_view() {
        let config = crate::config::AppConfig::default();
        let catalog = ToolCatalog::new(Arc::new(crate::mcp::McpManager::new()), &config);
        let snapshot = catalog.snapshot(&ToolExecutionContext::default()).await;

        assert_eq!(snapshot.schemas().len(), DEFAULT_NATIVE_TOOL_COUNT);
        assert!(snapshot.active_tool_names().contains("calculator"));
        assert!(snapshot.active_tool_names().contains("get_system_time"));
        assert!(snapshot.active_tool_names().contains("list_files"));

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
        let mut mounted_servers = MountedMcpServerSessions::new();
        mounted_servers.insert(
            "openai_docs".to_string(),
            MountedMcpServerSession {
                server_id: "openai_docs".to_string(),
                server_config: McpServerConfig::default(),
                tools: vec![MountedMcpToolDefinition {
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
        conversation_store::update_conversation_mounted_mcp_servers(
            &conversation_id,
            vec![conversation_store::ConversationMountedMcpServer {
                server_id: "openai_docs".to_string(),
                tools: vec![conversation_store::ConversationMountedMcpToolDefinition {
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
        .expect("persist mounted MCP servers");

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

        assert!(snapshot
            .active_tool_names()
            .contains("mcp_server__openai_docs"));
        assert!(snapshot
            .active_tool_names()
            .contains("mcp__openai_docs__search_openai_docs"));

        conversation_store::delete_conversation(&conversation_id).ok();
    }
}
