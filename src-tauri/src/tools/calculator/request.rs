use super::types::{
    CalculatorMode, CalculatorRequest, DEFAULT_PRECISION, MAX_EXPRESSION_LENGTH, MAX_PRECISION,
};
use serde_json::Value;
use std::collections::HashMap;

/// Parse and validate external JSON payload into a stable internal request.
pub(crate) fn parse_request(arguments: &Value) -> Result<CalculatorRequest, String> {
    let expression = arguments
        .get("expression")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let mode = CalculatorMode::parse(arguments.get("mode").and_then(Value::as_str))?;
    let precision = arguments
        .get("precision")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_PRECISION as u64)
        .min(MAX_PRECISION as u64) as u32;
    let variables = parse_variables(arguments.get("variables"))?;

    if mode != CalculatorMode::Capabilities && expression.is_none() {
        return Err(
            "Missing calculator input. Provide 'expression', or use mode='capabilities' to inspect supported syntax."
                .to_string(),
        );
    }

    if let Some(expr) = &expression {
        validate_expression_budget(expr)?;
    }

    Ok(CalculatorRequest {
        expression,
        mode,
        precision,
        variables,
    })
}

/// Normalize lightweight user-facing expression variants before fend-core
/// evaluation while preserving calculator-friendly syntax expectations.
pub(crate) fn normalize_expression(expression: &str) -> Result<String, String> {
    let mut normalized = expression.trim().to_string();
    normalized = normalized
        .replace('π', "pi")
        .replace('×', "*")
        .replace('÷', "/")
        .replace('−', "-")
        .replace('·', "*");

    ensure_balanced_grouping(&normalized)?;

    if let Some(adapted) = wrap_simple_function_argument(&normalized) {
        return Ok(adapted);
    }

    Ok(normalized)
}

fn parse_variables(value: Option<&Value>) -> Result<HashMap<String, f64>, String> {
    let mut variables = HashMap::new();
    if let Some(values) = value.and_then(Value::as_object) {
        for (name, raw) in values {
            let number = raw.as_f64().ok_or_else(|| {
                format!(
                    "Variable '{name}' must be a finite number. Structured variable bindings currently accept only numeric values."
                )
            })?;

            if !number.is_finite() {
                return Err(format!(
                    "Variable '{name}' must be finite. Received {number}."
                ));
            }

            variables.insert(name.clone(), number);
        }
    }

    Ok(variables)
}

fn validate_expression_budget(expression: &str) -> Result<(), String> {
    if expression.len() > MAX_EXPRESSION_LENGTH {
        return Err(format!(
            "Expression is too long ({} characters). The calculator currently limits inputs to {MAX_EXPRESSION_LENGTH} characters to avoid runaway evaluations.",
            expression.len()
        ));
    }

    Ok(())
}

fn ensure_balanced_grouping(expression: &str) -> Result<(), String> {
    let mut paren_depth = 0i32;
    let mut bracket_depth = 0i32;

    for (position, ch) in expression.chars().enumerate() {
        match ch {
            '(' => paren_depth += 1,
            ')' => {
                paren_depth -= 1;
                if paren_depth < 0 {
                    return Err(format!(
                        "Failed to parse the expression. Parentheses do not match at position {position}. Near `{expression}`."
                    ));
                }
            }
            '[' => bracket_depth += 1,
            ']' => {
                bracket_depth -= 1;
                if bracket_depth < 0 {
                    return Err(format!(
                        "Failed to parse the expression. Brackets do not match at position {position}. Near `{expression}`."
                    ));
                }
            }
            _ => {}
        }
    }

    if paren_depth != 0 || bracket_depth != 0 {
        return Err(format!(
            "Failed to parse the expression. Look for an unclosed parenthesis or bracket in `{expression}`."
        ));
    }

    Ok(())
}

fn wrap_simple_function_argument(expression: &str) -> Option<String> {
    const SIMPLE_FUNCTIONS: &[&str] = &[
        "sin", "cos", "tan", "asin", "acos", "atan", "sqrt", "ln", "exp", "abs",
    ];

    for function in SIMPLE_FUNCTIONS {
        let prefix = format!("{function} ");
        if let Some(argument) = expression.strip_prefix(&prefix) {
            if !argument.starts_with('(') {
                return Some(format!("{function}({argument})"));
            }
        }
    }

    None
}
