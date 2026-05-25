#[cfg(test)]
mod tests {
    use crate::tools::{
        CalculatorTool, FileReaderTool, FileWriterTool, SystemTimeTool, Tool, ToolRegistry,
    };
    use serde_json::json;
    use std::fs;

    #[test]
    fn test_calculator_success() {
        let calc = CalculatorTool;

        // Basic arithmetic
        let args = json!({ "expression": "2 + 3 * 4" });
        let res = calc.execute(&args).unwrap();
        assert_eq!(res["result"].as_f64().unwrap(), 14.0);

        // Exponentiation
        let args = json!({ "expression": "2^3" });
        let res = calc.execute(&args).unwrap();
        assert_eq!(res["result"].as_f64().unwrap(), 8.0);

        // Complex expressions with parentheses and sqrt
        let args = json!({ "expression": "2 * (3.5 + 4.5) / sqrt(16)" });
        let res = calc.execute(&args).unwrap();
        assert_eq!(res["result"].as_f64().unwrap(), 4.0);

        // Negative numbers
        let args = json!({ "expression": "-3 + 5" });
        let res = calc.execute(&args).unwrap();
        assert_eq!(res["result"].as_f64().unwrap(), 2.0);
    }

    #[test]
    fn test_calculator_errors() {
        let calc = CalculatorTool;

        // Division by zero
        let args = json!({ "expression": "5 / 0" });
        let err = calc.execute(&args).unwrap_err();
        assert!(err.contains("Division by zero"));

        // Sqrt of negative
        let args = json!({ "expression": "sqrt(-4)" });
        let err = calc.execute(&args).unwrap_err();
        assert!(err.contains("Cannot compute square root of a negative number"));

        // Unbalanced parentheses
        let args = json!({ "expression": "(2 + 3" });
        let err = calc.execute(&args).unwrap_err();
        assert!(
            err.contains("Missing matching closing parenthesis")
                || err.contains("Unexpected end of expression")
        );

        // Missing sqrt parentheses
        let args = json!({ "expression": "sqrt 16" });
        let err = calc.execute(&args).unwrap_err();
        assert!(
            err.contains("sqrt function requires parenthesis")
                || err.contains("Unsupported function")
        );
    }

    #[test]
    fn test_system_time() {
        let time_tool = SystemTimeTool;
        let args = json!({});
        let res = time_tool.execute(&args).unwrap();

        assert!(res.get("localTime").is_some());
        assert!(res["unixTimestampMs"].as_i64().unwrap() > 0);
    }

    #[test]
    fn test_file_tools_sandbox() {
        let reader = FileReaderTool;
        let writer = FileWriterTool;

        let filename = "test_sandbox_run.txt";
        let test_content = "Hello from Tauri Agent Sandbox Tool Test!";

        // 1. Write the file
        let write_args = json!({
            "filename": filename,
            "content": test_content
        });
        let write_res = writer.execute(&write_args).unwrap();
        assert_eq!(write_res["status"], "success");
        assert_eq!(
            write_res["bytesWritten"].as_u64().unwrap(),
            test_content.len() as u64
        );

        // 2. Read the file back
        let read_args = json!({
            "filename": filename
        });
        let read_res = reader.execute(&read_args).unwrap();
        assert_eq!(read_res["content"].as_str().unwrap(), test_content);

        // 3. Verify directory traversal protection.
        // The path validator must extract only the file name part.
        // Therefore writing to "../traversal_test.txt" should write to "traversal_test.txt" inside sandbox.
        let traversal_filename = "../traversal_test.txt";
        let traversal_args = json!({
            "filename": traversal_filename,
            "content": "traversal-secured"
        });
        let traversal_res = writer.execute(&traversal_args).unwrap();
        assert_eq!(traversal_res["filename"], traversal_filename); // returns input filename back

        // Reading it via pure filename "traversal_test.txt" should succeed because it was sandboxed
        let read_clean_args = json!({
            "filename": "traversal_test.txt"
        });
        let read_clean_res = reader.execute(&read_clean_args).unwrap();
        assert_eq!(
            read_clean_res["content"].as_str().unwrap(),
            "traversal-secured"
        );

        // Clean up
        let _ = fs::remove_file(FileReaderTool::validate_path(&reader, filename).unwrap());
        let _ =
            fs::remove_file(FileReaderTool::validate_path(&reader, "traversal_test.txt").unwrap());
    }

    #[test]
    fn test_tool_registry() {
        let registry = ToolRegistry::new_with_defaults();
        let schemas = registry.list_schemas();
        assert_eq!(schemas.len(), 4);

        // Execute via registry
        let args = json!({ "expression": "100 * 2.5" });
        let res = registry.execute("calculator", &args).unwrap();
        assert_eq!(res["result"].as_f64().unwrap(), 250.0);
    }
}
