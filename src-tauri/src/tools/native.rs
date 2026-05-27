use crate::conversation_store;
use crate::time_context::TimeSnapshot;
use crate::tools::math::evaluate_math_expression;
use crate::tools::{Tool, ToolExecutionContext};
use chrono::Local;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct CalculatorTool;

impl Tool for CalculatorTool {
    fn name(&self) -> &'static str {
        "calculator"
    }

    fn display_name(&self) -> &'static str {
        "Calculator"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("Calculator")
    }

    fn description(&self) -> &'static str {
        "Safely evaluates mathematical expressions with arithmetic, powers, remainder (%), constants (pi, e, tau, phi), standard functions (sqrt, abs, exp, ln, trig, rounding, min/max), and advanced helpers like gamma, beta, erf, factorial, ncr, npr, logistic, harmonic, sum, mean, and product."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "The mathematical expression to evaluate, e.g. '2 * (3.5 + 4) / sqrt(16)' or 'mean(2, 4, 6) + gamma(5) - ncr(6, 2)'"
                }
            },
            "required": ["expression"]
        })
    }

    fn execute(&self, arguments: &Value, _context: &ToolExecutionContext) -> Result<Value, String> {
        let expression = arguments
            .get("expression")
            .and_then(Value::as_str)
            .ok_or_else(|| "Missing required parameter 'expression'".to_string())?;

        let clean_expr = expression.replace(' ', "");
        let result = evaluate_math_expression(&clean_expr)?;

        Ok(json!({
            "expression": expression,
            "result": result
        }))
    }
}

pub struct SystemTimeTool;

impl Tool for SystemTimeTool {
    fn name(&self) -> &'static str {
        "get_system_time"
    }

    fn display_name(&self) -> &'static str {
        "Get System Time"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("Clock3")
    }

    fn description(&self) -> &'static str {
        "Returns the current date and time of the host system."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    fn execute(
        &self,
        _arguments: &Value,
        _context: &ToolExecutionContext,
    ) -> Result<Value, String> {
        let now = SystemTime::now();
        let duration = now
            .duration_since(UNIX_EPOCH)
            .map_err(|e| format!("System clock error: {e}"))?;

        let unix_ms = duration.as_millis() as i64;
        let snapshot = TimeSnapshot::from_unix_ms(unix_ms);
        let local_timezone = Local::now().offset().to_string();

        Ok(json!({
            "utcTime": snapshot.utc_rfc3339,
            "localTime": snapshot.local_rfc3339,
            "localOffset": snapshot.local_offset,
            "localTimezone": local_timezone,
            "unixTimestampMs": unix_ms
        }))
    }
}

pub struct FileReaderTool;

impl FileReaderTool {
    fn get_workspace_dir(context: &ToolExecutionContext) -> Result<PathBuf, String> {
        let dir = if let Some(conversation_id) = context
            .conversation_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            conversation_store::conversation_workspace_path(conversation_id)?
        } else {
            return Err(
                "Missing conversation context for file tool. File tools require a conversation workspace."
                    .to_string(),
            );
        };
        if !dir.exists() {
            fs::create_dir_all(&dir).map_err(|e| {
                format!(
                    "Failed to create workspace directory {}: {e}",
                    dir.display()
                )
            })?;
        }
        Ok(dir)
    }

    pub(crate) fn validate_path(
        &self,
        filename: &str,
        context: &ToolExecutionContext,
    ) -> Result<PathBuf, String> {
        let workspace_dir = Self::get_workspace_dir(context)?;
        let path = Path::new(filename);
        let filename_only = path
            .file_name()
            .ok_or_else(|| "Invalid filename".to_string())?;
        Ok(workspace_dir.join(filename_only))
    }
}

impl Tool for FileReaderTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn display_name(&self) -> &'static str {
        "Read File"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("FileSearch")
    }

    fn description(&self) -> &'static str {
        "Reads the text content of a file located in the current conversation workspace."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "filename": {
                    "type": "string",
                    "description": "The name of the file to read (e.g. 'results.txt')"
                }
            },
            "required": ["filename"]
        })
    }

    fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> Result<Value, String> {
        let filename = arguments
            .get("filename")
            .and_then(Value::as_str)
            .ok_or_else(|| "Missing required parameter 'filename'".to_string())?;

        let safe_path = self.validate_path(filename, context)?;
        if !safe_path.exists() {
            return Err(format!(
                "File '{}' not found in current conversation workspace",
                filename
            ));
        }

        let content =
            fs::read_to_string(&safe_path).map_err(|e| format!("Failed to read file: {e}"))?;

        Ok(json!({
            "filename": filename,
            "content": content
        }))
    }
}

pub struct FileWriterTool;

impl FileWriterTool {
    fn get_workspace_dir(context: &ToolExecutionContext) -> Result<PathBuf, String> {
        let dir = if let Some(conversation_id) = context
            .conversation_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            conversation_store::conversation_workspace_path(conversation_id)?
        } else {
            return Err(
                "Missing conversation context for file tool. File tools require a conversation workspace."
                    .to_string(),
            );
        };
        if !dir.exists() {
            fs::create_dir_all(&dir).map_err(|e| {
                format!(
                    "Failed to create workspace directory {}: {e}",
                    dir.display()
                )
            })?;
        }
        Ok(dir)
    }

    pub(crate) fn validate_path(
        &self,
        filename: &str,
        context: &ToolExecutionContext,
    ) -> Result<PathBuf, String> {
        let workspace_dir = Self::get_workspace_dir(context)?;
        let path = Path::new(filename);
        let filename_only = path
            .file_name()
            .ok_or_else(|| "Invalid filename".to_string())?;
        Ok(workspace_dir.join(filename_only))
    }
}

impl Tool for FileWriterTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn display_name(&self) -> &'static str {
        "Write File"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("FilePenLine")
    }

    fn description(&self) -> &'static str {
        "Writes text content to a file located in the current conversation workspace. Overwrites existing contents."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "filename": {
                    "type": "string",
                    "description": "The name of the file to write to (e.g. 'results.txt')"
                },
                "content": {
                    "type": "string",
                    "description": "The text content to write"
                }
            },
            "required": ["filename", "content"]
        })
    }

    fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> Result<Value, String> {
        let filename = arguments
            .get("filename")
            .and_then(Value::as_str)
            .ok_or_else(|| "Missing required parameter 'filename'".to_string())?;

        let content = arguments
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| "Missing required parameter 'content'".to_string())?;

        let safe_path = self.validate_path(filename, context)?;
        fs::write(&safe_path, content).map_err(|e| format!("Failed to write to file: {e}"))?;

        Ok(json!({
            "filename": filename,
            "bytesWritten": content.len(),
            "status": "success"
        }))
    }
}
