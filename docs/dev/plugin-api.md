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
  "settingsSections": [
    {
      "id": "plugin.local.demo.settings",
      "title": "Local Demo",
      "icon": "Puzzle",
      "order": 900,
      "children": [
        {
          "kind": "collapsible",
          "id": "plugin.local.demo.settings.advanced",
          "title": "Advanced",
          "defaultExpanded": false,
          "children": []
        }
      ]
    }
  ],
  "settingsData": {
    "items": [
      {
        "id": "primary",
        "name": "Primary item",
        "description": "Rendered by the shared SchemaRenderer plugin provider."
      }
    ]
  },
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

## Settings UI

Plugins should not ship standalone React settings panels. Static and
configuration-backed settings UI can be declared in manifest `settingsSections`
with SchemaRenderer nodes. Dynamic panels should use those schema nodes with a
namespaced data provider.

Simple plugin-owned data can be declared in manifest `settingsData`. Relative
keys are materialized as `plugin.{pluginId}.{key}` data sources; fully qualified
keys beginning with `plugin.` are preserved. See
[schema-renderer-runtime.md](schema-renderer-runtime.md) for the shared layout,
binding, property, action, and provider contract.
