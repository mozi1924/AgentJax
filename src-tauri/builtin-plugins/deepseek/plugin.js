// DeepSeek Provider — fully declarative in plugin.json.
// All provider metadata is declared in plugin.json and parsed directly by Rust.
// The JS runtime is NOT created for this plugin — provider_definitions_for_package
// skips JS extraction when the manifest contains model_routing or builtin_models.
//
// DeepSeek uses the standard Chat Completions protocol (native Rust implementation).
// Thinking mode is enabled via `thinking: {"type": "enabled"}` top-level field
// in the request body, controlled by `reasoning_effort`.

globalThis.AgentJaxPlugin = {
  providers: [],
  tools: {},
};
