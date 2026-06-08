use app_lib::provider_api;
    use app_lib::provider_api::types::{ProviderStreamEvent, ResponseStreamRequest};
    use app_lib::tools::ToolSchemaFormat;
    use serde_json::json;
    use std::sync::Once;
    use std::time::Duration;
    use tokio::sync::watch;

    static RUSTLS_CRYPTO_PROVIDER: Once = Once::new();

    fn ensure_rustls_crypto_provider() {
        RUSTLS_CRYPTO_PROVIDER.call_once(|| {
            rustls::crypto::ring::default_provider()
                .install_default()
                .expect("Failed to install rustls ring crypto provider");
        });
    }

    #[test]
    fn test_provider_capability_lookup() {
        let openai = provider_api::get_capabilities("openai").expect("openai adapter should exist");
        assert!(!openai.supports_stored_responses);

        let err = provider_api::get_capabilities("not-a-provider").unwrap_err();
        assert!(err.contains("Unsupported provider kind"));

        let old_codex = provider_api::get_capabilities("codex").unwrap_err();
        assert!(old_codex.contains("Unsupported provider kind"));
    }

    #[test]
    fn test_provider_tool_schema_format_lookup() {
        let openai =
            provider_api::get_tool_schema_format("openai").expect("openai adapter should exist");
        assert_eq!(openai, ToolSchemaFormat::Responses);
    }

    #[test]
    fn test_builtin_provider_plugins_seed_registry() {
        let definitions = provider_api::registry::builtin_provider_definitions();
        let kinds = definitions
            .iter()
            .map(|definition| definition.kind.as_str())
            .collect::<Vec<_>>();

        assert!(
            kinds.contains(&"openai"),
            "expected openai provider, got {kinds:?}"
        );
        assert!(
            kinds.contains(&"deepseek"),
            "expected deepseek provider, got {kinds:?}"
        );

        let openai = definitions
            .iter()
            .find(|definition| definition.kind == "openai")
            .expect("openai provider plugin should be registered");
        assert_eq!(openai.tool_schema_format, ToolSchemaFormat::Responses);

        let deepseek = definitions
            .iter()
            .find(|definition| definition.kind == "deepseek")
            .expect("deepseek provider plugin should be registered");
        assert_eq!(
            deepseek.tool_schema_format,
            ToolSchemaFormat::ChatCompletions
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

        let pending = provider_api::extract_pending_tool_calls("openai", &output_items)
            .expect("extract pending calls");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].call_id, "call_b");
        assert_eq!(pending[0].name, "tool_b");
    }

    #[test]
    fn test_provider_builds_tool_result_and_continuation_input_items() {
        let user_input_item =
            provider_api::build_user_input_item("openai", "hello").expect("build user input item");
        assert_eq!(
            user_input_item.get("role").and_then(|v| v.as_str()),
            Some("user")
        );

        let tool_output_item = provider_api::build_tool_result_input_item("openai", "call_1", "ok")
            .expect("build tool result item");
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
        let continuation_items = provider_api::compose_tool_continuation_input(
            "openai",
            &output_items,
            vec![tool_output_item.clone()],
        )
        .expect("build continuation input items");
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
    async fn real_openai_smoke_from_local_config() {
        if std::env::var("AGENTJAX_REAL_PROVIDER_SMOKE")
            .ok()
            .as_deref()
            != Some("1")
        {
            eprintln!("Skip provider smoke test. Set AGENTJAX_REAL_PROVIDER_SMOKE=1 to enable.");
            return;
        }

        ensure_rustls_crypto_provider();
        let config = app_lib::config::load_config().expect("load local AgentJax config");
        let provider_kind = "openai";

        let model_ref = first_enabled_model_ref_for_kind(&config, provider_kind)
            .unwrap_or_else(|| panic!("missing enabled model for provider kind {provider_kind}"));
        let resolved = config
            .resolve_model_profile(Some(&model_ref))
            .expect("resolve smoke-test model");
        assert!(
            resolved.provider.resolved_credential().is_some(),
            "provider kind {provider_kind} has no resolved credential"
        );

        let user_item = provider_api::build_user_input_item(
            provider_kind,
            "Reply with exactly this token and no extra words: agentjax-smoke-ok",
        )
        .expect("build provider user input");
        let request = ResponseStreamRequest {
            input_items: vec![user_item],
            model: Some(model_ref.clone()),
            reasoning: None,
            instructions_override: None,
            text: None,
            include: None,
            service_tier: None,
            prompt_cache_key: None,
            client_metadata: None,
            generate: None,
            tools: None,
            tool_choice: None,
            ..Default::default()
        };

        let (_cancel_tx, mut cancel_rx) = watch::channel(false);
        let mut events = Vec::new();
        let result = tokio::time::timeout(
            Duration::from_secs(120),
            provider_api::stream_response(
                &config,
                &app_lib::config::AgentConfig::default(),
                &request,
                &mut cancel_rx,
                |event| {
                    events.push(event);
                    Ok(())
                },
            ),
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

    #[tokio::test]
    #[ignore = "requires configured real upstream credentials and network access"]
    async fn real_deepseek_smoke_from_local_config() {
        if std::env::var("AGENTJAX_REAL_PROVIDER_SMOKE")
            .ok()
            .as_deref()
            != Some("1")
        {
            eprintln!("Skip provider smoke test. Set AGENTJAX_REAL_PROVIDER_SMOKE=1 to enable.");
            return;
        }

        ensure_rustls_crypto_provider();
        let config = app_lib::config::load_config().expect("load local AgentJax config");
        let provider_kind = "deepseek";

        let model_ref = first_enabled_model_ref_for_kind(&config, provider_kind)
            .unwrap_or_else(|| panic!("missing enabled model for provider kind {provider_kind}"));
        eprintln!("Using model ref: {model_ref}");
        let resolved = config
            .resolve_model_profile(Some(&model_ref))
            .expect("resolve smoke-test model");
        assert!(
            resolved.provider.resolved_credential().is_some(),
            "provider kind {provider_kind} has no resolved credential"
        );
        eprintln!(
            "Protocol: {:?}, Endpoint: {}",
            resolved.api_protocol,
            resolved.provider.api_endpoint()
        );

        let user_item = provider_api::build_user_input_item(
            provider_kind,
            "Reply with exactly this token and no extra words: agentjax-smoke-ok",
        )
        .expect("build provider user input");
        let request = ResponseStreamRequest {
            input_items: vec![user_item],
            model: Some(model_ref.clone()),
            reasoning: None,
            instructions_override: None,
            text: None,
            include: None,
            service_tier: None,
            prompt_cache_key: None,
            client_metadata: None,
            generate: None,
            tools: None,
            tool_choice: None,
            ..Default::default()
        };

        let (_cancel_tx, mut cancel_rx) = watch::channel(false);
        let mut events = Vec::new();
        let result = tokio::time::timeout(
            Duration::from_secs(120),
            provider_api::stream_response(
                &config,
                &app_lib::config::AgentConfig::default(),
                &request,
                &mut cancel_rx,
                |event| {
                    events.push(event);
                    Ok(())
                },
            ),
        )
        .await
        .unwrap_or_else(|_| panic!("{provider_kind} smoke test timed out"))
        .unwrap_or_else(|err| panic!("{provider_kind} smoke test failed: {err}"));

        eprintln!("Output text: {:?}", &result.output_text);
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
        eprintln!("{provider_kind} smoke test PASSED");
    }

    fn first_enabled_model_ref_for_kind(
        config: &app_lib::config::AppConfig,
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
        config: &'a app_lib::config::AppConfig,
        provider_kind: &str,
    ) -> Option<(&'a String, &'a app_lib::config::ProviderConfig)> {
        config.providers.iter().find(|(_, provider)| {
            provider.kind == provider_kind && provider.models.values().any(|model| model.enabled)
        })
    }
