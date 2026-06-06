use crate::conversation_store;
use crate::error::AgentJaxError;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalClientMetadataEnvelope {
    #[serde(default)]
    dynamic_tools: Vec<conversation_store::ConversationDynamicTool>,
}

/// Extract AgentJax-local client metadata extensions and return a sanitized
/// payload safe to forward upstream.
pub(super) fn split_local_client_metadata(
    client_metadata: Option<Value>,
) -> Result<
    (
        Option<Value>,
        Option<Vec<conversation_store::ConversationDynamicTool>>,
    ),
    AgentJaxError,
> {
    let Some(value) = client_metadata else {
        return Ok((None, None));
    };
    let Value::Object(mut metadata) = value else {
        return Ok((Some(value), None));
    };

    let Some(local_value) = metadata.remove("agentjax_local") else {
        return Ok((Some(Value::Object(metadata)), None));
    };

    let local: LocalClientMetadataEnvelope = serde_json::from_value(local_value)
        .map_err(|err| AgentJaxError::config(format!("Invalid agentjax_local client metadata: {err}")))?;
    validate_conversation_dynamic_tools(&local.dynamic_tools)?;
    let dynamic_tools = Some(local.dynamic_tools);

    let sanitized = if metadata.is_empty() {
        None
    } else {
        Some(Value::Object(metadata))
    };
    Ok((sanitized, dynamic_tools))
}

fn validate_dynamic_tool_name(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 64
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

/// Validate conversation-scoped dynamic tools before they are persisted.
///
/// Keeping the validation local and deterministic makes plugin-driven tool
/// registration easier to reason about and avoids storing malformed tool specs
/// that would later disappear from snapshots.
pub(super) fn validate_conversation_dynamic_tools(
    tools: &[conversation_store::ConversationDynamicTool],
) -> Result<(), AgentJaxError> {
    let mut seen_names = HashSet::new();
    for tool in tools {
        if !validate_dynamic_tool_name(&tool.name) {
            return Err(AgentJaxError::config(format!(
                "Dynamic tool name '{}' must match [A-Za-z0-9_-] and be at most 64 characters",
                tool.name
            )));
        }
        if !seen_names.insert(tool.name.clone()) {
            return Err(AgentJaxError::config(format!("Duplicate dynamic tool name '{}'", tool.name)));
        }
        if tool.description.trim().is_empty() {
            return Err(AgentJaxError::config(format!(
                "Dynamic tool '{}' must have a non-empty description",
                tool.name
            )));
        }
        if !tool.parameters.is_object() {
            return Err(AgentJaxError::config(format!(
                "Dynamic tool '{}' parameters must be a JSON object schema",
                tool.name
            )));
        }

        match &tool.binding {
            conversation_store::ConversationDynamicToolBinding::Native { tool: native_tool } => {
                if native_tool.trim().is_empty() {
                    return Err(AgentJaxError::config(format!(
                        "Dynamic tool '{}' has an empty native binding target",
                        tool.name
                    )));
                }
            }
            conversation_store::ConversationDynamicToolBinding::Mcp {
                server_id,
                tool: mcp_tool,
            } => {
                if server_id.trim().is_empty() || mcp_tool.trim().is_empty() {
                    return Err(AgentJaxError::config(format!(
                        "Dynamic tool '{}' must include non-empty MCP server_id and tool target",
                        tool.name
                    )));
                }
            }
        }
    }

    Ok(())
}
