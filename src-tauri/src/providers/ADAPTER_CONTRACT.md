# Provider Adapter Contract

Provider adapters translate an upstream API into AgentJax's internal provider
protocol. The runtime should only consume `ProviderStreamEvent`,
`ResponseStreamResult`, and the normalized `output_items` timeline; it should
not know whether the upstream API was OpenAI Responses, Anthropic Messages,
Gemini GenerateContent, or Chat Completions.

## Required normalization

- Emit text through `ProviderStreamEvent::OutputTextDelta` and finish each hop
  with a `ResponseStreamResult`.
- Preserve tool calls as Responses-like `function_call` items in
  `output_items`.
- Preserve tool results as `function_call_output` items with the same
  `call_id`.
- Emit `UsageUpdated` whenever upstream usage becomes available. Provider usage
  is authoritative and must be preferred over local tokenizer estimates.
- If the upstream protocol does not provide `response_id`, `item_id`, or
  `call_id`, synthesize them inside the adapter before events reach the runtime.

## ID requirements

- `response_id` identifies one model response hop.
- `item_id` identifies a streamed output item within that hop.
- `call_id` identifies one logical tool invocation and must connect the
  `function_call` item to the later `function_call_output` item.

Anthropic and Chat Completions usually provide a native tool call ID that can be
mapped directly to `call_id`. Gemini function calls are name-based and normally
need synthetic IDs. Use `ProviderIdFactory` for synthetic IDs so future adapters
produce IDs with consistent prefixes and counters.

## Parser boundary

`providers/responses/stream/parser.rs` is only for OpenAI Responses-compatible
SSE payloads. Native provider streams should get their own adapter/parser
modules that emit the normalized events above. `providers/chat_completions.rs`
is the reference implementation for this split: it converts AgentJax's
Responses-like timeline into Chat Completions `messages`, then converts
streamed `choices.delta` chunks back into normalized AgentJax events.
`providers/gemini.rs` follows the same boundary for Gemini `contents`,
`functionCall`/`functionResponse`, and `usageMetadata`.
