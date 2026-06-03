# AGENTS.md

This file provides guidance to Codex (Codex.ai/code) when working with code in this repository.
# As the project is in its early stages, compatibility concerns are not yet an issue; therefore, problems should be resolved thoroughly rather than merely minimized.

## Build & Develop

```bash
pnpm install                          # Install frontend dependencies
pnpm dev                              # Vite dev server (port 1420, hot reload)
pnpm build                            # Production frontend build → dist/
pnpm dev:desktop                      # Full Tauri desktop app in dev mode
pnpm build:desktop                    # Production desktop binary
```

**Type checking and linting:**
```bash
pnpm typecheck                        # tsc --noEmit
pnpm lint                             # eslint .
```

**Testing:**

```bash
# Rust tests (from src-tauri/)
cargo test                            # All Rust tests
cargo test <test_name>                # Single test
cargo test lcm::                       # Module tests

# Frontend tests (Node test runner, no Jest/Vitest)
pnpm test:frontend                    # Run all frontend test scripts
node scripts/test-tool-manager-data.mjs   # Single test script
```

The Rust workspace is in `src-tauri/` and requires **Rust 1.95.0** (pinned in `src-tauri/rust-toolchain.toml`). There is no CI/CD or Docker configuration.

## Architecture

AgentJax is a **Tauri v2 desktop app** — React/TypeScript frontend (Vite, Tailwind CSS 4) + Rust backend. It's a local AI agent runtime with multi-provider support, MCP tool integration, plugin extensibility, and a deterministic context compression engine (LCM).

### Frontend (`src/`)

React 19 with a component tree roughly like:
- **App.tsx** — root orchestrator: sidebar, header, chat area, composer, settings modal
- **features/** — domain state management (conversations, i18n, models, settings, Tauri bridge). No global state library — state lives in React hooks and context providers.
- **components/settings/** — schema-driven settings UI. `SchemaRenderer` renders a JSON schema into form controls declaratively. `dataSources/` provides dynamic data to these schemas from the Tauri backend (plugin manager snapshots, tool catalog, provider registry, runtime state).
- **hooks/** — React hooks that bridge the Tauri IPC layer (`@tauri-apps/api`) for chat streaming, conversation management, and app config.

### Backend (`src-tauri/src/`)

The Rust backend is organized as a set of Tauri IPC command modules on top of core subsystems:

| Module | Role |
|--------|------|
| `commands/` | Tauri IPC handlers (`chat`, `config`, `models`, `tools`, `devtools`). These are the API surface the frontend calls. |
| `runtime/` | Agent runtime engine — the core loop that processes messages, invokes tools, and streams responses. |
| `provider_api/` | Provider abstraction layer — `stream_response`, `get_capabilities`, circuit breaker, retry. Multi-provider (Anthropic, OpenAI, Gemini, chat-completions) via a registry pattern. |
| `plugin_runtime/` | JavaScript plugin runtime powered by `deno_core`. Plugins define provider adapters. Built-in plugins live in `builtin-plugins/` (`.js` + `plugin.json`). |
| `lcm/` | **Lossless Context Management** — deterministic 3-level compaction engine (Normal/Aggressive/Truncation) backed by SQLite. Has a DAG for summary relationships and its own tools (`expand`, `grep`, `llm_map`, `agentic_map`). |
| `tools/` | Tool system: native tools (`files`, `calculator`, `background_jobs`), tool catalog/registry, and MCP tool mounting. The `catalog/` directory manages tool discovery, snapshots, and plugin tool execution. |
| `conversation_store/` | Conversation persistence: file I/O, locks, context window management (budgeting, truncation, token counting via `tokenizers`). |
| `config/` | App configuration system: YAML config I/O, settings snapshots/patches, prompt composer (block assembly), model profiles, settings UI section definitions. |
| `mcp.rs` | MCP client manager using `rmcp` (stdio + streamable HTTP transports). |

### Key patterns

- **Provider plugins** are JavaScript files run in `deno_core`. They expose a standard interface (capabilities, streaming, tool calling) that the Rust `provider_api` layer normalizes. Built-in plugins are in `src-tauri/builtin-plugins/`.
- **The settings UI** is entirely schema-driven. JSON schema definitions in `src-tauri/src/config/settings_ui_sections/*.json` describe the settings form layout. The React `SchemaRenderer` renders them. Dynamic data sources (tool list, provider list, plugin manager state) are fetched from the Rust backend via Tauri commands and fed into the schema context.
- **LCM tools** (`llm_map`, `agentic_map`) are tools the model can call to recursively explore summarized context — they enable operator-level recursion where the model maps over summarized blocks.
- **Frontend tests** use Node's built-in `node:test` runner and import TypeScript modules via SSR (server-side rendering React components to strings) — not a browser test framework.
- **Error handling** uses a unified `AgentJaxError` in `error.rs` with an `error_classifier.rs` for categorization.
