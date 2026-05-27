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

    struct TestHomeGuard {
        home: std::path::PathBuf,
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
    }

    #[test]
    fn test_calculator_errors() {
        let calc = CalculatorTool;
        let ctx = ToolExecutionContext::default();

        // Division by zero
        let args = json!({ "expression": "5 / 0" });
        let err = calc.execute(&args, &ctx).unwrap_err();
        assert!(err.contains("Division by zero"));

        // Sqrt of negative
        let args = json!({ "expression": "sqrt(-4)" });
        let err = calc.execute(&args, &ctx).unwrap_err();
        assert!(err.contains("Cannot compute square root of a negative number"));

        // Unbalanced parentheses
        let args = json!({ "expression": "(2 + 3" });
        let err = calc.execute(&args, &ctx).unwrap_err();
        assert!(
            err.contains("Missing matching closing parenthesis")
                || err.contains("Unexpected end of expression")
        );

        // Missing sqrt parentheses
        let args = json!({ "expression": "sqrt 16" });
        let err = calc.execute(&args, &ctx).unwrap_err();
        assert!(
            err.contains("sqrt function requires parenthesis")
                || err.contains("Unsupported function")
        );
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

        let filename = "same_name.txt";
        writer
            .execute(&json!({"filename": filename, "content": "from-a"}), &ctx_a)
            .unwrap();
        writer
            .execute(&json!({"filename": filename, "content": "from-b"}), &ctx_b)
            .unwrap();

        let read_a = reader
            .execute(&json!({"filename": filename}), &ctx_a)
            .unwrap();
        let read_b = reader
            .execute(&json!({"filename": filename}), &ctx_b)
            .unwrap();
        assert_eq!(read_a["content"], "from-a");
        assert_eq!(read_b["content"], "from-b");

        conversation_store::delete_conversation(&conversation_a).unwrap();
        conversation_store::delete_conversation(&conversation_b).unwrap();
    }

    #[test]
    fn test_tool_registry() {
        let registry = ToolRegistry::new_with_defaults();
        let schemas = registry.list_schemas();
        assert_eq!(schemas.len(), 4);

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

        assert_eq!(responses_schemas.len(), 4);
        assert_eq!(cc_schemas.len(), 4);

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
    }

    #[tokio::test]
    async fn test_tool_catalog_snapshot_freezes_native_tool_view() {
        let config = crate::config::AppConfig::default();
        let catalog = ToolCatalog::new(Arc::new(crate::mcp::McpManager::new()), &config);
        let snapshot = catalog.snapshot(&ToolExecutionContext::default()).await;

        assert_eq!(snapshot.schemas().len(), 4);
        assert!(snapshot.active_tool_names().contains("calculator"));
        assert!(snapshot.active_tool_names().contains("get_system_time"));

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
