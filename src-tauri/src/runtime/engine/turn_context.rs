//! Turn context — bundles the ~16 parameters previously passed individually
//! to `run_turn()` into a single struct. This makes the orchestration layer
//! readable and allows extracted helper functions to receive a single context
//! reference instead of a long argument list.

use crate::commands::chat::ChatRequest;
use crate::config::{AgentConfig, AppConfig};

use crate::tools::ToolCatalog;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::watch;

/// Bundles all per-turn configuration and state that the hop loop needs.
pub(crate) struct TurnContext<'a> {
    pub config: &'a AppConfig,
    pub agent: &'a AgentConfig,
    pub agent_id: &'a str,
    pub req: &'a ChatRequest,
    pub conversation_id: &'a str,
    pub user_message_ts: i64,
    pub provider_kind: String,
    pub resolved_model: crate::config::ResolvedModelConfig,
    pub provider_capabilities: crate::provider_api::ProviderCapabilities,
    pub tool_schema_format: crate::tools::ToolSchemaFormat,
    pub tools_catalog: &'a Arc<ToolCatalog>,
    pub cancel_rx: &'a mut watch::Receiver<bool>,
    pub sub_agent_event_tx: Option<
        tokio::sync::mpsc::UnboundedSender<crate::sub_agents::SubAgentEvent>,
    >,
    pub is_sub_agent: bool,
    pub sub_agent_type: Option<String>,
    pub is_memory_sub_agent: bool,
    pub is_auto_resume: bool,
    pub max_turns: usize,
    pub system_items: Vec<Value>,
    pub recovery_note: Option<Value>,
    pub street_items: Vec<Value>,
}

/// Tool execution context derived from TurnContext.
pub(crate) fn build_tool_context(ctx: &TurnContext<'_>) -> crate::tools::ToolExecutionContext {
    let sub_agent_id = if ctx.is_sub_agent {
        ctx.conversation_id.rsplit('/').next().map(|s| s.to_string())
    } else {
        None
    };
    crate::tools::ToolExecutionContext {
        conversation_id: Some(ctx.conversation_id.to_string()),
        agent_id: Some(ctx.agent_id.to_string()),
        model_id: Some(ctx.resolved_model.model_id.clone()),
        app_config: Some(Arc::new(ctx.config.clone())),
        agent_config: Some(Arc::new(ctx.agent.clone())),
        tool_catalog: Some(Arc::clone(ctx.tools_catalog)),
        sub_agent_id,
        sub_agent_type: ctx.sub_agent_type.clone(),
        is_memory_sub_agent: ctx.is_memory_sub_agent,
        sub_agent_event_tx: ctx.sub_agent_event_tx.clone(),
        ..Default::default()
    }
}
