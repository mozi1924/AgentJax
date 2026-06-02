use super::request::normalize_expression;
use super::types::{
    CalculatorMode, CalculatorRequest, CalculatorResponse, CalculatorVariableBinding,
    FEND_TIMEOUT_MS,
};
use crate::agentjax_err;
use crate::error::AgentJaxResult;
use fend_core::{Context as FendContext, Interrupt as FendInterrupt, evaluate_with_interrupt};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// Interrupt guard for fend-core computations.
/// This keeps the calculator responsive even for pathological inputs.
struct DeadlineInterrupt {
    deadline: Instant,
}

impl DeadlineInterrupt {
    fn new(timeout: Duration) -> Self {
        Self {
            deadline: Instant::now() + timeout,
        }
    }
}

impl FendInterrupt for DeadlineInterrupt {
    fn should_interrupt(&self) -> bool {
        Instant::now() >= self.deadline
    }
}

#[derive(Debug)]
struct FendPayload {
    result: Value,
    exact_value: Option<String>,
    approximate_value: Option<Value>,
    unit: Option<String>,
    warnings: Vec<String>,
    used_approximation: bool,
}

/// Execute calculator request using fend-core only.
pub(crate) fn execute(request: CalculatorRequest) -> AgentJaxResult<CalculatorResponse> {
    reject_unsupported_mode(request.mode)?;

    let original_expression = request.expression.clone();
    let expression = request
        .expression
        .as_deref()
        .ok_or_else(|| agentjax_err!("Missing calculator expression.", ToolExecution))?;
    let normalized = normalize_expression(expression)?;

    reject_unsupported_symbolic_call(&normalized)?;

    let prepared = build_fend_expression(&normalized, &request.variables);
    let evaluated = evaluate_with_fend(&prepared, request.precision)?;

    let mut warnings = evaluated.warnings;
    if !request.variables.is_empty() {
        warnings.push(
            "Injected native fend variable assignments from the 'variables' object before evaluation."
                .to_string(),
        );
    }
    Ok(CalculatorResponse {
        expression: original_expression,
        normalized_expression: Some(normalized),
        mode: "evaluate".to_string(),
        result: evaluated.result,
        exact_value: evaluated.exact_value,
        approximate_value: evaluated.approximate_value,
        unit: evaluated.unit,
        steps: Vec::new(),
        warnings,
        used_approximation: evaluated.used_approximation,
        capabilities: None,
    })
}

fn reject_unsupported_mode(mode: CalculatorMode) -> AgentJaxResult<()> {
    if matches!(mode, CalculatorMode::Auto | CalculatorMode::Evaluate) {
        return Ok(());
    }

    Err(agentjax_err!(
        format!("Unsupported mode '{}'. This calculator is now fend-core only; use mode='auto' or mode='evaluate', or pass mode='capabilities' to inspect supported behavior.", mode.as_str()),
        ToolExecution
    ))
}

fn reject_unsupported_symbolic_call(expression: &str) -> AgentJaxResult<()> {
    let Some((name, _)) = parse_top_level_call(expression) else {
        return Ok(());
    };

    let legacy_symbolic_calls = [
        "diff",
        "differentiate",
        "derivative",
        "integral",
        "integrate",
        "solve",
        "solve_system",
        "limit",
        "simplify",
        "factor",
        "expand",
    ];

    if legacy_symbolic_calls.contains(&name.as_str()) {
        return Err(agentjax_err!(
            format!("'{name}(...)' is no longer supported in the native calculator. The legacy symbolic engine was removed; please provide a direct fend-core expression instead."),
            ToolExecution
        ));
    }

    Ok(())
}

/// Translate structured variable bindings into ordinary fend assignments so the
/// engine, not our Rust layer, owns parsing and operator precedence.
fn build_fend_expression(expression: &str, variables: &[CalculatorVariableBinding]) -> String {
    if variables.is_empty() {
        return expression.to_string();
    }

    let mut out = String::with_capacity(expression.len() + variables.len() * 16);
    for binding in variables {
        out.push_str(&binding.name);
        out.push_str(" = (");
        out.push_str(&binding.expression);
        out.push_str("); ");
    }
    out.push_str(expression);
    out
}

fn evaluate_with_fend(expression: &str, precision: u32) -> AgentJaxResult<FendPayload> {
    let mut context = FendContext::new();
    let interrupt = DeadlineInterrupt::new(Duration::from_millis(FEND_TIMEOUT_MS));
    let result = evaluate_with_interrupt(expression, &mut context, &interrupt)
        .map_err(|err| agentjax_err!(
            format_evaluation_error(expression, &err.to_string()),
            ToolExecution
        ))?;

    let (rendered, warnings) = normalize_fend_output(result.get_main_result().trim());

    if rendered.is_empty() {
        return Ok(FendPayload {
            result: json!(null),
            exact_value: None,
            approximate_value: None,
            unit: None,
            warnings,
            used_approximation: false,
        });
    }

    let is_complex = looks_like_complex_result(&rendered);
    let exact_value = evaluate_exact_with_fend(expression);
    let (approx_numeric, unit) = if is_complex {
        (None, None)
    } else {
        split_numeric_result(&rendered)
    };

    let approximate_value = if is_complex {
        Some(json!(rendered.clone()))
    } else {
        approx_numeric.map(|value| json!(round_with_precision(value, precision)))
    };

    let result_value = if is_complex || unit.is_some() {
        json!(rendered)
    } else if let Some(value) = approx_numeric {
        json!(round_with_precision(value, precision))
    } else {
        json!(rendered)
    };

    let used_approximation = approximate_value.is_some() || is_complex;

    Ok(FendPayload {
        result: result_value,
        exact_value,
        approximate_value,
        unit,
        warnings,
        used_approximation,
    })
}

