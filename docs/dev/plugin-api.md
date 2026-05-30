# AgentJax Plugin API Draft

This document captures the first host-side plugin contract. The Rust API lives
under `src-tauri/src/plugin_runtime`.

## Manifest

Plugins live under `$AGENTJAX_HOME/plugins`. Each plugin should be placed in its
own directory with a `plugin.json` manifest:

```text
$AGENTJAX_HOME/
  plugins/
    local.demo/
      plugin.json
      plugin.js
```

Plugins declare metadata and tools in `plugin.json`. The host validates the
manifest before loading any JavaScript.

```json
{
  "id": "local.demo",
  "name": "Local Demo",
  "version": "0.1.0",
  "apiVersion": 1,
  "entrypoint": "plugin.ts",
  "description": "Demo plugin",
  "tools": [
    {
      "name": "say_hello",
      "displayName": "Say Hello",
      "description": "Returns a greeting from the demo plugin.",
      "icon": "Puzzle",
      "kind": "function",
      "inputSchema": {
        "type": "object",
        "properties": {
          "name": { "type": "string" }
        }
      }
    }
  ],
  "sandbox": {
    "allowFileRead": false,
    "allowFileWrite": false,
    "allowNetwork": false,
    "allowProcessSpawn": false,
    "allowEnvRead": false,
    "maxExecutionMs": 30000
  }
}
```

## Tool Names

The host preserves manifest tool names for dispatch, then exposes provider-safe
names to the model:

```text
plugin__{sanitizedPluginId}__{sanitizedToolName}
```

For example, `local.demo` plus `say_hello` becomes
`plugin__local_demo__say_hello`.

## Runtime Boundary

The Rust `PluginRuntime` trait now owns three responsibilities:

1. Register and validate manifests.
2. List registered plugin tools for catalog mounting.
3. Prepare validated `PluginToolCall` payloads with the resolved sandbox policy.
4. Execute a prepared synchronous JavaScript tool call.

The first `DenoCorePluginRuntime` execution bridge loads the entrypoint script
from disk and expects it to install a global plugin object:

```js
globalThis.AgentJaxPlugin = {
  tools: {
    say_hello(args, context) {
      return {
        greeting: `Hello, ${args.name ?? "there"}`,
        conversationId: context.conversationId ?? null
      };
    }
  }
};
```

Handlers are synchronous in this draft. A handler may return any JSON-like value,
which the host wraps as `{ ok: true, output: value }`, or it may return an
explicit `{ ok, output, error }` object. The next phase should add ESM entrypoint
loading, async handlers, and explicit host ops instead of expanding this global
bridge.
