# Agent Core Refactor (Codex-rs Aligned)

## Goal

Rebuild AgentJax runtime around a stable turn engine so behavior is closer to OpenAI Codex-style UX:

- deterministic turn loop
- provider stream mapped into internal events
- local tool loop continuation
- conversation context replay that survives tool lifecycle changes

## What changed in this phase

This phase keeps external API stable (`AgentRuntime::run_turn`) but rewires internals:

- `src-tauri/src/runtime/engine.rs`
  - new turn orchestrator
  - owns turn accumulator and continuation loop
- `src-tauri/src/runtime/stream_collection.rs`
  - provider stream event collection
  - pending tool call extraction with fallback to output-item scan
- `src-tauri/src/runtime/tool_execution.rs`
  - local tool execution retry/failure-guard
  - tool output event emission and timeline records
- `src-tauri/src/runtime/turn.rs`
  - compatibility entrypoint; delegates to engine

## Why this matches codex-rs direction

This layout mirrors codex-rs separation:

- session/turn control (our `engine.rs`) vs tool orchestration (our `tool_execution.rs`)
- stream item handling isolated from tool execution (our `stream_collection.rs`)
- explicit continuation construction after tool results

## Next phases

1. Introduce a typed `TurnState` model (`Sampling`, `ExecutingTools`, `Completed`, `Interrupted`) and expose it to UI stream mapping.
2. Split persistence from runtime command path:
   - runtime returns a full `TurnReport` (items + timeline + metrics)
   - chat command only handles IO/event emission.
3. Add provider adapter contract tests:
   - streamed function_call args
   - multi-tool same turn
   - malformed/incomplete tool events fallback.
4. Add runtime metrics and guardrails:
   - per-turn hop count
   - tool retry counters
   - cancellation and timeout reason codes.
