use super::AgentRuntime;
use crate::config::AppConfig;
use crate::providers::types::{ProviderStreamEvent, ResponseStreamResult};
use crate::tools::ToolCatalog;
use serde_json::Value;
use tokio::sync::watch;

impl AgentRuntime {
    pub async fn run_turn<F>(
        config: &AppConfig,
        req: &crate::commands::chat::ChatRequest,
        conversation_id: &str,
        context_items: Vec<Value>,
        tools_catalog: &ToolCatalog,
        cancel_rx: &mut watch::Receiver<bool>,
        on_event: F,
    ) -> Result<(ResponseStreamResult, Vec<Value>), String>
    where
        F: FnMut(ProviderStreamEvent) -> Result<(), String> + Send + 'static,
    {
        Self::run_turn_with_engine(
            config,
            req,
            conversation_id,
            context_items,
            tools_catalog,
            cancel_rx,
            on_event,
        )
        .await
    }
}
