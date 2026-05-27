use super::types::{CalculatorResponse, FEND_TIMEOUT_MS, MAX_EXPRESSION_LENGTH, MAX_PRECISION};
use serde_json::{json, Value};

pub(crate) fn capabilities_response(expression: Option<String>) -> CalculatorResponse {
    CalculatorResponse {
        expression,
        normalized_expression: Some("capabilities()".to_string()),
        mode: "capabilities".to_string(),
        result: json!("calculator capabilities"),
        exact_value: None,
        approximate_value: None,
        unit: None,
        steps: Vec::new(),
        warnings: Vec::new(),
        used_approximation: false,
        capabilities: Some(calculator_capabilities()),
    }
}

pub(crate) fn is_capability_query(expression: &str) -> bool {
    matches!(
        expression.trim().to_ascii_lowercase().as_str(),
        "capabilities()" | "capabilities" | "help()" | "help"
    )
}

fn calculator_capabilities() -> Value {
    json!({
        "version": 3,
        "modes": ["auto", "capabilities", "evaluate"],
        "supports": {
            "symbolicMath": false,
            "equationSolving": false,
            "equationSystems": false,
            "units": true,
            "complexNumbers": true,
            "structuredReturn": true,
            "precisionControl": true,
            "recoverableErrors": true
        },
        "engine": {
            "name": "fend-core",
            "policy": "All calculator expressions are evaluated directly by fend-core.",
            "notes": [
                "Legacy symbolic engine has been removed.",
                "Legacy statrs helper preprocessing has been removed.",
                "Use natural fend expressions for numeric, unit, and complex arithmetic."
            ]
        },
        "syntax": {
            "naturalForms": [
                "sin pi/2",
                "2 * (3.5 + 4)",
                "3 km + 500 m",
                "60 km/h * 2 h",
                "sqrt(-4)",
                "(1 + i)^2"
            ],
            "variables": "Pass variable bindings with the optional 'variables' object to pre-substitute values before evaluation, for example {\"x\": 2.5}."
        },
        "resourceLimits": {
            "maxExpressionLength": MAX_EXPRESSION_LENGTH,
            "maxPrecision": MAX_PRECISION,
            "fendTimeoutMs": FEND_TIMEOUT_MS
        },
        "examples": [
            {
                "input": { "expression": "2 * (3.5 + 4.5) / sqrt(16)" },
                "summary": "numeric evaluation"
            },
            {
                "input": { "expression": "3 km + 500 m" },
                "summary": "unit-aware arithmetic"
            },
            {
                "input": { "expression": "sqrt(-4)" },
                "summary": "complex output"
            }
        ]
    })
}
