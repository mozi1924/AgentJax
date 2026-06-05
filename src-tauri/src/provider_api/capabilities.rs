use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderCapabilities {
    pub requires_instructions: bool,
    pub requires_stream_true_in_websocket: bool,
    pub supports_stored_responses: bool,
    pub supports_cross_socket_continuation: bool,
    pub supports_generate_false: bool,
    pub supports_json_mode: bool,
    pub supports_json_schema: bool,
    pub supports_parallel_tool_calls: bool,
    pub supports_built_in_web_search: bool,
    pub emits_final_output_items: bool,
    pub emits_incremental_tool_call_arguments: bool,
}

impl ProviderCapabilities {
    pub fn openai_responses() -> Self {
        Self {
            requires_instructions: true,
            requires_stream_true_in_websocket: true,
            supports_stored_responses: false,
            supports_cross_socket_continuation: false,
            supports_generate_false: true,
            supports_json_mode: true,
            supports_json_schema: true,
            supports_parallel_tool_calls: true,
            supports_built_in_web_search: false,
            emits_final_output_items: true,
            emits_incremental_tool_call_arguments: true,
        }
    }

    pub fn chat_completions() -> Self {
        Self {
            requires_instructions: false,
            requires_stream_true_in_websocket: false,
            supports_stored_responses: false,
            supports_cross_socket_continuation: false,
            supports_generate_false: false,
            supports_json_mode: true,
            supports_json_schema: true,
            supports_parallel_tool_calls: true,
            supports_built_in_web_search: false,
            emits_final_output_items: false,
            emits_incremental_tool_call_arguments: true,
        }
    }
}
