use crate::tools::math::evaluate_math_expression;
use fend_core::{evaluate_with_interrupt, Context as FendContext, Interrupt as FendInterrupt};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use thales::ast::{Expression, Variable};
use thales::integration::{definite_integral, integrate};
use thales::limits::{limit, LimitPoint, LimitResult};
use thales::parser::{parse_equation, parse_expression, ParseError};
use thales::resolution_path::ResolutionStep;
use thales::solver::{SmartSolver, Solution, Solver, SystemSolver};

const DEFAULT_PRECISION: u32 = 12;
const MAX_PRECISION: u32 = 32;
const MAX_EXPRESSION_LENGTH: usize = 512;
const MAX_EQUATION_COUNT: usize = 8;
const MAX_STEP_COUNT: usize = 24;
const FEND_TIMEOUT_MS: u64 = 200;

/// High-level execution mode exposed through the calculator tool schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CalculatorMode {
    Auto,
    Capabilities,
    Evaluate,
    Simplify,
    Differentiate,
    Integrate,
    Solve,
    SolveSystem,
    Limit,
}

impl CalculatorMode {
    fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("auto") {
            "auto" => Ok(Self::Auto),
            "capabilities" => Ok(Self::Capabilities),
            "evaluate" => Ok(Self::Evaluate),
            "simplify" => Ok(Self::Simplify),
            "differentiate" => Ok(Self::Differentiate),
            "integrate" => Ok(Self::Integrate),
            "solve" => Ok(Self::Solve),
            "solve_system" => Ok(Self::SolveSystem),
            "limit" => Ok(Self::Limit),
            other => Err(format!(
                "Unsupported calculator mode '{other}'. Try one of: auto, capabilities, evaluate, simplify, differentiate, integrate, solve, solve_system, limit."
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Capabilities => "capabilities",
            Self::Evaluate => "evaluate",
            Self::Simplify => "simplify",
            Self::Differentiate => "differentiate",
            Self::Integrate => "integrate",
            Self::Solve => "solve",
            Self::SolveSystem => "solve_system",
            Self::Limit => "limit",
        }
    }
}

/// Controls how much derivation detail is included in the structured response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StepsMode {
    None,
    Summary,
    Detailed,
}

impl StepsMode {
    fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("summary") {
            "none" => Ok(Self::None),
            "summary" => Ok(Self::Summary),
            "detailed" => Ok(Self::Detailed),
            other => Err(format!(
                "Unsupported steps mode '{other}'. Try one of: none, summary, detailed."
            )),
        }
    }
}

/// Parsed calculator request with normalized optional fields so the execution
/// pipeline can focus on math logic instead of JSON plumbing.
#[derive(Debug)]
pub(crate) struct CalculatorRequest {
    pub expression: Option<String>,
    pub mode: CalculatorMode,
    pub precision: u32,
    pub steps_mode: StepsMode,
    pub variable: Option<String>,
    pub lower_bound: Option<String>,
    pub upper_bound: Option<String>,
    pub variables: HashMap<String, f64>,
    pub equations: Vec<String>,
}

/// Structured calculator response. We keep a legacy `result` field for existing
/// tool consumers and layer richer metadata alongside it for agent workflows.
#[derive(Debug)]
struct CalculatorResponse {
    expression: Option<String>,
    normalized_expression: Option<String>,
    mode: String,
    result: Value,
    exact_value: Option<String>,
    approximate_value: Option<Value>,
    unit: Option<String>,
    steps: Vec<String>,
    warnings: Vec<String>,
    used_approximation: bool,
    capabilities: Option<Value>,
}

impl CalculatorResponse {
    fn into_value(self) -> Value {
        json!({
            "expression": self.expression,
            "normalizedExpression": self.normalized_expression,
            "mode": self.mode,
            "result": self.result,
            "exactValue": self.exact_value,
            "approximateValue": self.approximate_value,
            "unit": self.unit,
            "steps": self.steps,
            "warnings": self.warnings,
            "usedApproximation": self.used_approximation,
            "capabilities": self.capabilities,
        })
    }
}

