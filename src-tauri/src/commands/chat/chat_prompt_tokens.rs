use crate::config;
use crate::conversation_store;
use crate::provider_api::get_tool_schema_format;
use crate::tools::{ToolCatalog, ToolCatalogSnapshot, ToolExecutionContext};

pub(super) fn resolve_prompt_counting_model(
    config: &config::AppConfig,
    model: Option<&str>,
) -> Option<crate::config::ResolvedModelConfig> {
    match config.resolve_model_profile(model) {
        Ok(resolved) => Some(resolved),
        Err(err) => {
            log::warn!(
                "Failed to resolve prompt counting model from {:?}: {}",
                model,
                err
            );
            match config.resolve_model_profile(None) {
                Ok(resolved) => Some(resolved),
                Err(err) => {
                    log::warn!("Failed to resolve fallback prompt counting model: {}", err);
                    None
                }
            }
        }
    }
}

async fn tool_snapshot_for_conversation(
    tools_catalog: &ToolCatalog,
    conversation_id: &str,
    provider_kind: &str,
) -> Result<ToolCatalogSnapshot, String> {
    let tool_context = ToolExecutionContext {
        conversation_id: Some(conversation_id.to_string()),
    };
    let tool_schema_format = get_tool_schema_format(provider_kind)?;
    let mounted_mcp_servers = tools_catalog.load_persisted_mounted_servers(&tool_context);
    Ok(tools_catalog
        .snapshot_with_format_and_mounted_servers(
            tool_schema_format,
            &tool_context,
            &mounted_mcp_servers,
        )
        .await)
}

/// Estimate prompt tokens when stored provider usage is not available yet.
pub(super) async fn load_conversation_prompt_token_count(
    conversation_id: &str,
    model: Option<&str>,
    mcp_manager: std::sync::Arc<crate::mcp::McpManager>,
) -> usize {
    let cfg = match config::load_config() {
        Ok(cfg) => cfg,
        Err(err) => {
            log::warn!("Failed to load config for token counting: {}", err);
            return 0;
        }
    };

    let Some(resolved_model) = resolve_prompt_counting_model(&cfg, model) else {
        return 0;
    };

    let tools_catalog = ToolCatalog::new_with_home_plugins(mcp_manager, &cfg);
    let tool_snapshot = match tool_snapshot_for_conversation(
        &tools_catalog,
        conversation_id,
        &resolved_model.provider.kind,
    )
    .await
    {
        Ok(snapshot) => snapshot,
        Err(err) => {
            log::warn!(
                "Failed to load tool snapshot for conversation '{}' while counting tokens: {}",
                conversation_id,
                err
            );
            return 0;
        }
    };

    let recovery_note = conversation_store::build_recovery_developer_note(conversation_id)
        .ok()
        .flatten();

    match conversation_store::load_context_for_request(conversation_id) {
        Ok(context) => {
            let archived_context_items =
                crate::runtime::tool_archiving::archive_unavailable_historical_tool_calls(
                    context.input_items,
                    tool_snapshot.active_tool_names(),
                );
            match conversation_store::count_conversation_prompt_tokens(
                &resolved_model.model_id,
                Some(&resolved_model.system_prompt),
                &resolved_model.prompt_assembly.developer_items,
                recovery_note.as_ref(),
                &archived_context_items,
                &[],
                tool_snapshot.schemas(),
            ) {
                Ok(usage) => usage.prompt_tokens,
                Err(err) => {
                    log::warn!(
                        "Failed to count prompt tokens for conversation '{}' with model '{}': {}",
                        conversation_id,
                        resolved_model.model_id,
                        err
                    );
                    0
                }
            }
        }
        Err(err) => {
            log::warn!(
                "Failed to load conversation '{}' for token counting: {}",
                conversation_id,
                err
            );
            0
        }
    }
}
