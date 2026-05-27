use crate::time_context::TimeSnapshot;
use crate::tools::math::evaluate_math_expression;
use crate::tools::{Tool, ToolExecutionContext};
use chrono::Local;
use serde_json::{json, Value};
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
