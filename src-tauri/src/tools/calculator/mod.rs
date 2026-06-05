mod capabilities;
mod evaluator;
mod request;
mod types;

use crate::error::AgentJaxResult;
pub(crate) use request::parse_request;
use serde_json::Value;
use types::CalculatorMode;
pub(crate) use types::CalculatorRequest;

/// Execute calculator request using the new fend-core-only engine.
pub(crate) fn execute(request: CalculatorRequest) -> AgentJaxResult<Value> {
    if request.mode == CalculatorMode::Capabilities
        || request
            .expression
            .as_deref()
            .map(capabilities::is_capability_query)
            .unwrap_or(false)
    {
        return Ok(capabilities::capabilities_response(request.expression).into_value());
    }

    evaluator::execute(request).map(|response| response.into_value())
}
