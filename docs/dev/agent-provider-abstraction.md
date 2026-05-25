# Agent Provider Abstraction Notes

Last updated: 2026-05-25

## Goal

This document records the behavior we verified against the current project gateway and uses those results to define an internal provider abstraction for the future agent runtime.

The immediate goal is:

- treat the current gateway as a `Codex-style Responses` provider
- build a separate `OpenAI Responses` provider later
- keep the agent runtime mostly provider-agnostic so that future `Anthropic` and `Gemini` integrations can reuse the same core loop

## Test Target

Current local config points to:

- `api_endpoint = https://api.iowa-1.2007911.xyz/v1`
- inferred websocket endpoint = `wss://api.iowa-1.2007911.xyz/v1/responses`
- tested model profile = `gpt-5.4-mini`
- upstream behavior strongly suggests this gateway is forwarding to a `Codex-style responses backend`, not the standard OpenAI `/v1/responses` transport

The inference above is based on observed protocol behavior, not on direct upstream visibility.

## Summary

The current gateway is usable for agent workflows, but it is not wire-compatible with the standard OpenAI Responses WebSocket mode.

It behaves like a `Codex-style` provider with these important properties:

- websocket requests require `instructions`
- websocket requests require `stream=true`
- `store=true` is rejected
- provider-side `previous_response_id` continuation exists but is no longer used by runtime
- final `response.completed.response.output` is empty
- real output items arrive in `response.output_item.done`
- function call arguments stream incrementally via `response.function_call_arguments.delta`
- built-in OpenAI tool type `web_search_preview` is not supported
- `generate=false` warmup works

This means the agent runtime must be built around streamed items and tool-loop events, not around the final `response.completed.response.output` object.

## Verified Cases

### Transport and request shape

| Case | Result | Notes |
| --- | --- | --- |
| Basic websocket text turn | Pass | Normal text delta flow works |
| Official minimal WS-style payload without `instructions` | Fail | `Instructions are required` |
| WS payload without `stream=true` | Fail | `Stream must be set to true` |
| `store=true` in WS mode | Fail | `Store must be set to false` |
| Metadata field | Pass | `metadata` is accepted |
| `generate=false` warmup | Pass | Returned a response id and later continuation worked |

### Structured output

| Case | Result | Notes |
| --- | --- | --- |
| `text.format = json_object` | Pass | Input must explicitly mention `JSON` |
| `text.format = json_schema` | Pass | Schema-constrained output worked |
| `json_schema` on unsafe prompt | Pass | Model stayed inside schema and returned `ok: false` instead of emitting a refusal item |
| `json_schema` with tiny `max_output_tokens` | Unexpected | Still returned a complete JSON object; tested limit did not appear to constrain output the way standard Responses would |

### Tool use

| Case | Result | Notes |
| --- | --- | --- |
| Single function call | Pass | `function_call` item emitted |
| Incremental function call arguments | Pass | `response.function_call_arguments.delta` and `.done` emitted |
| Same-socket `function_call_output` continuation | Pass | Tool loop completed successfully |
| Parallel tool calls in one turn | Pass | Two separate `function_call` items emitted |
| Built-in `web_search_preview` | Fail | `Unsupported tool type: web_search_preview` |

### Continuation and state

| Case | Result | Notes |
| --- | --- | --- |
| Same-socket `previous_response_id` continuation | Pass | Provider supports it, but runtime no longer depends on it |
| Fresh-socket continuation with `store=false` | Fail | `previous_response_not_found` |
| Fresh-socket continuation with `store=true` | Not available | `store=true` is rejected before generation |

## Observed Event Model

The gateway emits a usable but non-standard event model.

Common events observed:

- `codex.rate_limits`
- `response.created`
- `response.in_progress`
- `response.output_item.added`
- `response.content_part.added`
- `response.output_text.delta`
- `response.output_text.done`
- `response.content_part.done`
- `response.output_item.done`
- `response.function_call_arguments.delta`
- `response.function_call_arguments.done`
- `response.completed`
- `error`

Important detail:

- `response.completed.response.output` was always empty in all tested success cases
- the actual assistant message or function call lived in `response.output_item.done`

For this provider, `response.output_item.done` is the canonical event for completed items.

## Key Differences From Standard OpenAI Responses

These are the main differences we must preserve in the provider split:

1. Request strictness

- current gateway requires `instructions`
- current gateway requires `stream=true`
- standard OpenAI WebSocket mode should not require either of those in the same way

2. Persistence model

- current gateway rejects `store=true`
- runtime continuation is now fully local and no longer depends on provider-side persisted response chains

3. Final response shape

- current gateway returns empty `response.output` on `response.completed`
- standard OpenAI Responses expects meaningful final `response.output` items

4. Tool surface

- current gateway supports user-defined function tools
- current gateway does not support the tested built-in tool type `web_search_preview`
- standard OpenAI Responses supports built-in tools like web search and file search

5. Refusal behavior under `json_schema`

- current gateway kept the model inside the supplied schema and encoded refusal semantics as structured JSON
- standard OpenAI Responses may emit an explicit refusal content item in some structured-output flows

## Implications for the Current Codebase

The current app can stream text, but it is not yet preserving enough protocol detail for a real multi-provider agent runtime.

Current limitations:

- the frontend/backend request shape only supports plain chat input, selected model, and reasoning effort
- streamed protocol handling only emits thinking and text deltas
- final persistence stores assistant replies mostly as text, not as full provider items

This is sufficient for chat UX, but not sufficient for a durable provider-agnostic agent core.

## Recommended Internal Abstraction

We should split the architecture into three layers:

1. Agent runtime core
2. Provider adapter
3. UI/session persistence

### 1. Agent runtime core

