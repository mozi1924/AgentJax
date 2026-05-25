#[cfg(test)]
mod tests {
    use crate::providers;
    use crate::tools::ToolSchemaFormat;
    use serde_json::json;

    #[test]
    fn test_provider_capability_lookup() {
        let codex = providers::get_capabilities("codex").expect("codex adapter should exist");
        assert!(!codex.supports_stored_responses);

        let openai_default =
            providers::get_capabilities("openai").expect("openai adapter should exist");
        assert!(openai_default.supports_stored_responses);

        let openai = providers::get_capabilities("openai-standard")
            .expect("openai-standard adapter should exist");
        assert!(openai.supports_stored_responses);

        let err = providers::get_capabilities("not-a-provider").unwrap_err();
        assert!(err.contains("Unsupported provider kind"));
    }

    #[test]
    fn test_provider_tool_schema_format_lookup() {
        let codex = providers::get_tool_schema_format("codex").expect("codex adapter should exist");
        assert_eq!(codex, ToolSchemaFormat::Responses);

        let openai_default =
            providers::get_tool_schema_format("openai").expect("openai adapter should exist");
        assert_eq!(openai_default, ToolSchemaFormat::Responses);

        let openai = providers::get_tool_schema_format("openai-standard")
            .expect("openai-standard adapter should exist");
        assert_eq!(openai, ToolSchemaFormat::Responses);
    }

    #[test]
    fn test_extract_pending_tool_calls_is_provider_scoped() {
        let output_items = vec![
            json!({
                "type": "function_call",
                "call_id": "call_a",
                "name": "tool_a",
                "arguments": {"x": 1}
            }),
            json!({
                "type": "function_call_output",
                "call_id": "call_a",
                "output": "ok"
            }),
            json!({
                "type": "function_call",
                "call_id": "call_b",
                "name": "tool_b",
                "arguments": {"y": 2}
            }),
        ];

        let pending = providers::extract_pending_tool_calls("codex", &output_items)
            .expect("codex adapter should extract pending calls");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].call_id, "call_b");
        assert_eq!(pending[0].name, "tool_b");
    }

    #[test]
    fn test_provider_builds_tool_result_and_continuation_input_items() {
        let user_input_item = providers::build_user_input_item("codex", "hello")
            .expect("codex adapter should build user input item");
        assert_eq!(
            user_input_item.get("role").and_then(|v| v.as_str()),
            Some("user")
        );

        let tool_output_item = providers::build_tool_result_input_item("codex", "call_1", "ok")
            .expect("codex adapter should build tool result item");
        assert_eq!(
            tool_output_item.get("type").and_then(|v| v.as_str()),
            Some("function_call_output")
        );
        assert_eq!(
            tool_output_item.get("call_id").and_then(|v| v.as_str()),
            Some("call_1")
        );

        let output_items = vec![
            json!({"type":"reasoning","id":"r1"}),
            json!({"type":"function_call","call_id":"call_1","name":"tool_a"}),
        ];
        let continuation_items = providers::compose_tool_continuation_input(
            "codex",
            &output_items,
            vec![tool_output_item.clone()],
        )
        .expect("codex adapter should build continuation input items");
        assert_eq!(continuation_items.len(), 2);
        assert_eq!(
            continuation_items[0].get("id").and_then(|v| v.as_str()),
            Some("r1")
        );
        assert_eq!(
            continuation_items[1].get("type").and_then(|v| v.as_str()),
            Some("function_call_output")
        );
    }
}
