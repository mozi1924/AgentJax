#[cfg(test)]
mod tests {
    use crate::providers;
    use crate::providers::types::{ProviderStreamEvent, ResponseStreamRequest};
    use crate::tools::ToolSchemaFormat;
    use serde_json::json;
    use std::sync::Once;
    use std::time::Duration;
    use tokio::sync::watch;

    static RUSTLS_CRYPTO_PROVIDER: Once = Once::new();

    fn ensure_rustls_crypto_provider() {
        RUSTLS_CRYPTO_PROVIDER.call_once(|| {
            rustls::crypto::ring::default_provider()
                .install_default()
                .expect("Failed to install rustls ring crypto provider for provider smoke tests");
        });
    }

    #[test]
    fn test_provider_capability_lookup() {
        let openai_responses = providers::get_capabilities("openai-responses")
            .expect("openai-responses adapter should exist");
        assert!(!openai_responses.supports_stored_responses);

        let err = providers::get_capabilities("not-a-provider").unwrap_err();
        assert!(err.contains("Unsupported provider kind"));

        let old_codex = providers::get_capabilities("codex").unwrap_err();
        assert!(old_codex.contains("Unsupported provider kind"));
    }

    #[test]
    fn test_provider_tool_schema_format_lookup() {
        let openai_responses = providers::get_tool_schema_format("openai-responses")
            .expect("openai-responses adapter should exist");
        assert_eq!(openai_responses, ToolSchemaFormat::Responses);

        let chat_completions = providers::get_tool_schema_format("chat-completions")
            .expect("chat-completions adapter should exist");
        assert_eq!(chat_completions, ToolSchemaFormat::ChatCompletions);

        let gemini =
            providers::get_tool_schema_format("gemini").expect("gemini adapter should exist");
        assert_eq!(gemini, ToolSchemaFormat::Gemini);

        let anthropic =
            providers::get_tool_schema_format("anthropic").expect("anthropic adapter should exist");
        assert_eq!(anthropic, ToolSchemaFormat::Anthropic);
    }

    #[test]
    fn test_builtin_provider_plugins_seed_registry() {
        let definitions = providers::registry::builtin_provider_definitions();
        let kinds = definitions
            .iter()
            .map(|definition| definition.kind.as_str())
            .collect::<Vec<_>>();

        assert!(kinds.contains(&"openai-responses"));
        assert!(kinds.contains(&"chat-completions"));
        assert!(kinds.contains(&"gemini"));
        assert!(kinds.contains(&"anthropic"));
        assert_eq!(
            providers::registry::provider_transport_family("openai-responses"),
            Some(providers::registry::ProviderTransportFamily::Responses)
        );
    }