fn format_evaluation_error(expression: &str, reason: &str) -> String {
    format!("Calculator could not evaluate `{expression}`. {reason}")
}

fn parse_top_level_call(expression: &str) -> Option<(String, Vec<String>)> {
    let open = expression.find('(')?;
    if !expression.ends_with(')') {
        return None;
    }

    let name = expression[..open].trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return None;
    }

    let inner = &expression[open + 1..expression.len() - 1];
    let args = split_top_level_arguments(inner);
    Some((name.to_ascii_lowercase(), args))
}

fn split_top_level_arguments(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut depth_paren = 0usize;
    let mut depth_bracket = 0usize;
    let mut depth_brace = 0usize;
    let mut start = 0usize;

    for (index, ch) in input.char_indices() {
        match ch {
            '(' => depth_paren += 1,
            ')' => depth_paren = depth_paren.saturating_sub(1),
            '[' => depth_bracket += 1,
            ']' => depth_bracket = depth_bracket.saturating_sub(1),
            '{' => depth_brace += 1,
            '}' => depth_brace = depth_brace.saturating_sub(1),
            ',' if depth_paren == 0 && depth_bracket == 0 && depth_brace == 0 => {
                args.push(input[start..index].trim().to_string());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }

    let tail = input[start..].trim();
    if !tail.is_empty() {
        args.push(tail.to_string());
    }

    args
}

fn normalize_fend_output(rendered: &str) -> (String, Vec<String>) {
    let mut warnings = Vec::new();
    let mut cleaned = rendered.trim().to_string();

    if let Some(rest) = cleaned.strip_prefix("approx. ") {
        warnings.push(
            "Returned an approximate numeric result from the high-precision evaluator.".to_string(),
        );
        cleaned = rest.trim().to_string();
    }

    cleaned = cleaned
        .replace("0 + ", "")
        .replace("0 - ", "-")
        .replace(" i", "i");
    cleaned = strip_zero_imaginary_part(&cleaned);

    (cleaned, warnings)
}

fn evaluate_exact_with_fend(expression: &str) -> Option<String> {
    let exact_query = format!("({expression}) as exact");
    let mut context = FendContext::new();
    let interrupt = DeadlineInterrupt::new(Duration::from_millis(FEND_TIMEOUT_MS));
    let exact_result = evaluate_with_interrupt(&exact_query, &mut context, &interrupt).ok()?;
    let raw_rendered = exact_result.get_main_result().trim();

    if raw_rendered.starts_with("approx. ") {
        return None;
    }

    let (rendered, _) = normalize_fend_output(raw_rendered);
    if rendered.is_empty() || looks_like_complex_result(&rendered) {
        None
    } else {
        Some(rendered)
    }
}

fn split_numeric_result(rendered: &str) -> (Option<f64>, Option<String>) {
    if looks_like_complex_result(rendered) {
        return (None, None);
    }

    if let Some((number, unit)) = rendered.split_once(' ') {
        if let Ok(value) = number.parse::<f64>() {
            return (Some(value), Some(unit.trim().to_string()));
        }
        if !unit.trim().is_empty() {
            return (None, Some(unit.trim().to_string()));
        }
    }

    if let Ok(value) = rendered.parse::<f64>() {
        return (Some(value), None);
    }

    let numeric_prefix_len = rendered
        .char_indices()
        .take_while(|(_, ch)| {
            ch.is_ascii_digit() || matches!(ch, '.' | '-' | '+' | 'e' | 'E' | '∞')
        })
        .last()
        .map(|(index, ch)| index + ch.len_utf8())
        .unwrap_or(0);

    if numeric_prefix_len > 0 && numeric_prefix_len < rendered.len() {
        let number = rendered[..numeric_prefix_len].trim();
        let unit = rendered[numeric_prefix_len..].trim();
        if let Ok(value) = number.parse::<f64>() {
            return (Some(value), (!unit.is_empty()).then(|| unit.to_string()));
        }
    }

    (None, None)
}

fn looks_like_complex_result(rendered: &str) -> bool {
    let trimmed = rendered.trim();
    if !trimmed.ends_with('i') {
        return false;
    }

    let without_i = trimmed[..trimmed.len() - 1].trim_end();
    if without_i.is_empty() {
        return false;
    }

    without_i
        .chars()
        .all(|ch| ch.is_ascii_digit() || matches!(ch, ' ' | '+' | '-' | '.' | '/'))
}

fn strip_zero_imaginary_part(rendered: &str) -> String {
    for marker in [" + 0i", " - 0i"] {
        if let Some(prefix) = rendered.strip_suffix(marker) {
            return prefix.trim().to_string();
        }
    }
    rendered.to_string()
}

fn round_with_precision(value: f64, precision: u32) -> f64 {
    let factor = 10_f64.powi(precision.min(12) as i32);
    (value * factor).round() / factor
}
