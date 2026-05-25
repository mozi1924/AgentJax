use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config;

pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters_schema(&self) -> Value;
    fn execute(&self, arguments: &Value) -> Result<Value, String>;

    fn to_schema(&self) -> Value {
        json!({
            "type": "function",
            "name": self.name(),
            "description": self.description(),
            "parameters": self.parameters_schema(),
        })
    }
}

// 1. Calculator Tool
pub struct CalculatorTool;

impl Tool for CalculatorTool {
    fn name(&self) -> &'static str {
        "calculator"
    }

    fn description(&self) -> &'static str {
        "Safely evaluates basic mathematical expressions. Supports addition (+), subtraction (-), multiplication (*), division (/), exponentiation (^), square root (sqrt), and parentheses ()."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "The mathematical expression to evaluate, e.g. '2 * (3.5 + 4) / sqrt(16)'"
                }
            },
            "required": ["expression"]
        })
    }

    fn execute(&self, arguments: &Value) -> Result<Value, String> {
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

// 2. System Time Tool
pub struct SystemTimeTool;

impl Tool for SystemTimeTool {
    fn name(&self) -> &'static str {
        "get_system_time"
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

    fn execute(&self, _arguments: &Value) -> Result<Value, String> {
        let now = SystemTime::now();
        let duration = now
            .duration_since(UNIX_EPOCH)
            .map_err(|e| format!("System clock error: {e}"))?;

        let seconds = duration.as_secs();
        let minutes = seconds / 60;
        let hours = minutes / 60;
        let days = hours / 24;

        // Approximate calendar formatting
        let epoch_year = 1970;
        let mut year = epoch_year;
        let mut days_left = days;

        loop {
            let is_leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
            let days_in_year = if is_leap { 366 } else { 365 };
            if days_left >= days_in_year {
                days_left -= days_in_year;
                year += 1;
            } else {
                break;
            }
        }

        let is_leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
        let month_days = if is_leap {
            [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        } else {
            [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        };

        let mut month = 1;
        for &days_in_month in &month_days {
            if days_left >= days_in_month {
                days_left -= days_in_month;
                month += 1;
            } else {
                break;
            }
        }

        let day = days_left + 1;
        let hour = (hours % 24) as u32;
        let minute = (minutes % 60) as u32;
        let second = (seconds % 60) as u32;

        let formatted_time = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
            year, month, day, hour, minute, second
        );

        Ok(json!({
            "localTime": formatted_time,
            "unixTimestampMs": duration.as_millis() as i64
        }))
    }
}

// 3. File Reader Tool
pub struct FileReaderTool;

impl FileReaderTool {
    fn get_sandbox_dir() -> Result<PathBuf, String> {
        let dir = config::config_dir_path()?.join("sandbox");
        if !dir.exists() {
            fs::create_dir_all(&dir)
                .map_err(|e| format!("Failed to create sandbox directory: {e}"))?;
        }
        Ok(dir)
    }

    pub(crate) fn validate_path(&self, filename: &str) -> Result<PathBuf, String> {
        let sandbox_dir = Self::get_sandbox_dir()?;

        let path = Path::new(filename);
        let filename_only = path
            .file_name()
            .ok_or_else(|| "Invalid filename".to_string())?;

        // Ensure path remains strictly inside the sandbox (no subdirectory traversal)
        let resolved = sandbox_dir.join(filename_only);
        Ok(resolved)
    }
}

impl Tool for FileReaderTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Reads the text content of a file located in the safe sandboxed directory."
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

    fn execute(&self, arguments: &Value) -> Result<Value, String> {
        let filename = arguments
            .get("filename")
            .and_then(Value::as_str)
            .ok_or_else(|| "Missing required parameter 'filename'".to_string())?;

        let safe_path = self.validate_path(filename)?;
        if !safe_path.exists() {
            return Err(format!("File '{}' not found in sandbox", filename));
        }

        let content =
            fs::read_to_string(&safe_path).map_err(|e| format!("Failed to read file: {e}"))?;

        Ok(json!({
            "filename": filename,
            "content": content
        }))
    }
}

// 4. File Writer Tool
pub struct FileWriterTool;

impl FileWriterTool {
    fn get_sandbox_dir() -> Result<PathBuf, String> {
        let dir = config::config_dir_path()?.join("sandbox");
        if !dir.exists() {
            fs::create_dir_all(&dir)
                .map_err(|e| format!("Failed to create sandbox directory: {e}"))?;
        }
        Ok(dir)
    }

    pub(crate) fn validate_path(&self, filename: &str) -> Result<PathBuf, String> {
        let sandbox_dir = Self::get_sandbox_dir()?;

        let path = Path::new(filename);
        let filename_only = path
            .file_name()
            .ok_or_else(|| "Invalid filename".to_string())?;

        let resolved = sandbox_dir.join(filename_only);
        Ok(resolved)
    }
}

impl Tool for FileWriterTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Writes text content to a file located in the safe sandboxed directory. Overwrites existing contents."
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

    fn execute(&self, arguments: &Value) -> Result<Value, String> {
        let filename = arguments
            .get("filename")
            .and_then(Value::as_str)
            .ok_or_else(|| "Missing required parameter 'filename'".to_string())?;

        let content = arguments
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| "Missing required parameter 'content'".to_string())?;

        let safe_path = self.validate_path(filename)?;

        fs::write(&safe_path, content).map_err(|e| format!("Failed to write to file: {e}"))?;

        Ok(json!({
            "filename": filename,
            "bytesWritten": content.len(),
            "status": "success"
        }))
    }
}

