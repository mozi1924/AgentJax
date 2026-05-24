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
base_url: "https://api.openai.com/v1"
websocket_url: ""
transport: "websocket"
api_key: ""
store: false
instructions: "You are Codex, a helpful AI assistant. Follow the user's instructions."
default_model: "gpt-5-mini"
available_models:
  - "gpt-5-mini"
  - "gpt-5"
request_timeout_seconds: 120
```

说明：

- `api_key` 为空时，会回退读取环境变量 `OPENAI_API_KEY`。
- 前端模型下拉会读取后端返回的 `effective_models`（优先使用配置文件中的 `available_models`）。
- 聊天默认模型使用 `default_model`。
- `transport` 支持 `websocket` / `sse`，默认 `websocket`。
- `websocket_url` 为空时，会从 `base_url` 自动推导（`https -> wss`，`http -> ws`）。
- `store` 控制 Responses 的会话持久化（部分第三方网关在 WebSocket 模式下要求 `store=false`，后端已做兼容）。
- `instructions` 为系统提示词，后端会随每次请求发送（部分网关要求必填）。
- 为兼容 `previous_response_id` 不稳定的网关，前端会将当前聊天历史一并发送给后端，由后端构造 `input[]`。

### 模型远端缓存

- 缓存文件：与配置文件同目录，文件名为 `models-cache.yaml`。
- 同步周期：后端每 30 分钟自动同步一次远端模型列表（`GET /models`）。
- 启动后和前端请求模型目录时，会按过期策略（30 分钟）补充同步。
- 该缓存用于后续模型设置能力（例如筛选、标签、分组）扩展。
