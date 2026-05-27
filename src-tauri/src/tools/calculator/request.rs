use super::types::{
    CalculatorMode, CalculatorRequest, CalculatorVariableBinding, DEFAULT_PRECISION,
    MAX_EXPRESSION_LENGTH, MAX_PRECISION,
};
use serde_json::Value;

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

/// Normalize only display-layer characters that users commonly paste into the
/// calculator UI, and otherwise let fend-core own the full parse.
pub(crate) fn normalize_expression(expression: &str) -> Result<String, String> {
    let normalized = expression
        .trim()
        .replace('π', "pi")
        .replace('×', "*")
        .replace('÷', "/")
        .replace('−', "-")
        .replace('·', "*");

    Ok(normalized)
}

fn parse_variables(value: Option<&Value>) -> Result<Vec<CalculatorVariableBinding>, String> {
    let mut variables = Vec::new();
    if let Some(values) = value.and_then(Value::as_object) {
        for (name, raw) in values {
            validate_variable_name(name)?;
            variables.push(CalculatorVariableBinding {
                name: name.clone(),
                expression: parse_variable_expression(name, raw)?,
            });
        }
    }

    Ok(variables)
}

fn validate_variable_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err("Variable names cannot be empty.".to_string());
    };

    if !(first.is_ascii_alphabetic() || first == '_')
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(format!(
            "Variable '{name}' is not a valid fend identifier. Use letters, numbers, and underscores, and do not start with a number."
        ));
    }

    Ok(())
}

fn parse_variable_expression(name: &str, raw: &Value) -> Result<String, String> {
    match raw {
        Value::Number(number) => {
            if number.as_f64().is_some_and(|value| !value.is_finite()) {
                return Err(format!("Variable '{name}' must be finite."));
            }

            Ok(number.to_string())
        }
        Value::Bool(value) => Ok(value.to_string()),
        Value::String(value) => {
            let expression = value.trim();
            if expression.is_empty() {
                return Err(format!(
                    "Variable '{name}' cannot be an empty fend expression."
                ));
            }
            Ok(expression.to_string())
        }
        _ => Err(format!(
            "Variable '{name}' must be a number, boolean, or fend expression string."
        )),
    }
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