// Global Registry for Tools
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new_with_defaults() -> Self {
        Self {
            tools: vec![
                Box::new(CalculatorTool),
                Box::new(SystemTimeTool),
                Box::new(FileReaderTool),
                Box::new(FileWriterTool),
            ],
        }
    }

    pub fn list_schemas(&self) -> Vec<Value> {
        self.tools.iter().map(|tool| tool.to_schema()).collect()
    }

    pub fn execute(&self, name: &str, arguments: &Value) -> Result<Value, String> {
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.name() == name)
            .ok_or_else(|| format!("Tool '{}' not found in registry", name))?;

        tool.execute(arguments)
    }
}

// Unified Tool Catalog bridging native tools and dynamic MCP tools
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct ToolCatalog {
    native_tools: Vec<Box<dyn Tool>>,
    mcp_manager: Arc<crate::mcp::McpManager>,
    mcp_config: BTreeMap<String, crate::config::McpServerConfig>,
}

impl ToolCatalog {
    pub fn new(
        mcp_manager: Arc<crate::mcp::McpManager>,
        config: &crate::config::AppConfig,
    ) -> Self {
        Self {
            native_tools: vec![
                Box::new(CalculatorTool),
                Box::new(SystemTimeTool),
                Box::new(FileReaderTool),
                Box::new(FileWriterTool),
            ],
            mcp_manager,
            mcp_config: config.mcp_servers.clone(),
        }
    }

    pub async fn list_schemas(&self) -> Vec<Value> {
        let mut schemas = Vec::new();

        // 1. Native tools
        for tool in &self.native_tools {
            schemas.push(tool.to_schema());
        }

        // 2. MCP tools
        for (server_id, server_config) in &self.mcp_config {
            if !server_config.enabled {
                continue;
            }
            match self.mcp_manager.list_tools(server_id, server_config).await {
                Ok(mcp_tools) => {
                    for raw_tool in mcp_tools {
                        let raw_name = raw_tool
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        if raw_name.is_empty() {
                            continue;
                        }

                        // Prefix naming mapping: mcp__<server_id>__<tool_name>
                        let prefixed_name = format!("mcp__{}__{}", server_id, raw_name);

                        let description = raw_tool
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let input_schema = raw_tool
                            .get("inputSchema")
                            .or_else(|| raw_tool.get("input_schema"))
                            .cloned()
                            .unwrap_or(json!({
                                "type": "object",
                                "properties": {}
                            }));

                        schemas.push(json!({
                            "type": "function",
                            "name": prefixed_name,
                            "description": description,
                            "parameters": input_schema,
                        }));
                    }
                }
                Err(err) => {
                    log::warn!(
                        "Failed to list tools from MCP server '{}': {}",
                        server_id,
                        err
                    );
                }
            }
        }

        schemas
    }

