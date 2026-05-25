#[cfg(test)]
mod tests {
    use crate::providers;
    use crate::tools::ToolSchemaFormat;

    #[test]
    fn test_provider_capability_lookup() {
        let codex = providers::get_capabilities("codex").expect("codex adapter should exist");
        assert!(!codex.supports_stored_responses);

        let openai = providers::get_capabilities("openai-standard")
            .expect("openai-standard adapter should exist");
        assert!(openai.supports_stored_responses);

        let err = providers::get_capabilities("not-a-provider").unwrap_err();
        assert!(err.contains("Unsupported provider kind"));
    }

    #[test]
    fn test_provider_tool_schema_format_lookup() {
        let codex = providers::get_tool_schema_format("codex").expect("codex adapter should exist");
        assert_eq!(codex, ToolSchemaFormat::Responses);

        let openai = providers::get_tool_schema_format("openai-standard")
            .expect("openai-standard adapter should exist");
        assert_eq!(openai, ToolSchemaFormat::Responses);
    }
}
