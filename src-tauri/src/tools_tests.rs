#[cfg(test)]
mod tests {
    use crate::conversation_store;
    use crate::tools::{
        CalculatorTool, FileReaderTool, FileWriterTool, SystemTimeTool, Tool, ToolExecutionContext,
        ToolRegistry, ToolSchemaFormat,
    };
    use serde_json::json;

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
        let reader = FileReaderTool;
        let writer = FileWriterTool;
        let utility_model = "gpt-5-mini";
        let conversation_a = format!("test-workspace-a-{}", uuid::Uuid::new_v4());
        let conversation_b = format!("test-workspace-b-{}", uuid::Uuid::new_v4());

        conversation_store::ensure_conversation(&conversation_a, utility_model).unwrap();
        conversation_store::ensure_conversation(&conversation_b, utility_model).unwrap();

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
}
