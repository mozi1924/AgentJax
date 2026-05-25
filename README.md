# AgentJax

AgentJax is a desktop AI assistant built with React, Vite, and Tauri. It focuses on configurable providers, reusable model profiles, and local MCP tool integration.

## Documentation

- [Documentation home](docs/README.md)
- [Configuration reference](docs/configuration.md)
- [Development plan](docs/dev/PLAN.md)
- [Provider abstraction notes](docs/dev/agent-provider-abstraction.md)

## Quick Start

```bash
pnpm install
pnpm dev
```

For the desktop app:

```bash
pnpm dev:desktop
```

To produce a build:

```bash
pnpm build
pnpm build:desktop
```

## Configuration

AgentJax creates its configuration on first launch under `AGENTJAX_HOME`:

- `AGENTJAX_HOME` default: `~/.agentjax`
- Config file: `$AGENTJAX_HOME/config.yaml`
- Sessions directory: `$AGENTJAX_HOME/sessions/`

The full configuration layout, field descriptions, and generated cache files are documented in [docs/configuration.md](docs/configuration.md).
