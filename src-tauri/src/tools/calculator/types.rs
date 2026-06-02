use serde_json::{Value, json};
use crate::agentjax_err;
use crate::error::AgentJaxResult;

pub(crate) const DEFAULT_PRECISION: u32 = 12;
pub(crate) const MAX_PRECISION: u32 = 32;

/// Bound single-expression input size so fend-core evaluations stay responsive.
pub(crate) const MAX_EXPRESSION_LENGTH: usize = 512;

/// Hard timeout for fend-core interrupt checks to prevent runaway evaluations.
pub(crate) const FEND_TIMEOUT_MS: u64 = 200;

/// Execution mode for the calculator tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CalculatorMode {
    Auto,
    Capabilities,
    Evaluate,
}

impl CalculatorMode {
    pub(crate) fn parse(value: Option<&str>) -> AgentJaxResult<Self> {
        match value.unwrap_or("auto") {
            "auto" => Ok(Self::Auto),
            "capabilities" => Ok(Self::Capabilities),
            "evaluate" => Ok(Self::Evaluate),
            other => Err(agentjax_err!(
                format!("Unsupported calculator mode '{other}'. Try one of: auto, capabilities, evaluate."),
                ToolExecution
            )),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Capabilities => "capabilities",
            Self::Evaluate => "evaluate",
        }
    }
}

/// Normalized request payload consumed by the fend-core execution pipeline.
#[derive(Debug)]
pub(crate) struct CalculatorRequest {
    pub(crate) expression: Option<String>,
    pub(crate) mode: CalculatorMode,
    pub(crate) precision: u32,
    pub(crate) variables: Vec<CalculatorVariableBinding>,
}

/// A validated variable binding that can be translated into native fend
/// assignment syntax before evaluation.
#[derive(Debug)]
pub(crate) struct CalculatorVariableBinding {
    pub(crate) name: String,
    pub(crate) expression: String,
}

/// Stable structured response for tool callers.
#[derive(Debug)]
pub(crate) struct CalculatorResponse {
    pub(crate) expression: Option<String>,
    pub(crate) normalized_expression: Option<String>,
    pub(crate) mode: String,
    pub(crate) result: Value,
    pub(crate) exact_value: Option<String>,
    pub(crate) approximate_value: Option<Value>,
    pub(crate) unit: Option<String>,
    pub(crate) steps: Vec<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) used_approximation: bool,
    pub(crate) capabilities: Option<Value>,
}

impl CalculatorResponse {
    pub(crate) fn into_value(self) -> Value {
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