The core should not know whether it is talking to Codex, OpenAI, Anthropic, or Gemini.

The core should operate on these concepts:

- turn request
- streamed events
- output items
- tool calls
- tool outputs
- continuation state
- capabilities

Suggested core request shape:

```ts
type AgentTurnRequest = {
  model: string
  instructions?: string
  inputItems: AgentInputItem[]
  tools?: AgentToolDefinition[]
  toolChoice?: "auto" | "none" | "required" | { functionName: string }
  outputMode?: AgentOutputMode
  metadata?: Record<string, string>
  generationMode?: "generate" | "warmup"
  persistence?: "provider-default" | "must-store" | "must-not-store"
}
```

Suggested core event shape:

```ts
type AgentTurnEvent =
  | { type: "rate_limits"; raw: unknown }
  | { type: "response_started"; responseId: string }
  | { type: "reasoning_started" }
  | { type: "message_delta"; itemId: string; text: string }
  | { type: "message_done"; item: AgentOutputItem }
  | { type: "tool_call_args_delta"; itemId: string; callId: string; delta: string }
  | { type: "tool_call_done"; item: AgentToolCallItem }
  | { type: "response_completed"; result: AgentTurnResult }
  | { type: "error"; error: ProviderError }
```

Suggested core result shape:

```ts
type AgentTurnResult = {
  responseId: string
  outputItems: AgentOutputItem[]
  outputText: string
  rawFinal: unknown
}
```

### 2. Provider adapter

Each provider adapter is responsible for mapping raw wire behavior into the internal event model.

Examples:

- `CodexProvider`
- `OpenAIResponsesProvider`
- `AnthropicProvider`
- `GeminiProvider`

Each provider must expose a capability object.

Suggested capability shape:

```ts
type ProviderCapabilities = {
  requiresInstructions: boolean
  requiresStreamTrueInWebSocket: boolean
  supportsStoredResponses: boolean
  supportsCrossSocketContinuation: boolean
  supportsGenerateFalse: boolean
  supportsJsonMode: boolean
  supportsJsonSchema: boolean
  supportsParallelToolCalls: boolean
  supportsBuiltInWebSearch: boolean
  emitsFinalOutputItems: boolean
  emitsIncrementalToolCallArguments: boolean
}
```

This capability layer is what will let us add future providers with minimal runtime changes.

### 3. UI and persistence

The UI may still render plain text, but persistence must retain the raw item structure.

Do not persist only:

- final assistant text
- synthetic assistant message reconstructed from text alone

Persist instead:

- all provider output items
- raw tool call item payloads
- raw tool output items
- continuation references
- provider capability snapshot if useful for debugging

This matters because:

- Codex-style providers may only expose canonical outputs in streamed item events
- OpenAI Responses may use final response output arrays more heavily
- Anthropic and Gemini will each have their own tool/result conventions

## Provider-Specific Guidance

### Codex-style provider

Treat the current gateway as a separate provider with these rules:

- always send `instructions`
- always send `stream=true` for websocket mode
- force `store=false`
- treat `response.output_item.done` as canonical output
- support same-socket continuation only
- support `generate=false`
- support user-defined function tools
- do not assume built-in OpenAI tools are available

### OpenAI Responses provider

Implement separately and keep it aligned to official behavior:

- websocket mode should follow the official `/v1/responses` semantics
- stored responses may be supported
- built-in tools may be supported
- final `response.output` should be treated as meaningful
- explicit refusal items should be expected in structured output flows

Do not add Codex-specific request hacks to the OpenAI provider.

## Runtime Design Rules

1. Build the runtime around items, not text

Text is one rendering of the model output. The runtime should consider these all first-class:

- assistant message item
- function call item
- function call output item
- refusal item
- reasoning item

2. Continuation state must be provider-aware

`previous_response_id` should not be part of runtime continuation state.

We also need:

- socket affinity when required
- provider capability checks
- fallback-to-full-context replay when continuation is unavailable

3. Tool loop must be event-driven

Do not wait for the final response object to discover tool calls.

Instead:

- observe streamed tool call events
- assemble arguments from incremental deltas if needed
- execute tools
- submit `function_call_output`
- continue the turn

4. Persistence must preserve raw provider items

For future cross-provider support, the conversation log should be rich enough to rebuild:

- tool chains
- structured output parsing
- provider-specific recovery

5. Capability checks should happen before requests

Examples:

- if provider does not support built-in web search, reject or reroute that tool request before sending
- if provider requires same-socket continuation, do not attempt reconnect-based continuation without replay
- if provider does not support `store=true`, do not let upper layers depend on persistent response hydration

## Recommended Near-Term Refactor

Suggested order:

1. Introduce provider capability objects
2. Add a provider-agnostic output item model
3. Capture `response.output_item.done` and `response.function_call_arguments.*`
4. Persist raw provider items alongside rendered text
5. Implement a dedicated `CodexProvider`
6. Keep the current `OpenAI` provider name only for the future true OpenAI Responses adapter

## Additional Test Ideas

Useful follow-up coverage before adding more providers:

- more complex nested `json_schema`
- explicit refusal item detection on a true OpenAI Responses provider
- socket reconnect after long-running session
- repeated tool loop with more than one round trip
- image/file input support
- background mode behavior on providers that expose it
- provider-side compaction and context management

## Bottom Line

The current gateway is good enough to support a serious agent runtime, but only if we model it as a `Codex-style` provider instead of pretending it is standard OpenAI Responses.

The most important implementation choice is this:

- make the runtime item-based and capability-driven
- keep each vendor/provider adapter thin and explicit

That is what will make later OpenAI, Anthropic, and Gemini integrations mostly incremental instead of invasive.
