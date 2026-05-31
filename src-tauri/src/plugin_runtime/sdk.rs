//! Shared plugin SDK modules provided via deno_core's `StaticModuleLoader`.
//!
//! Built-in and user plugins can `import` from these pre-registered modules
//! instead of bundling duplicate helper code. The SDK sources are compiled
//! into the binary, so no filesystem access is needed at runtime.

use deno_core::{StaticModuleLoader, url::Url};
use std::rc::Rc;

/// Module specifier for the AgentJax SDK shared library.
pub const SDK_MODULE_SPECIFIER: &str = "builtin:/agentjax/sdk";

/// Return an `Rc<dyn ModuleLoader>` pre-populated with all built-in SDK modules.
///
/// Call this once when initialising the plugin runtime and clone the `Rc` for
/// each new plugin `JsRuntime` instance so all plugins share the same module
/// resolution table.
pub fn create_sdk_module_loader() -> Rc<dyn deno_core::ModuleLoader> {
    let sdk_url = Url::parse(SDK_MODULE_SPECIFIER).expect("SDK module specifier is a valid URL");
    let sdk_source: &'static str = include_str!("../../builtin-plugins/sdk/sdk.js");
    let loader = StaticModuleLoader::new([(sdk_url, sdk_source)]);
    Rc::new(loader)
}

#[cfg(test)]
mod tests {
    use super::*;
    use deno_core::{JsRuntime, RuntimeOptions};

    #[test]
    fn sdk_module_is_loadable() {
        let module_loader = create_sdk_module_loader();
        let mut runtime = JsRuntime::new(RuntimeOptions {
            module_loader: Some(module_loader),
            ..Default::default()
        });

        let result = runtime.execute_script(
            "<test-sdk-import>",
            r#"
(async () => {
  const sdk = await import("builtin:/agentjax/sdk");
  if (typeof sdk.withQuery !== "function") throw new Error("withQuery missing");
  if (typeof sdk.event !== "function") throw new Error("event missing");
  if (typeof sdk.textFromContent !== "function") throw new Error("textFromContent missing");
  if (typeof sdk.parseArgs !== "function") throw new Error("parseArgs missing");
  if (typeof sdk.headerMap !== "function") throw new Error("headerMap missing");
  if (typeof sdk.usageFrom !== "function") throw new Error("usageFrom missing");
  if (typeof sdk.applyRequestConfig !== "function") throw new Error("applyRequestConfig missing");
  const result = sdk.withQuery("https://example.com/path", { foo: "bar", baz: "qux" });
  if (result !== "https://example.com/path?foo=bar&baz=qux") {
    throw new Error("withQuery returned unexpected: " + result);
  }
  return "ok";
})()
"#,
        );
        assert!(result.is_ok(), "SDK module should load and export all helpers");
    }

    #[test]
    fn sdk_header_map_bearer_strategy() {
        let module_loader = create_sdk_module_loader();
        let mut runtime = JsRuntime::new(RuntimeOptions {
            module_loader: Some(module_loader),
            ..Default::default()
        });

        let result = runtime.execute_script(
            "<test-header-map>",
            r#"
(async () => {
  const sdk = await import("builtin:/agentjax/sdk");
  const headers = sdk.headerMap(
    { "Content-Type": "application/json" },
    { credential: "sk-test", resolvedHttpHeaders: {} },
    "bearer"
  );
  if (headers.Authorization !== "Bearer sk-test") throw new Error("bearer auth failed");
  if (headers["Content-Type"] !== "application/json") throw new Error("base headers lost");
  return "ok";
})()
"#,
        );
        assert!(result.is_ok(), "headerMap bearer strategy should work");
    }

    #[test]
    fn sdk_usage_from_handles_all_shapes() {
        let module_loader = create_sdk_module_loader();
        let mut runtime = JsRuntime::new(RuntimeOptions {
            module_loader: Some(module_loader),
            ..Default::default()
        });

        // Anthropic shape
        let r1 = runtime.execute_script(
            "<test-usage-anthropic>",
            r#"
(async () => {
  const sdk = await import("builtin:/agentjax/sdk");
  const u = sdk.usageFrom({ message: { usage: { input_tokens: 10, output_tokens: 20 } } });
  if (!u || u.promptTokens !== 10 || u.completionTokens !== 20) throw new Error("anthropic");
  return "ok";
})()
"#,
        );
        assert!(r1.is_ok(), "Anthropic usage shape");

        // Chat Completions shape
        let r2 = runtime.execute_script(
            "<test-usage-chat>",
            r#"
(async () => {
  const sdk = await import("builtin:/agentjax/sdk");
  const u = sdk.usageFrom({ usage: { prompt_tokens: 5, completion_tokens: 15 } });
  if (!u || u.promptTokens !== 5 || u.completionTokens !== 15) throw new Error("chat");
  return "ok";
})()
"#,
        );
        assert!(r2.is_ok(), "Chat Completions usage shape");

        // Gemini shape
        let r3 = runtime.execute_script(
            "<test-usage-gemini>",
            r#"
(async () => {
  const sdk = await import("builtin:/agentjax/sdk");
  const u = sdk.usageFrom({ usageMetadata: { promptTokenCount: 8, candidatesTokenCount: 12 } });
  if (!u || u.promptTokens !== 8 || u.completionTokens !== 12) throw new Error("gemini");
  return "ok";
})()
"#,
        );
        assert!(r3.is_ok(), "Gemini usage shape");
    }

    #[test]
    fn sdk_event_with_and_without_data() {
        let module_loader = create_sdk_module_loader();
        let mut runtime = JsRuntime::new(RuntimeOptions {
            module_loader: Some(module_loader),
            ..Default::default()
        });

        let result = runtime.execute_script(
            "<test-event>",
            r#"
(async () => {
  const sdk = await import("builtin:/agentjax/sdk");
  const e1 = sdk.event("OutputTextStarted");
  if (e1.type !== "OutputTextStarted" || e1.data !== undefined) throw new Error("no-data");
  const e2 = sdk.event("ToolCallStarted", { call_id: "call_1" });
  if (e2.type !== "ToolCallStarted" || e2.data.call_id !== "call_1") throw new Error("with-data");
  return "ok";
})()
"#,
        );
        assert!(result.is_ok(), "event helper should work");
    }
}
