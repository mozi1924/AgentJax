# Configuration Reference

This page documents the files and sections involved in AgentJax configuration.

## Files

- `config.yaml` is the user-editable configuration file.
- `models-cache.yaml` is generated automatically next to `config.yaml` and stores remote model metadata per provider.

## File Location

AgentJax creates the home directory automatically on first launch:

- Environment variable: `AGENTJAX_HOME`
- Default value when unset: `~/.agentjax`
- Config file path: `$AGENTJAX_HOME/config.yaml`
- Model cache path: `$AGENTJAX_HOME/models-cache.yaml`
- Sessions root: `$AGENTJAX_HOME/sessions/`

## Recommended `config.yaml` Layout

Keep the file in the same order as the runtime expects it:

1. `active_provider`
2. `default_model`
3. `utility_small_model`
4. `request_timeout_seconds`
5. `system_prompt`
6. `providers`
7. `mcp_runtime`
8. `mcp_servers`

That order keeps the file easier to scan as it grows.

## Top-Level Settings

- `active_provider`: the provider key currently in use.
- `default_model`: default model reference in `{provider}/{model_id}` format.
- `utility_small_model`: lightweight utility model in `{provider}/{model_id}` format.
- `request_timeout_seconds`: global fallback timeout in seconds.
- `system_prompt`: global default instruction string for all providers.
- `providers`: map of provider definitions.
- `mcp_runtime`: shared runtime settings for local MCP processes.
- `mcp_servers`: map of configured MCP servers.

## Provider Settings

Each entry under `providers` supports these fields:

- `kind`: provider kind used by the adapter layer.
- `api_endpoint`: base API endpoint.
- `models_endpoint_candidates`: optional alternate model-list endpoints.
- `realtime_endpoint`: explicit websocket endpoint override.
- `stream_transport`: `websocket` or `sse`.
- `credential`: inline credential value.
- `credential_env`: environment variable to read when `credential` is empty.
- `request_timeout_seconds`: provider-specific timeout override.
- `models`: provider-local model map.

If `credential` is empty, AgentJax falls back to `credential_env`.

## Provider Models

Each entry under `providers.<provider>.models` is a reusable preset for that provider.

- `model`: model identifier sent to the provider.
- `enabled`: whether the profile can be selected.
- `request`: model-specific request parameters.

Model selection keys are globally referenced as `{provider}/{model_id}`.
For example, `openai-responses/gpt-5-mini`.

Supported request fields:

- `temperature`
- `top_p`
- `top_k`
- `max_output_tokens`
- `frequency_penalty`
- `presence_penalty`
- `reasoning_effort`
- `extra_body`

`extra_body` is a passthrough map for provider-specific request fields.

## MCP Runtime

`mcp_runtime` holds shared runtime settings for local `stdio` MCP servers.

- `mcp_runtime.stdio.env`: environment variables shared by all local stdio MCP servers.
- `mcp_runtime.stdio.inherit_parent_env`: whether stdio MCP servers inherit the host process environment.

The default keeps stdio servers isolated from the main app process unless a server opts in.

## MCP Servers

Each entry under `mcp_servers` configures one MCP server.

Supported fields:

- `transport`: `stdio` or `streamable_http`.
- `command`: executable used for `stdio` transport.
- `args`: command arguments for the server process.
- `env`: server-specific environment variables.
- `cwd`: working directory for the server process.
- `use_global_stdio_env`: whether to merge `mcp_runtime.stdio.env` into the server environment.
- `inherit_parent_env`: whether this server inherits the parent environment.
- `uri`: endpoint used for `streamable_http`.
- `auth_header`: auth token or `Bearer ...` value for HTTP transport.
- `headers`: extra request headers for HTTP transport.
- `allow_stateless`: whether stateless operation is allowed.
- `channel_buffer_capacity`: optional internal channel buffer size.
- `reinit_on_expired_session`: whether to reinitialize after an expired session.
- `enabled`: whether the server is available for discovery and tool registration.

Transport rules:

- `stdio` requires `command`.
- `streamable_http` requires `uri`.

## Generated Cache

`models-cache.yaml` is refreshed automatically so the app can keep remote model listings around without hitting the provider on every startup.

- The cache is grouped by provider.
- The cache is treated as stale after 30 minutes.
- Startup and model directory reads can trigger a refresh when the cache is expired.

## Notes

- The first launch writes the default `config.yaml` if no file exists yet.
- The runtime normalizes keys, trims empty values, and fills in missing defaults where possible.
- Context is always managed locally by AgentJax conversation storage; runtime does not depend on `previous_response_id` chaining.
- If you are changing config fields in code, update this page at the same time so the layout stays readable.