    pub async fn execute(&self, prefixed_name: &str, arguments: &Value) -> Result<Value, String> {
        // Check if it's an MCP tool
        if prefixed_name.starts_with("mcp__") {
            let parts: Vec<&str> = prefixed_name.split("__").collect();
            if parts.len() >= 3 && parts[0] == "mcp" {
                let server_id = parts[1];
                let tool_name = parts[2..].join("__");

                let server_config = self
                    .mcp_config
                    .get(server_id)
                    .ok_or_else(|| format!("MCP server '{}' config not found", server_id))?;

                return self
                    .mcp_manager
                    .call_tool(server_id, server_config, &tool_name, arguments.clone())
                    .await;
            }
        }

        // Native tool
        let tool = self
            .native_tools
            .iter()
            .find(|tool| tool.name() == prefixed_name)
            .ok_or_else(|| format!("Tool '{}' not found in catalog", prefixed_name))?;

        tool.execute(arguments)
    }
}

// ----------------- Pure-Rust Safe Math Evaluator -----------------
fn evaluate_math_expression(expr: &str) -> Result<f64, String> {
    let mut chars = expr.chars().peekable();
    let val = parse_add_sub(&mut chars)?;
    if let Some(&c) = chars.peek() {
        return Err(format!("Unexpected character '{}' at end of expression", c));
    }
    Ok(val)
}

fn parse_add_sub<I>(chars: &mut std::iter::Peekable<I>) -> Result<f64, String>
where
    I: Iterator<Item = char>,
{
    let mut val = parse_mul_div(chars)?;
    while let Some(&c) = chars.peek() {
        if c == '+' {
            chars.next();
            val += parse_mul_div(chars)?;
        } else if c == '-' {
            chars.next();
            val -= parse_mul_div(chars)?;
        } else {
            break;
        }
    }
    Ok(val)
}

fn parse_mul_div<I>(chars: &mut std::iter::Peekable<I>) -> Result<f64, String>
where
    I: Iterator<Item = char>,
{
    let mut val = parse_exp(chars)?;
    while let Some(&c) = chars.peek() {
        if c == '*' {
            chars.next();
            val *= parse_exp(chars)?;
        } else if c == '/' {
            chars.next();
            let divisor = parse_exp(chars)?;
            if divisor == 0.0 {
                return Err("Division by zero".to_string());
            }
            val /= divisor;
        } else {
            break;
        }
    }
    Ok(val)
}

fn parse_exp<I>(chars: &mut std::iter::Peekable<I>) -> Result<f64, String>
where
    I: Iterator<Item = char>,
{
    let mut val = parse_primary(chars)?;
    while let Some(&c) = chars.peek() {
        if c == '^' {
            chars.next();
            val = val.powf(parse_primary(chars)?);
        } else {
            break;
        }
    }
    Ok(val)
}

fn parse_primary<I>(chars: &mut std::iter::Peekable<I>) -> Result<f64, String>
where
    I: Iterator<Item = char>,
{
    if let Some(&c) = chars.peek() {
        if c == '(' {
            chars.next();
            let val = parse_add_sub(chars)?;
            if chars.next() != Some(')') {
                return Err("Missing matching closing parenthesis".to_string());
            }
            return Ok(val);
        }

        if c == '-' {
            chars.next();
            return Ok(-parse_primary(chars)?);
        }

        if c == '+' {
            chars.next();
            return parse_primary(chars);
        }

        // Check for functions like sqrt
        if c.is_ascii_alphabetic() {
            let mut func = String::new();
            while let Some(&next_c) = chars.peek() {
                if next_c.is_ascii_alphabetic() {
                    func.push(next_c);
                    chars.next();
                } else {
                    break;
                }
            }

            if func == "sqrt" {
                if chars.peek() != Some(&'(') {
                    return Err("sqrt function requires parenthesis, e.g. sqrt(16)".to_string());
                }
                chars.next(); // Consume '('
                let val = parse_add_sub(chars)?;
                if chars.next() != Some(')') {
                    return Err("Missing closing parenthesis for sqrt".to_string());
                }
                if val < 0.0 {
                    return Err("Cannot compute square root of a negative number".to_string());
                }
                return Ok(val.sqrt());
            }

            return Err(format!("Unsupported function/variable name '{}'", func));
        }

        // Parse number
        if c.is_ascii_digit() || c == '.' {
            let mut num_str = String::new();
            while let Some(&next_c) = chars.peek() {
                if next_c.is_ascii_digit() || next_c == '.' {
                    num_str.push(next_c);
                    chars.next();
                } else {
                    break;
                }
            }

            let num = num_str
                .parse::<f64>()
                .map_err(|e| format!("Failed to parse number '{}': {e}", num_str))?;
            return Ok(num);
        }
    }

    Err("Unexpected end of expression".to_string())
}