    #[test]
    fn test_extract_pending_tool_calls_is_provider_scoped() {
        let output_items = vec![
            json!({
                "type": "function_call",
                "call_id": "call_a",
                "name": "tool_a",
                "arguments": {"x": 1}
            }),
            json!({
                "type": "function_call_output",
                "call_id": "call_a",
                "output": "ok"
            }),
            json!({
                "type": "function_call",
                "call_id": "call_b",
                "name": "tool_b",
                "arguments": {"y": 2}
            }),
        ];

        let pending = providers::extract_pending_tool_calls("openai-responses", &output_items)
            .expect("openai-responses adapter should extract pending calls");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].call_id, "call_b");
        assert_eq!(pending[0].name, "tool_b");
    }

    #[test]
    fn test_provider_builds_tool_result_and_continuation_input_items() {
        let user_input_item = providers::build_user_input_item("openai-responses", "hello")
            .expect("openai-responses adapter should build user input item");
        assert_eq!(
            user_input_item.get("role").and_then(|v| v.as_str()),
            Some("user")
        );

        let tool_output_item =
            providers::build_tool_result_input_item("openai-responses", "call_1", "ok")
                .expect("openai-responses adapter should build tool result item");
        assert_eq!(
            tool_output_item.get("type").and_then(|v| v.as_str()),
            Some("function_call_output")
        );
        assert_eq!(
            tool_output_item.get("call_id").and_then(|v| v.as_str()),
            Some("call_1")
        );

        let output_items = vec![
            json!({"type":"reasoning","id":"r1"}),
            json!({"type":"function_call","call_id":"call_1","name":"tool_a"}),
        ];
        let continuation_items = providers::compose_tool_continuation_input(
            "openai-responses",
            &output_items,
            vec![tool_output_item.clone()],
        )
        .expect("openai-responses adapter should build continuation input items");
        assert_eq!(continuation_items.len(), 3);
        assert_eq!(
            continuation_items[0].get("id").and_then(|v| v.as_str()),
            Some("r1")
        );
        assert_eq!(
            continuation_items[1].get("type").and_then(|v| v.as_str()),
            Some("function_call")
        );
        assert_eq!(
            continuation_items[2].get("type").and_then(|v| v.as_str()),
            Some("function_call_output")
        );
    }

    #[tokio::test]
    #[ignore = "requires configured real upstream credentials and network access"]
    async fn real_openai_responses_and_chat_completions_smoke_from_local_config() {
        if std::env::var("AGENTJAX_REAL_PROVIDER_SMOKE")
            .ok()
            .as_deref()
            != Some("1")
        {
            eprintln!("Skip provider smoke test. Set AGENTJAX_REAL_PROVIDER_SMOKE=1 to enable.");
            return;
        }

        ensure_rustls_crypto_provider();
        let config = crate::config::load_config().expect("load local AgentJax config");

        for provider_kind in ["openai-responses", "chat-completions"] {
            let (smoke_config, model_ref) = config_for_provider_smoke(&config, provider_kind)
                .unwrap_or_else(|| {
                    panic!("missing enabled model for provider kind {provider_kind}")
                });
            let resolved = smoke_config
                .resolve_model_profile(Some(&model_ref))
                .expect("resolve smoke-test model");
            assert!(
                resolved.provider.resolved_credential().is_some(),
                "provider kind {provider_kind} has no resolved credential"
            );

            let user_item = providers::build_user_input_item(
                provider_kind,
                "Reply with exactly this token and no extra words: agentjax-smoke-ok",
            )
            .expect("build provider user input");
            let request = ResponseStreamRequest {
                input_items: vec![user_item],
                model: Some(model_ref.clone()),
                reasoning_effort: None,
                instructions_override: None,
                text: None,
                include: None,
                service_tier: None,
                prompt_cache_key: None,
                client_metadata: None,
                generate: None,
                tools: None,
                tool_choice: None,
            };

            let (_cancel_tx, mut cancel_rx) = watch::channel(false);
            let mut events = Vec::new();
            let result = tokio::time::timeout(
                Duration::from_secs(120),
                providers::stream_response(&smoke_config, &request, &mut cancel_rx, |event| {
                    events.push(event);
                    Ok(())
                }),
            )
            .await
            .unwrap_or_else(|_| panic!("{provider_kind} smoke test timed out"))
            .unwrap_or_else(|err| panic!("{provider_kind} smoke test failed: {err}"));

            assert!(
                !result.output_text.trim().is_empty(),
                "{provider_kind} returned empty output text"
            );
            assert!(
                events.iter().any(|event| matches!(
                    event,
                    ProviderStreamEvent::OutputTextDelta { .. }
                        | ProviderStreamEvent::AssistantMessageCompleted { .. }
                        | ProviderStreamEvent::HopAssistantText { .. }
                )),
                "{provider_kind} did not emit text stream events"
            );
        }
    }

    fn config_for_provider_smoke(
        config: &crate::config::AppConfig,
        provider_kind: &str,
    ) -> Option<(crate::config::AppConfig, String)> {
        if let Some(model_ref) = first_enabled_model_ref_for_kind(config, provider_kind) {
            return Some((config.clone(), model_ref));
        }

        if provider_kind != "chat-completions" {
            return None;
        }

        let (source_key, source_provider) =
            first_enabled_provider_for_kind(config, "openai-responses")?;
        let model_key = source_provider
            .models
            .iter()
            .find(|(_, model)| model.enabled)
            .map(|(model_key, _)| model_key.clone())?;

        let mut smoke_config = config.clone();
        let mut provider = source_provider.clone();
        provider.kind = "chat-completions".to_string();
        provider.custom_settings.insert(
            "supportsWebsockets".to_string(),
            serde_json::Value::Bool(false),
        );
        provider.custom_settings.insert(
            "streamTransport".to_string(),
            serde_json::Value::String("sse".to_string()),
        );

        let smoke_key = format!("{source_key}-chat-completions-smoke");
        smoke_config.providers.insert(smoke_key.clone(), provider);
        Some((smoke_config.normalize(), format!("{smoke_key}/{model_key}")))
    }

    fn first_enabled_model_ref_for_kind(
        config: &crate::config::AppConfig,
        provider_kind: &str,
    ) -> Option<String> {
        first_enabled_provider_for_kind(config, provider_kind).and_then(
            |(provider_key, provider)| {
                provider
                    .models
                    .iter()
                    .find(|(_, model)| model.enabled)
                    .map(|(model_key, _)| format!("{provider_key}/{model_key}"))
            },
        )
    }

    fn first_enabled_provider_for_kind<'a>(
        config: &'a crate::config::AppConfig,
        provider_kind: &str,
    ) -> Option<(&'a String, &'a crate::config::ProviderConfig)> {
        config.providers.iter().find(|(_, provider)| {
            provider.kind == provider_kind && provider.models.values().any(|model| model.enabled)
        })
    }
}