/// A time-based interrupt used to keep natural-language/unit evaluation bounded.
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
    let steps_mode = StepsMode::parse(
        arguments
            .get("steps")
            .or_else(|| arguments.get("showSteps"))
            .and_then(Value::as_str),
    )?;
    let variable = arguments
        .get("variable")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let lower_bound = arguments
        .get("lowerBound")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let upper_bound = arguments
        .get("upperBound")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let mut variables = HashMap::new();
    if let Some(values) = arguments.get("variables").and_then(Value::as_object) {
        for (name, value) in values {
            let number = value.as_f64().ok_or_else(|| {
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

    let equations = arguments
        .get("equations")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if mode != CalculatorMode::Capabilities && expression.is_none() && equations.is_empty() {
        return Err(
            "Missing calculator input. Provide 'expression', or use mode='capabilities' to inspect supported syntax."
                .to_string(),
        );
    }

    if let Some(expr) = &expression {
        validate_expression_budget(expr)?;
    }
    if equations.len() > MAX_EQUATION_COUNT {
        return Err(format!(
            "Too many equations supplied ({}). The calculator currently limits system solving to {MAX_EQUATION_COUNT} equations per call.",
            equations.len()
        ));
    }
    for equation in &equations {
        validate_expression_budget(equation)?;
    }

    Ok(CalculatorRequest {
        expression,
        mode,
        precision,
        steps_mode,
        variable,
        lower_bound,
        upper_bound,
        variables,
        equations,
    })
}

pub(crate) fn execute(request: CalculatorRequest) -> Result<Value, String> {
    let response = execute_inner(request)?;
    Ok(response.into_value())
}

fn execute_inner(request: CalculatorRequest) -> Result<CalculatorResponse, String> {
    if request.mode == CalculatorMode::Capabilities
        || request
            .expression
            .as_deref()
            .map(is_capability_query)
            .unwrap_or(false)
    {
        return Ok(capabilities_response(request.expression));
    }

    if request.mode == CalculatorMode::SolveSystem || request.equations.len() > 1 {
        return solve_equation_system(&request);
    }

    let original_expression = request.expression.clone();
    let normalized_expression = request
        .expression
        .as_deref()
        .map(normalize_expression)
        .transpose()?;

    let normalized = normalized_expression
        .clone()
        .ok_or_else(|| "Missing calculator expression.".to_string())?;

    match request.mode {
        CalculatorMode::Capabilities => unreachable!(),
        CalculatorMode::Auto => execute_auto(&request, &normalized, original_expression),
        CalculatorMode::Evaluate => {
            evaluate_plain_expression(&request, &normalized, original_expression)
        }
        CalculatorMode::Simplify => simplify_expression(&request, &normalized, original_expression),
        CalculatorMode::Differentiate => {
            differentiate_expression(&request, &normalized, original_expression)
        }
        CalculatorMode::Integrate => {
            integrate_expression(&request, &normalized, original_expression)
        }
        CalculatorMode::Solve => solve_equation(&request, &normalized, original_expression),
        CalculatorMode::SolveSystem => unreachable!(),
        CalculatorMode::Limit => limit_expression(&request, &normalized, original_expression),
    }
}

fn execute_auto(
    request: &CalculatorRequest,
    normalized: &str,
    original_expression: Option<String>,
) -> Result<CalculatorResponse, String> {
    if let Some((name, args)) = parse_top_level_call(normalized) {
        return dispatch_top_level_call(request, &name, &args, original_expression);
    }

    if let Some((integrand, variable, lower, upper)) = parse_unicode_integral(normalized) {
        return integrate_from_parts(
            request,
            &integrand,
            &variable,
            lower.as_deref(),
            upper.as_deref(),
            original_expression,
            Some(normalized.to_string()),
        );
    }

    if normalized.contains('=') {
        return solve_equation(request, normalized, original_expression);
    }

    if let Ok(expr) = parse_expression(normalized) {
        let variables = expr.variables();
        if !variables.is_empty() {
            return evaluate_symbolic_expression(
                request,
                normalized,
                original_expression,
                expr,
                variables,
            );
        }
    }

    evaluate_plain_expression(request, normalized, original_expression)
}

fn dispatch_top_level_call(
    request: &CalculatorRequest,
    name: &str,
    args: &[String],
    original_expression: Option<String>,
) -> Result<CalculatorResponse, String> {
    match name {
        "capabilities" | "help" | "supported" => Ok(capabilities_response(original_expression)),
        "diff" | "differentiate" | "derivative" => {
            let expression = required_arg(args, 0, "diff(expr, variable[, order])")?;
            let variable = required_arg(args, 1, "diff(expr, variable[, order])")?;
            let order = args.get(2).cloned();
            differentiate_from_parts(
                request,
                expression,
                variable,
                order.as_deref(),
                original_expression,
                None,
            )
        }
        "integral" | "integrate" => {
            let expression =
                required_arg(args, 0, "integral(expr, variable[, lower, upper])")?;
            let variable = required_arg(args, 1, "integral(expr, variable[, lower, upper])")?;
            let lower = args.get(2).map(String::as_str);
            let upper = args.get(3).map(String::as_str);
            integrate_from_parts(
                request,
                expression,
                variable,
                lower,
                upper,
                original_expression,
                None,
            )
        }
        "solve" => {
            let equation = required_arg(args, 0, "solve(equation, variable)")?;
            let variable = required_arg(args, 1, "solve(equation, variable)")?;
            solve_equation_from_parts(request, equation, variable, original_expression, None)
        }
        "solve_system" => {
            if args.len() < 2 {
                return Err(
                    "solve_system requires at least two equations. Example: solve_system(x+y=3, x-y=1)"
                        .to_string(),
                );
            }
            let mut nested_request = CalculatorRequest {
                expression: original_expression.clone(),
                mode: CalculatorMode::SolveSystem,
                precision: request.precision,
                steps_mode: request.steps_mode,
                variable: request.variable.clone(),
                lower_bound: request.lower_bound.clone(),
                upper_bound: request.upper_bound.clone(),
                variables: request.variables.clone(),
                equations: args.to_vec(),
            };
            if nested_request.expression.is_none() {
                nested_request.expression = Some(format!("solve_system({})", args.join(", ")));
            }
            solve_equation_system(&nested_request)
        }
        "limit" => {
            let expression = required_arg(args, 0, "limit(expr, variable, target)")?;
            let variable = required_arg(args, 1, "limit(expr, variable, target)")?;
            let target = required_arg(args, 2, "limit(expr, variable, target)")?;
            limit_from_parts(request, expression, variable, target, original_expression, None)
        }
        "simplify" => {
            let expression = required_arg(args, 0, "simplify(expr)")?;
            simplify_with_expression(request, expression, original_expression, None)
        }
        "factor" | "expand" => Err(format!(
            "{name}(...) is not wired into the calculator tool yet. The current symbolic engine supports simplify, differentiate, integrate, solve, and limit. As a fallback, try simplify(...) or solve(...) depending on your goal."
        )),
        _ => {
            let fallback_expression = original_expression.clone().unwrap_or_default();
            if is_legacy_numeric_function(name) || is_passthrough_expression_function(name) {
                evaluate_plain_expression(request, &fallback_expression, original_expression)
            } else {
                let suggestion = closest_function_suggestion(name)
                    .map(|candidate| format!(" Did you mean `{candidate}`?"))
                    .unwrap_or_default();
                Err(format!(
                    "Unknown function '{name}'. This calculator currently supports diff, integral, solve, limit, simplify, and the documented numeric helpers.{suggestion}"
                ))
            }
        }
    }
}

fn evaluate_symbolic_expression(
    request: &CalculatorRequest,
    normalized: &str,
    original_expression: Option<String>,
    expr: Expression,
    variables: HashSet<String>,
) -> Result<CalculatorResponse, String> {
    let simplified = expr.simplify();
    let missing = variables
        .iter()
        .filter(|name| !request.variables.contains_key(*name))
        .cloned()
        .collect::<Vec<_>>();

    if missing.is_empty() {
        let numeric = simplified.evaluate(&request.variables).ok_or_else(|| {
            "The expression parsed symbolically, but could not be evaluated numerically with the provided variable bindings."
                .to_string()
        })?;
        return Ok(numeric_response(
            original_expression,
            Some(normalized.to_string()),
            request.mode.as_str(),
            numeric,
            Vec::new(),
            Vec::new(),
            request.precision,
        ));
    }

    let mut warnings = Vec::new();
    warnings.push(format!(
        "Returned a symbolic result because these variables are still unbound: {}.",
        missing.join(", ")
    ));
    Ok(symbolic_response(
        original_expression,
        Some(normalized.to_string()),
        "evaluate",
        &simplified,
        None,
        None,
        symbolic_steps(
            request.steps_mode,
            "Simplified the symbolic expression.",
            &format!("Result: {}", simplified),
        ),
        warnings,
    ))
}

fn evaluate_plain_expression(
    request: &CalculatorRequest,
    normalized: &str,
    original_expression: Option<String>,
) -> Result<CalculatorResponse, String> {
    match evaluate_with_fend(normalized, request.precision) {
        Ok(fend) => Ok(CalculatorResponse {
            expression: original_expression,
            normalized_expression: Some(normalized.to_string()),
            mode: "evaluate".to_string(),
            result: fend.result,
            exact_value: fend.exact_value,
            approximate_value: fend.approximate_value,
            unit: fend.unit,
            steps: Vec::new(),
            warnings: fend.warnings,
            used_approximation: fend.used_approximation,
            capabilities: None,
        }),
        Err(fend_error) => {
            let fallback = evaluate_math_expression(&normalized.replace(' ', ""));
            match fallback {
                Ok(value) => Ok(numeric_response(
                    original_expression,
                    Some(normalized.to_string()),
                    "evaluate",
                    value,
                    Vec::new(),
                    vec!["Used the legacy numeric evaluator because the natural-language evaluator rejected the expression.".to_string()],
                    request.precision,
                )),
                Err(legacy_error) => Err(format_evaluation_error(
                    normalized,
                    &format!(
                        "Natural-language/unit evaluation failed with: {fend_error}. Legacy numeric fallback also failed with: {legacy_error}"
                    ),
                )),
            }
        }
    }
}

fn simplify_expression(
    request: &CalculatorRequest,
    normalized: &str,
    original_expression: Option<String>,
) -> Result<CalculatorResponse, String> {
    simplify_with_expression(
        request,
        normalized,
        original_expression,
        Some(normalized.to_string()),
    )
}

fn simplify_with_expression(
    request: &CalculatorRequest,
    expression: &str,
    original_expression: Option<String>,
    normalized_override: Option<String>,
) -> Result<CalculatorResponse, String> {
    let parsed = parse_expression(expression)
        .map_err(|errors| format_parse_errors(expression, &errors, None))?;
    let simplified = parsed.simplify();
    Ok(symbolic_response(
        original_expression,
        normalized_override.or_else(|| Some(expression.to_string())),
        "simplify",
        &simplified,
        approximate_expression(&simplified, &request.variables, request.precision),
        None,
        symbolic_steps(
            request.steps_mode,
            "Applied algebraic simplification rules.",
            &format!("Result: {}", simplified),
        ),
        Vec::new(),
    ))
}

fn differentiate_expression(
    request: &CalculatorRequest,
    normalized: &str,
    original_expression: Option<String>,
) -> Result<CalculatorResponse, String> {
    let variable = request.variable.as_deref().ok_or_else(|| {
        "Differentiate mode requires 'variable'. Example: {\"mode\":\"differentiate\",\"expression\":\"x^3 + sin(x)\",\"variable\":\"x\"}".to_string()
    })?;
    differentiate_from_parts(
        request,
        normalized,
        variable,
        None,
        original_expression,
        Some(normalized.to_string()),
    )
}

fn differentiate_from_parts(
    request: &CalculatorRequest,
    expression: &str,
    variable: &str,
    order: Option<&str>,
    original_expression: Option<String>,
    normalized_override: Option<String>,
) -> Result<CalculatorResponse, String> {
    let parsed = parse_expression(expression)
        .map_err(|errors| format_parse_errors(expression, &errors, None))?;
    let order = order
        .map(|value| {
            value.parse::<usize>().map_err(|_| {
                format!("Derivative order must be a positive integer. Received '{value}'.")
            })
        })
        .transpose()?
        .unwrap_or(1);
    if order == 0 {
        return Err("Derivative order must be at least 1.".to_string());
    }

    let mut result = parsed;
    for _ in 0..order {
        result = result.differentiate(variable).simplify();
    }

    let steps = match request.steps_mode {
        StepsMode::None => Vec::new(),
        StepsMode::Summary => vec![
            format!("Differentiated with respect to {variable}."),
            format!("Simplified the derivative to {result}."),
        ],
        StepsMode::Detailed => vec![
            format!("Parsed the input expression: {expression}."),
            format!("Applied symbolic differentiation {order} time(s) with respect to {variable}."),
            format!("Simplified the derivative to {result}."),
        ],
    };

    Ok(symbolic_response(
        original_expression,
        normalized_override.or_else(|| Some(expression.to_string())),
        "differentiate",
        &result,
        approximate_expression(&result, &request.variables, request.precision),
        None,
        steps,
        Vec::new(),
    ))
}

fn integrate_expression(
    request: &CalculatorRequest,
    normalized: &str,
    original_expression: Option<String>,
) -> Result<CalculatorResponse, String> {
    let variable = request.variable.as_deref().ok_or_else(|| {
        "Integrate mode requires 'variable'. Example: {\"mode\":\"integrate\",\"expression\":\"x^2\",\"variable\":\"x\",\"lowerBound\":\"0\",\"upperBound\":\"1\"}".to_string()
    })?;
    integrate_from_parts(
        request,
        normalized,
        variable,
        request.lower_bound.as_deref(),
        request.upper_bound.as_deref(),
        original_expression,
        Some(normalized.to_string()),
    )
}

fn integrate_from_parts(
    request: &CalculatorRequest,
    expression: &str,
    variable: &str,
    lower: Option<&str>,
    upper: Option<&str>,
    original_expression: Option<String>,
    normalized_override: Option<String>,
) -> Result<CalculatorResponse, String> {
    let parsed = parse_expression(expression)
        .map_err(|errors| format_parse_errors(expression, &errors, None))?;
    let (result, mode, steps) = if let (Some(lower), Some(upper)) = (lower, upper) {
        let lower_expr = parse_expression(lower)
            .map_err(|errors| format_parse_errors(lower, &errors, Some("lower bound")))?;
        let upper_expr = parse_expression(upper)
            .map_err(|errors| format_parse_errors(upper, &errors, Some("upper bound")))?;
        let result = definite_integral(&parsed, variable, &lower_expr, &upper_expr)
            .map_err(|err| {
                format!("Definite integral is not supported for this expression: {err:?}")
            })?
            .simplify();
        let steps = match request.steps_mode {
            StepsMode::None => Vec::new(),
            StepsMode::Summary => vec![
                format!("Integrated with respect to {variable}."),
                format!("Evaluated the antiderivative between {lower} and {upper}."),
                format!("Result: {result}."),
            ],
            StepsMode::Detailed => vec![
                format!("Parsed the integrand: {expression}."),
                format!("Computed an antiderivative with respect to {variable}."),
                format!("Substituted the upper bound {upper} and lower bound {lower}."),
                format!("Simplified the definite integral to {result}."),
            ],
        };
        (result, "integrate", steps)
    } else {
        let result = integrate(&parsed, variable)
            .map_err(|err| {
                format!("Symbolic integration is not supported for this expression: {err:?}")
            })?
            .simplify();
        let steps = match request.steps_mode {
            StepsMode::None => Vec::new(),
            StepsMode::Summary => vec![
                format!("Integrated with respect to {variable}."),
                format!("Result: {result}."),
            ],
            StepsMode::Detailed => vec![
                format!("Parsed the integrand: {expression}."),
                format!("Applied symbolic integration rules with respect to {variable}."),
                format!("Simplified the antiderivative to {result}."),
            ],
        };
        (result, "integrate", steps)
    };

    Ok(symbolic_response(
        original_expression,
        normalized_override.or_else(|| Some(expression.to_string())),
        mode,
        &result,
        approximate_expression(&result, &request.variables, request.precision),
        None,
        steps,
        Vec::new(),
    ))
}

fn solve_equation(
    request: &CalculatorRequest,
    normalized: &str,
    original_expression: Option<String>,
) -> Result<CalculatorResponse, String> {
    let variable = if let Some(variable) = request.variable.as_deref() {
        variable.to_string()
    } else {
        infer_equation_variable(normalized)?
    };
    solve_equation_from_parts(
        request,
        normalized,
        &variable,
        original_expression,
        Some(normalized.to_string()),
    )
}

fn solve_equation_from_parts(
    request: &CalculatorRequest,
    equation: &str,
    variable: &str,
    original_expression: Option<String>,
    normalized_override: Option<String>,
) -> Result<CalculatorResponse, String> {
    let parsed =
        parse_equation(equation).map_err(|errors| format_parse_errors(equation, &errors, None))?;
    let solver = SmartSolver::new();
    let (solution, path) = solver
        .solve(&parsed, &Variable::new(variable))
        .map_err(|err| format!("Could not solve the equation for {variable}: {err}"))?;

    let result_value = solution_to_value(&solution);
    let exact_value = Some(solution_to_string(&solution));
    let approximate_value =
        solution_to_approximation(&solution, &request.variables, request.precision);
    let used_approximation = approximate_value.is_some();
    let steps = render_resolution_steps(request.steps_mode, &path.steps, &path.result);

    Ok(CalculatorResponse {
        expression: original_expression,
        normalized_expression: normalized_override.or_else(|| Some(equation.to_string())),
        mode: "solve".to_string(),
        result: result_value,
        exact_value,
        approximate_value,
        unit: None,
        steps,
        warnings: Vec::new(),
        used_approximation,
        capabilities: None,
    })
}

fn solve_equation_system(request: &CalculatorRequest) -> Result<CalculatorResponse, String> {
    if request.equations.len() < 2 {
        return Err(
            "System solving requires at least two equations. Provide them via 'equations', for example: {\"mode\":\"solve_system\",\"equations\":[\"x+y=3\",\"x-y=1\"]}"
                .to_string(),
        );
    }

    let mut parsed_equations = Vec::with_capacity(request.equations.len());
    let mut variable_names = HashSet::new();
    for equation in &request.equations {
        let parsed = parse_equation(equation)
            .map_err(|errors| format_parse_errors(equation, &errors, None))?;
        variable_names.extend(parsed.left.variables());
        variable_names.extend(parsed.right.variables());
        parsed_equations.push(parsed);
    }

    let variables = variable_names
        .iter()
        .map(|name| Variable::new(name))
        .collect::<Vec<_>>();
    let solver = SystemSolver::new();
    let solutions = solver
        .solve_system(&parsed_equations, &variables)
        .map_err(|err| format!("Could not solve the equation system: {err}"))?;

    let mut result_map = Map::new();
    let mut approx_map = Map::new();
    let mut exact_parts = Vec::new();

    for variable in &variables {
        if let Some(solution) = solutions.get(variable) {
            result_map.insert(variable.name.clone(), solution_to_value(solution));
            if let Some(approx) =
                solution_to_approximation(solution, &request.variables, request.precision)
            {
                approx_map.insert(variable.name.clone(), approx);
            }
            exact_parts.push(format!(
                "{} = {}",
                variable.name,
                solution_to_string(solution)
            ));
        }
    }

    let steps = match request.steps_mode {
        StepsMode::None => Vec::new(),
        StepsMode::Summary => vec![
            format!(
                "Solved a linear system with {} equation(s).",
                request.equations.len()
            ),
            format!("Solutions: {}", exact_parts.join(", ")),
        ],
        StepsMode::Detailed => {
            let mut detailed = vec![format!(
                "Parsed {} equation(s) for system solving.",
                request.equations.len()
            )];
            detailed.extend(
                request
                    .equations
                    .iter()
                    .enumerate()
                    .map(|(index, equation)| format!("Equation {}: {}", index + 1, equation)),
            );
            detailed.push("Applied Gaussian-elimination based system solving.".to_string());
            detailed.push(format!("Solutions: {}", exact_parts.join(", ")));
            detailed
        }
    };

    let approximate_value = if approx_map.is_empty() {
        None
    } else {
        Some(Value::Object(approx_map))
    };

    Ok(CalculatorResponse {
        expression: request.expression.clone(),
        normalized_expression: Some(request.equations.join("; ")),
        mode: "solve_system".to_string(),
        result: Value::Object(result_map),
        exact_value: Some(exact_parts.join(", ")),
        approximate_value,
        unit: None,
        steps,
        warnings: Vec::new(),
        used_approximation: false,
        capabilities: None,
    })
}

fn limit_expression(
    request: &CalculatorRequest,
    normalized: &str,
    original_expression: Option<String>,
) -> Result<CalculatorResponse, String> {
    let variable = request.variable.as_deref().ok_or_else(|| {
        "Limit mode requires 'variable'. Example: {\"mode\":\"limit\",\"expression\":\"sin(x)/x\",\"variable\":\"x\",\"lowerBound\":\"0\"}".to_string()
    })?;
    let target = request
        .lower_bound
        .as_deref()
        .or(request.upper_bound.as_deref())
        .ok_or_else(|| {
            "Limit mode requires a target value. Use 'lowerBound' or pass limit(expr, variable, target)."
                .to_string()
        })?;
    limit_from_parts(
        request,
        normalized,
        variable,
        target,
        original_expression,
        Some(normalized.to_string()),
    )
}

fn limit_from_parts(
    request: &CalculatorRequest,
    expression: &str,
    variable: &str,
    target: &str,
    original_expression: Option<String>,
    normalized_override: Option<String>,
) -> Result<CalculatorResponse, String> {
    let parsed = parse_expression(expression)
        .map_err(|errors| format_parse_errors(expression, &errors, None))?;
    let limit_point = parse_limit_target(target)?;
    let result = limit(&parsed, variable, limit_point)
        .map_err(|err| format!("Could not evaluate the limit: {err:?}"))?;

    let (result_value, exact_value, approximate_value) =
        limit_result_to_payload(&result, request.precision);
    let steps = match request.steps_mode {
        StepsMode::None => Vec::new(),
        StepsMode::Summary => vec![
            format!("Evaluated the limit as {variable} approaches {target}."),
            format!("Result: {exact_value}."),
        ],
        StepsMode::Detailed => vec![
            format!("Parsed the expression: {expression}."),
            format!("Evaluated the limit as {variable} approaches {target}."),
            format!("Result: {exact_value}."),
        ],
    };

    Ok(CalculatorResponse {
        expression: original_expression,
        normalized_expression: normalized_override.or_else(|| Some(expression.to_string())),
        mode: "limit".to_string(),
        result: result_value,
        exact_value: Some(exact_value),
        approximate_value,
        unit: None,
        steps,
        warnings: Vec::new(),
        used_approximation: false,
        capabilities: None,
    })
}

fn capabilities_response(expression: Option<String>) -> CalculatorResponse {
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

fn calculator_capabilities() -> Value {
    json!({
        "version": 2,
        "modes": [
            "auto",
            "capabilities",
            "evaluate",
            "simplify",
            "differentiate",
            "integrate",
            "solve",
            "solve_system",
            "limit"
        ],
        "supports": {
            "symbolicMath": true,
            "equationSolving": true,
            "equationSystems": true,
            "units": true,
            "complexNumbers": true,
            "structuredReturn": true,
            "stepControl": true,
            "precisionControl": true,
            "recoverableErrors": true,
            "matrices": false,
            "statistics": "legacy numeric helper functions only"
        },
        "syntax": {
            "functionCalls": [
                {
                    "name": "diff(expr, variable[, order])",
                    "example": "diff(x^3 + sin(x), x)"
                },
                {
                    "name": "integral(expr, variable[, lower, upper])",
                    "example": "integral(x^2, x, 0, 1)"
                },
                {
                    "name": "solve(equation, variable)",
                    "example": "solve(x^2 - 5x + 6 = 0, x)"
                },
                {
                    "name": "solve_system(eq1, eq2, ...)",
                    "example": "solve_system(x+y=3, x-y=1)"
                },
                {
                    "name": "limit(expr, variable, target)",
                    "example": "limit(sin(x)/x, x, 0)"
                },
                {
                    "name": "simplify(expr)",
                    "example": "simplify(2x + 3x)"
                }
            ],
            "naturalForms": [
                "sin pi/2",
                "2x + 3x",
                "3 km + 500 m",
                "60 km/h * 2 h",
                "∫_0^1 x^2 dx"
            ],
            "variables": "Pass variable bindings with the optional 'variables' object, for example {\"x\": 2.5}."
        },
        "functions": {
            "symbolic": [
                "sin",
                "cos",
                "tan",
                "asin",
                "acos",
                "atan",
                "exp",
                "ln",
                "log",
                "sqrt",
                "abs",
                "min",
                "max"
            ],
            "legacyNumeric": [
                "gamma",
                "ln_gamma",
                "digamma",
                "erf",
                "erfc",
                "erf_inv",
                "erfc_inv",
                "beta",
                "ln_beta",
                "factorial",
                "ln_factorial",
                "ncr",
                "npr",
                "logistic",
                "logit",
                "harmonic",
                "gen_harmonic",
                "sum",
                "mean",
                "product"
            ]
        },
        "resourceLimits": {
            "maxExpressionLength": MAX_EXPRESSION_LENGTH,
            "maxEquationCount": MAX_EQUATION_COUNT,
            "maxPrecision": MAX_PRECISION,
            "fendTimeoutMs": FEND_TIMEOUT_MS
        },
        "examples": [
            {
                "input": { "expression": "2 * (3.5 + 4.5) / sqrt(16)" },
                "summary": "numeric evaluation"
            },
            {
                "input": { "expression": "diff(x^3 + sin(x), x)", "steps": "summary" },
                "summary": "symbolic differentiation"
            },
            {
                "input": { "expression": "3 km + 500 m" },
                "summary": "unit-aware arithmetic"
            },
            {
                "input": { "mode": "solve_system", "equations": ["x+y=3", "x-y=1"] },
                "summary": "linear system solving"
            }
        ]
    })
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

fn evaluate_with_fend(expression: &str, precision: u32) -> Result<FendPayload, String> {
    let mut context = FendContext::new();
    let interrupt = DeadlineInterrupt::new(Duration::from_millis(FEND_TIMEOUT_MS));
    let result = evaluate_with_interrupt(expression, &mut context, &interrupt)
        .map_err(|err| err.to_string())?;
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

    let (approx_numeric, unit) = split_numeric_result(&rendered);
    let unit = unit.or_else(|| extract_unit_hint(&rendered));
    let approximate_value =
        approx_numeric.map(|value| json!(round_with_precision(value, precision)));
    let result_value = if unit.is_some() {
        json!(rendered)
    } else if let Some(value) = approx_numeric {
        json!(round_with_precision(value, precision))
    } else {
        json!(rendered)
    };

    let used_approximation = approximate_value.is_some();

    Ok(FendPayload {
        result: result_value,
        exact_value: Some(rendered),
        approximate_value,
        unit,
        warnings,
        used_approximation,
    })
}

fn numeric_response(
    expression: Option<String>,
    normalized_expression: Option<String>,
    mode: &str,
    value: f64,
    steps: Vec<String>,
    warnings: Vec<String>,
    precision: u32,
) -> CalculatorResponse {
    let rounded = round_with_precision(value, precision);
    CalculatorResponse {
        expression,
        normalized_expression,
        mode: mode.to_string(),
        result: json!(rounded),
        exact_value: Some(format_decimal(rounded, precision)),
        approximate_value: Some(json!(rounded)),
        unit: None,
        steps,
        warnings,
        used_approximation: true,
        capabilities: None,
    }
}

fn symbolic_response(
    expression: Option<String>,
    normalized_expression: Option<String>,
    mode: &str,
    result: &Expression,
    approximate: Option<Value>,
    unit: Option<String>,
    steps: Vec<String>,
    warnings: Vec<String>,
) -> CalculatorResponse {
    let used_approximation = approximate.is_some();
    CalculatorResponse {
        expression,
        normalized_expression,
        mode: mode.to_string(),
        result: json!(format!("{result}")),
        exact_value: Some(format!("{result}")),
        approximate_value: approximate,
        unit,
        steps,
        warnings,
        used_approximation,
        capabilities: None,
    }
}

fn render_resolution_steps(
    steps_mode: StepsMode,
    steps: &[ResolutionStep],
    final_result: &Expression,
) -> Vec<String> {
    match steps_mode {
        StepsMode::None => Vec::new(),
        StepsMode::Summary => {
            let mut rendered = steps
                .iter()
                .take(MAX_STEP_COUNT)
                .map(|step| step.operation.describe())
                .collect::<Vec<_>>();
            rendered.push(format!("Final result: {final_result}"));
            rendered
        }
        StepsMode::Detailed => {
            let mut rendered = Vec::with_capacity(steps.len().min(MAX_STEP_COUNT) + 1);
            for step in steps.iter().take(MAX_STEP_COUNT) {
                rendered.push(format!(
                    "{} => {} ({})",
                    step.operation.describe(),
                    step.result,
                    step.explanation
                ));
            }
            rendered.push(format!("Final result: {final_result}"));
            rendered
        }
    }
}

fn symbolic_steps(steps_mode: StepsMode, first: &str, second: &str) -> Vec<String> {
    match steps_mode {
        StepsMode::None => Vec::new(),
        StepsMode::Summary => vec![first.to_string(), second.to_string()],
        StepsMode::Detailed => vec![first.to_string(), second.to_string()],
    }
}

fn solution_to_value(solution: &Solution) -> Value {
    match solution {
        Solution::Unique(expression) => json!(format!("{expression}")),
        Solution::Multiple(expressions) => json!(expressions
            .iter()
            .map(|expr| format!("{expr}"))
            .collect::<Vec<_>>()),
        Solution::Parametric {
            expression,
            constraints,
        } => json!({
            "expression": format!("{expression}"),
            "constraints": constraints.iter().map(|constraint| {
                format!("{}: {}", constraint.variable.name, constraint.condition)
            }).collect::<Vec<_>>()
        }),
        Solution::None => json!("no solution"),
        Solution::Infinite => json!("infinite solutions"),
    }
}

fn solution_to_string(solution: &Solution) -> String {
    match solution {
        Solution::Unique(expression) => format!("{expression}"),
        Solution::Multiple(expressions) => expressions
            .iter()
            .map(|expr| format!("{expr}"))
            .collect::<Vec<_>>()
            .join(", "),
        Solution::Parametric {
            expression,
            constraints,
        } => {
            if constraints.is_empty() {
                format!("{expression}")
            } else {
                format!(
                    "{} with constraints [{}]",
                    expression,
                    constraints
                        .iter()
                        .map(|constraint| format!(
                            "{}: {}",
                            constraint.variable.name, constraint.condition
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        Solution::None => "no solution".to_string(),
        Solution::Infinite => "infinite solutions".to_string(),
    }
}

fn solution_to_approximation(
    solution: &Solution,
    bindings: &HashMap<String, f64>,
    precision: u32,
) -> Option<Value> {
    match solution {
        Solution::Unique(expression) => {
            approximate_expression(expression, bindings, precision).map(|value| value)
        }
        Solution::Multiple(expressions) => {
            let approximations = expressions
                .iter()
                .filter_map(|expr| approximate_expression(expr, bindings, precision))
                .collect::<Vec<_>>();
            if approximations.is_empty() {
                None
            } else {
                Some(Value::Array(approximations))
            }
        }
        Solution::Parametric { expression, .. } => {
            approximate_expression(expression, bindings, precision).map(|value| value)
        }
        Solution::None | Solution::Infinite => None,
    }
}

fn approximate_expression(
    expression: &Expression,
    bindings: &HashMap<String, f64>,
    precision: u32,
) -> Option<Value> {
    expression
        .evaluate(bindings)
        .map(|value| json!(round_with_precision(value, precision)))
}

fn limit_result_to_payload(result: &LimitResult, precision: u32) -> (Value, String, Option<Value>) {
    match result {
        LimitResult::Value(value) => {
            let rounded = round_with_precision(*value, precision);
            (
                json!(rounded),
                format_decimal(rounded, precision),
                Some(json!(rounded)),
            )
        }
        LimitResult::PositiveInfinity => (json!("∞"), "∞".to_string(), None),
        LimitResult::NegativeInfinity => (json!("-∞"), "-∞".to_string(), None),
        LimitResult::Expression(expression) => (
            json!(format!("{expression}")),
            format!("{expression}"),
            None,
        ),
    }
}

fn parse_limit_target(target: &str) -> Result<LimitPoint, String> {
    match target.trim().to_ascii_lowercase().as_str() {
        "inf" | "+inf" | "infinity" | "+infinity" => Ok(LimitPoint::PositiveInfinity),
        "-inf" | "-infinity" => Ok(LimitPoint::NegativeInfinity),
        _ => target
            .trim()
            .parse::<f64>()
            .map(LimitPoint::Value)
            .map_err(|_| {
                format!(
                    "Limit target '{target}' is not supported. Use a finite number, 'inf', or '-inf'."
                )
            }),
    }
}

fn infer_equation_variable(equation: &str) -> Result<String, String> {
    let parsed =
        parse_equation(equation).map_err(|errors| format_parse_errors(equation, &errors, None))?;
    let mut variables = parsed.left.variables();
    variables.extend(parsed.right.variables());
    if variables.len() == 1 {
        Ok(variables.into_iter().next().unwrap_or_default())
    } else {
        Err(format!(
            "Could not infer a single target variable from '{equation}'. Please pass 'variable', for example solve({equation}, x)."
        ))
    }
}

fn normalize_expression(expression: &str) -> Result<String, String> {
    let mut normalized = expression.trim().to_string();
    normalized = normalized
        .replace('π', "pi")
        .replace('×', "*")
        .replace('÷', "/")
        .replace('−', "-")
        .replace('·', "*");
    ensure_balanced_grouping(&normalized)?;

    if let Some((integrand, variable, lower, upper)) = parse_unicode_integral(&normalized) {
        return Ok(match (lower, upper) {
            (Some(lower), Some(upper)) => {
                format!("integral({integrand}, {variable}, {lower}, {upper})")
            }
            _ => format!("integral({integrand}, {variable})"),
        });
    }

    if let Some(adapted) = wrap_simple_function_argument(&normalized) {
        return Ok(adapted);
    }

    Ok(normalized)
}

fn wrap_simple_function_argument(expression: &str) -> Option<String> {
    static SIMPLE_FUNCTIONS: OnceLock<Vec<&'static str>> = OnceLock::new();
    let functions = SIMPLE_FUNCTIONS.get_or_init(|| {
        vec![
            "sin", "cos", "tan", "asin", "acos", "atan", "sqrt", "ln", "exp", "abs",
        ]
    });

    for function in functions {
        let prefix = format!("{function} ");
        if let Some(argument) = expression.strip_prefix(&prefix) {
            if !argument.starts_with('(') {
                return Some(format!("{function}({argument})"));
            }
        }
    }
    None
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

fn parse_unicode_integral(
    expression: &str,
) -> Option<(String, String, Option<String>, Option<String>)> {
    let trimmed = expression.trim();
    if !trimmed.starts_with('∫') {
        return None;
    }

    let rest = trimmed.strip_prefix('∫')?;
    let (lower, upper, body) = if let Some(bounds) = rest.strip_prefix('_') {
        let caret_index = bounds.find('^')?;
        let lower = bounds[..caret_index].trim().to_string();
        let after_caret = &bounds[caret_index + 1..];
        let mut split_index = None;
        for (index, ch) in after_caret.char_indices() {
            if ch.is_whitespace() {
                split_index = Some(index);
                break;
            }
        }
        let split_index = split_index?;
        (
            Some(lower),
            Some(after_caret[..split_index].trim().to_string()),
            after_caret[split_index..].trim().to_string(),
        )
    } else {
        (None, None, rest.trim().to_string())
    };

    let d_index = body.rfind(" d").or_else(|| body.rfind('d'))?;
    let (integrand, variable) = if let Some(variable) = body.strip_prefix("d") {
        return Some((String::new(), variable.trim().to_string(), lower, upper));
    } else {
        let integrand = body[..d_index].trim().to_string();
        let variable = body[d_index..]
            .trim_start_matches(' ')
            .trim_start_matches('d')
            .trim()
            .to_string();
        (integrand, variable)
    };

    if integrand.is_empty() || variable.is_empty() {
        return None;
    }

    Some((integrand, variable, lower, upper))
}

fn required_arg<'a>(args: &'a [String], index: usize, syntax: &str) -> Result<&'a str, String> {
    args.get(index)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("Missing argument {}. Expected syntax: {syntax}", index + 1))
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

fn format_parse_errors(
    expression: &str,
    errors: &[ParseError],
    context_label: Option<&str>,
) -> String {
    let prefix = context_label
        .map(|label| format!("Failed to parse the {label}."))
        .unwrap_or_else(|| "Failed to parse the expression.".to_string());
    let rendered = errors
        .iter()
        .map(|error| format_single_parse_error(expression, error))
        .collect::<Vec<_>>()
        .join(" ");
    format!("{prefix} {rendered}")
}

fn format_single_parse_error(expression: &str, error: &ParseError) -> String {
    let (position, reason, suggestion) = match error {
        ParseError::UnexpectedCharacter { pos, found } => (
            *pos,
            format!("Unexpected character '{found}'"),
            Some("Check for unsupported punctuation or a missing operator such as '*'."),
        ),
        ParseError::UnexpectedEndOfInput { pos, expected } => (
            *pos,
            format!("The expression ended before {expected}"),
            Some("Look for an unclosed parenthesis or a missing argument."),
        ),
        ParseError::InvalidNumber { pos, text } => (
            *pos,
            format!("Invalid number '{text}'"),
            Some("Use a standard decimal or scientific-notation literal."),
        ),
        ParseError::UnknownFunction { pos, name } => (
            *pos,
            format!("Unknown function '{name}'"),
            closest_function_suggestion(name),
        ),
        ParseError::MismatchedParentheses { pos } => (
            *pos,
            "Parentheses do not match".to_string(),
            Some("Make sure each opening '(' has a matching closing ')'"),
        ),
        ParseError::InvalidExpression { pos, message } => (*pos, message.clone(), None),
        _ => (
            0,
            error.to_string(),
            Some("Try a simpler form, or call capabilities() to inspect supported syntax."),
        ),
    };

    let snippet = position_snippet(expression, position);
    if let Some(suggestion) = suggestion {
        format!("{reason} at position {position}. {snippet} {suggestion}")
    } else {
        format!("{reason} at position {position}. {snippet}")
    }
}

fn position_snippet(expression: &str, position: usize) -> String {
    let start = position.saturating_sub(12);
    let end = (position + 12).min(expression.len());
    let snippet = expression.get(start..end).unwrap_or(expression);
    let caret_offset = position.saturating_sub(start);
    format!(
        "Near `{snippet}`.\n{}\n{}^",
        snippet,
        " ".repeat(caret_offset)
    )
}

fn closest_function_suggestion(name: &str) -> Option<&'static str> {
    static KNOWN_FUNCTIONS: OnceLock<Vec<&'static str>> = OnceLock::new();
    let functions = KNOWN_FUNCTIONS.get_or_init(|| {
        vec![
            "sin", "cos", "tan", "asin", "acos", "atan", "exp", "ln", "log", "sqrt", "abs", "min",
            "max", "gamma", "beta", "mean", "sum", "product",
        ]
    });

    functions
        .iter()
        .copied()
        .min_by_key(|candidate| levenshtein(name, candidate))
        .and_then(|candidate| {
            let distance = levenshtein(name, candidate);
            if distance <= 3 {
                Some(candidate)
            } else {
                None
            }
        })
}

fn levenshtein(left: &str, right: &str) -> usize {
    if left == right {
        return 0;
    }
    if left.is_empty() {
        return right.chars().count();
    }
    if right.is_empty() {
        return left.chars().count();
    }

    let right_chars = right.chars().collect::<Vec<_>>();
    let mut costs = (0..=right_chars.len()).collect::<Vec<_>>();

    for (i, left_ch) in left.chars().enumerate() {
        let mut previous_diagonal = i;
        costs[0] = i + 1;
        for (j, right_ch) in right_chars.iter().enumerate() {
            let temp = costs[j + 1];
            let substitution = previous_diagonal + usize::from(left_ch != *right_ch);
            let insertion = costs[j + 1] + 1;
            let deletion = costs[j] + 1;
            costs[j + 1] = substitution.min(insertion.min(deletion));
            previous_diagonal = temp;
        }
    }

    *costs.last().unwrap_or(&0)
}

fn format_evaluation_error(expression: &str, reason: &str) -> String {
    format!(
        "Calculator could not evaluate `{expression}`. {reason}. If you meant symbolic algebra, try simplify(...), diff(...), integral(...), solve(...), or pass variable bindings through the 'variables' object."
    )
}

fn split_numeric_result(rendered: &str) -> (Option<f64>, Option<String>) {
    if let Some((number, unit)) = rendered.split_once(' ') {
        if let Ok(value) = number.parse::<f64>() {
            return (Some(value), Some(unit.trim().to_string()));
        }
        if !unit.trim().is_empty() {
            return (None, Some(unit.trim().to_string()));
        }
    }

    let numeric_prefix_len = rendered
        .char_indices()
        .take_while(|(index, ch)| {
            ch.is_ascii_digit()
                || matches!(ch, '.' | '-' | '+')
                || (*ch == 'e' || *ch == 'E')
                || (*ch == '∞' && *index == 0)
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

    if let Ok(value) = rendered.parse::<f64>() {
        return (Some(value), None);
    }

    if let Some(index) = rendered.char_indices().find_map(|(index, ch)| {
        (index > 0 && (ch.is_ascii_alphabetic() || matches!(ch, '°' | '%' | 'Ω'))).then_some(index)
    }) {
        let unit = rendered[index..].trim();
        if !unit.is_empty() {
            return (None, Some(unit.to_string()));
        }
    }

    (None, None)
}

fn extract_unit_hint(rendered: &str) -> Option<String> {
    let trimmed = rendered.trim();
    let mut unit_start = trimmed.len();
    let mut saw_unit_char = false;

    for (index, ch) in trimmed.char_indices().rev() {
        let is_unit_char = ch.is_ascii_alphabetic() || matches!(ch, '/' | '°' | '%' | '^' | 'Ω');
        if is_unit_char {
            unit_start = index;
            saw_unit_char = true;
            continue;
        }
        if saw_unit_char {
            break;
        }
    }

    if !saw_unit_char {
        return None;
    }

    let unit = trimmed[unit_start..].trim();
    if unit.is_empty() || unit == "i" {
        None
    } else {
        Some(unit.to_string())
    }
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

    (cleaned, warnings)
}

fn round_with_precision(value: f64, precision: u32) -> f64 {
    let factor = 10_f64.powi(precision.min(12) as i32);
    (value * factor).round() / factor
}

fn format_decimal(value: f64, precision: u32) -> String {
    let precision = precision.min(12) as usize;
    let rendered = format!("{value:.precision$}");
    rendered
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn is_capability_query(expression: &str) -> bool {
    matches!(
        expression.trim().to_ascii_lowercase().as_str(),
        "capabilities()" | "capabilities" | "help()" | "help"
    )
}

fn is_legacy_numeric_function(name: &str) -> bool {
    matches!(
        name,
        "gamma"
            | "ln_gamma"
            | "digamma"
            | "erf"
            | "erfc"
            | "erf_inv"
            | "erfc_inv"
            | "beta"
            | "ln_beta"
            | "factorial"
            | "ln_factorial"
            | "ncr"
            | "npr"
            | "logistic"
            | "logit"
            | "harmonic"
            | "gen_harmonic"
            | "sum"
            | "mean"
            | "product"
    )
}

fn is_passthrough_expression_function(name: &str) -> bool {
    matches!(
        name,
        "sin"
            | "cos"
            | "tan"
            | "asin"
            | "acos"
            | "atan"
            | "atan2"
            | "sinh"
            | "cosh"
            | "tanh"
            | "exp"
            | "ln"
            | "log"
            | "log2"
            | "log10"
            | "sqrt"
            | "cbrt"
            | "pow"
            | "floor"
            | "ceil"
            | "round"
            | "abs"
            | "sign"
            | "min"
            | "max"
    )
}
