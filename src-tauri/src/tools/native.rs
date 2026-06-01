use crate::error::AgentJaxResult;
use crate::time_context::TimeSnapshot;
use crate::tools::calculator;
use crate::tools::{Tool, ToolExecutionContext};
use chrono::Local;
use serde_json::{Value, json};
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
        "A structured agent-friendly calculator powered exclusively by fend-core. Supports natural numeric input, unit-aware arithmetic, and complex numbers. Use mode='capabilities' to inspect supported syntax."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "Primary calculator input. Supports numeric expressions like '2 * (3.5 + 4) / sqrt(16)', fend-native function calls like 'sin(pi / 2)', complex values like 'sqrt(-4)', and unit-aware arithmetic like '3 km + 500 m' or '60 km/h * 2 h'."
                },
                "mode": {
                    "type": "string",
                    "enum": ["auto", "capabilities", "evaluate"],
                    "description": "Optional explicit mode. Defaults to 'auto'. Use 'capabilities' to inspect current fend-core-only behavior."
                },
                "precision": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 32,
                    "description": "Approximation precision in decimal digits for numeric fields."
                },
                "variables": {
                    "type": "object",
                    "description": "Optional variable bindings translated into native fend assignments before evaluation. Values may be numbers, booleans, or fend expression strings such as {\"x\": 2.5, \"y\": \"60 kg\"}."
                },
            },
            "required": []
        })
    }

    fn execute(&self, arguments: &Value, _context: &ToolExecutionContext) -> AgentJaxResult<Value> {
        let request = calculator::parse_request(arguments)?;
        calculator::execute(request).map_err(Into::into)
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
    ) -> AgentJaxResult<Value> {
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
