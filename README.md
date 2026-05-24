# React + Vite

This template provides a minimal setup to get React working in Vite with HMR and some ESLint rules.

Currently, two official plugins are available:

- [@vitejs/plugin-react](https://github.com/vitejs/vite-plugin-react/blob/main/packages/plugin-react) uses [Oxc](https://oxc.rs)
- [@vitejs/plugin-react-swc](https://github.com/vitejs/vite-plugin-react/blob/main/packages/plugin-react-swc) uses [SWC](https://swc.rs/)

## React Compiler

The React Compiler is not enabled on this template because of its impact on dev & build performances. To add it, see [this documentation](https://react.dev/learn/react-compiler/installation).

## Expanding the ESLint configuration

If you are developing a production application, we recommend using TypeScript with type-aware lint rules enabled. Check out the [TS template](https://github.com/vitejs/vite/tree/main/packages/create-vite/template-react-ts) for information on how to integrate TypeScript and [`typescript-eslint`](https://typescript-eslint.io) in your project.

## AgentJax Config (YAML)

应用启动时会自动初始化配置文件，遵循各系统配置目录规范：

- macOS: `~/Library/Application Support/AgentJax/config.yaml`
- Linux: `~/.config/AgentJax/config.yaml`
- Windows: `%APPDATA%\\AgentJax\\config.yaml`

示例：

```yaml
active_provider: "openai"
request_timeout_seconds: 120

providers:
  openai:
    kind: "openai"
    api_endpoint: "https://api.openai.com/v1"
    realtime_endpoint: ""
    stream_transport: "websocket"
    credential: ""
    credential_env: "OPENAI_API_KEY"
    store_responses: false
    system_prompt: "You are Codex, a helpful AI assistant. Follow the user's instructions."
    request_timeout_seconds: 120

model_profiles:
  gpt-5-mini:
    provider: "openai"
    model: "gpt-5-mini"
    enabled: true
    request:
      temperature: null
      top_p: null
      top_k: null
      max_output_tokens: null
      frequency_penalty: null
      presence_penalty: null
      reasoning_effort: null
      extra_body: {}
  gpt-5:
    provider: "openai"
    model: "gpt-5"
    enabled: true
    request:
      temperature: null
      top_p: null
      top_k: null
      max_output_tokens: null
      frequency_penalty: null
      presence_penalty: null
      reasoning_effort: null
      extra_body: {}

default_model: "gpt-5-mini"
```

说明：

- `providers` 支持多提供商并行配置，`active_provider` 控制当前生效提供商。
- `model_profiles` 用于定义“可选模型档案”，每个档案可以绑定不同 provider、底层模型 ID 以及参数。
- 模型级参数（`temperature`、`top_*`、`max_output_tokens` 等）写在 `model_profiles.*.request`。
- `extra_body` 可透传任意 Responses API 字段，便于快速接入新参数。
- 前端下拉展示的是 `model_profiles` 的 key，`default_model` 应该设置为对应 key。
- 若 provider 的 `credential` 为空，会回退读取该 provider 的 `credential_env` 环境变量。

### 模型远端缓存

- 缓存文件：与配置文件同目录，文件名为 `models-cache.yaml`。
- 格式：按 provider 分桶存储（每个 provider 独立 `last_synced_unix/models/source_api_endpoint`）。
- 同步周期：后端每 30 分钟自动同步一次远端模型列表（`GET /models`）。
- 启动后和前端请求模型目录时，会按过期策略（30 分钟）补充同步。
- 该缓存用于后续模型设置能力（例如筛选、标签、分组）扩展。
