// OpenAI Provider — fully declarative in plugin.json.
//
// All provider metadata (capabilities, model routing, built-in models,
// config schema, auth) is declared in plugin.json and parsed directly
// by Rust. This JS stub exists only for build.rs compatibility.
//
// The JS runtime is NOT created for this plugin — provider_definitions_for_package
// skips JS extraction when the manifest contains model_routing or builtin_models.

globalThis.AgentJaxPlugin = {
  providers: [],
  tools: {},
};
