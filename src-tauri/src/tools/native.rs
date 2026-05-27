use crate::time_context::TimeSnapshot;
use crate::tools::calculator;
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
        "A structured agent-friendly calculator with symbolic math, natural numeric input, unit-aware arithmetic, equation solving, limits, and legacy special-function support. Use mode='capabilities' to inspect supported syntax before generating expressions."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "Primary calculator input. Supports numeric expressions like '2 * (3.5 + 4) / sqrt(16)', symbolic calls like 'diff(x^3 + sin(x), x)', 'integral(x^2, x, 0, 1)', 'solve(x^2 - 5x + 6 = 0, x)', natural forms like 'sin pi/2', and unit-aware arithmetic like '3 km + 500 m' or '60 km/h * 2 h'."
                },
                "mode": {
                    "type": "string",
                    "enum": ["auto", "capabilities", "evaluate", "simplify", "differentiate", "integrate", "solve", "solve_system", "limit"],
                    "description": "Optional explicit mode. Defaults to 'auto'. Use 'capabilities' to ask the calculator what syntax and operations it supports."
                },
                "variable": {
                    "type": "string",
                    "description": "Target variable for differentiate/integrate/solve/limit modes when it cannot be inferred safely."
                },
                "lowerBound": {
                    "type": "string",
                    "description": "Optional lower bound. Used for definite integrals and as a limit target when mode='limit'."
                },
                "upperBound": {
                    "type": "string",
                    "description": "Optional upper bound. Used for definite integrals."
                },
                "precision": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 32,
                    "description": "Approximation precision in decimal digits for numeric fields. Exact symbolic strings are still returned separately when available."
                },
                "steps": {
                    "type": "string",
                    "enum": ["none", "summary", "detailed"],
                    "description": "Controls how much derivation detail is returned in the structured 'steps' field."
                },
                "variables": {
                    "type": "object",
                    "description": "Optional variable bindings for numeric evaluation, for example {\"x\": 2.5, \"y\": 1}."
                },
                "equations": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional equation list for system solving, for example ['x+y=3', 'x-y=1']."
                },
            },
            "required": []
        })
    }

    fn execute(&self, arguments: &Value, _context: &ToolExecutionContext) -> Result<Value, String> {
        let request = calculator::parse_request(arguments)?;
        calculator::execute(request)
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
