# AgentJax 代码库完全参考手册

> **目标**：本文档是 AgentJax 的完整代码库参考。每次新对话应首先阅读此文件，**无需**重新遍历整个代码库。
> 如有修改，请直接更新此文件，保持其始终为最新版本。

---

## 一、项目概述

AgentJax 是一个 **Tauri v2 桌面应用** — React 19/TypeScript 前端 (Vite, Tailwind CSS 4) + Rust 后端。它是一个本地 AI 代理运行时，支持多 Provider、MCP 工具集成、JS 插件扩展，以及确定性上下文压缩引擎 (LCM)。

- **Rust 版本**: 1.95.0 (pinned in `src-tauri/rust-toolchain.toml`)
- **Rust 后端**: 193+ `.rs` 文件, `src-tauri/src/`
- **前端**: 90+ `.ts`/`.tsx` 文件, `src/`
- **数据目录**: `~/.agentjax/`
- **仓库**: https://github.com/mozi1924/AgentJax
- **许可证**: GPL-3.0-only

---

## 二、构建与开发命令

```bash
# ── 前端 ──
pnpm install                          # 安装前端依赖
pnpm dev                              # Vite dev server (port 1420, hot reload)
pnpm build                            # 生产构建 → dist/
pnpm typecheck                        # tsc --noEmit
pnpm lint                             # eslint .

# ── 桌面应用 (Tauri) ──
pnpm dev:desktop                      # Tauri dev 模式
pnpm build:desktop                    # 生产桌面二进制
pnpm tauri                            # Tauri CLI

# ── Rust 测试 ──
cd src-tauri && cargo test            # 所有 Rust 测试
cd src-tauri && cargo test <name>     # 单个测试
cd src-tauri && cargo test lcm::      # 模块测试

# ── 前端测试 (Node test runner, 无 Jest/Vitest) ──
pnpm test:frontend                    # 运行所有前端测试
node scripts/test-tool-manager-data.mjs  # 单个测试脚本

# ── 代码生成 ──
pnpm gen:schemas                      # 生成 Rust schemas + TypeScript 类型
pnpm gen:types                        # 仅生成 TypeScript 类型
```

---

## 三、架构概览

```
main.tsx
  └─ I18nProvider (唯一 Context Provider)
      └─ App.tsx (所有状态通过 props drilling 传递)
           ├─ Sidebar (AgentSwitcher, SidebarConversationRow[], SidebarActionMenu)
           ├─ AppHeader (模型选择器, 推理模式选择)
           ├─ ChatArea (WorkLogPanel, markdownRenderer, CodeBlock)
           ├─ ChatComposer (FullscreenEditorModal)
           ├─ ConfirmModal
           ├─ LcmHealthModal
           └─ SettingsModal → SettingsRenderer → SchemaRenderer (递归)
```

### 关键架构决策

| 决策 | 说明 |
|------|------|
| **无全局状态库** | 无 Redux/Zustand。状态在 React hooks 中，通过 props drilling 从 App.tsx 下发 |
| **唯一 Context** | 仅有 `I18nProvider` |
| **乐观 UI** | 用户消息在 IPC 调用前立即显示，失败时回滚 |
| **对话懒加载** | 摘要先出现，详情通过 `load_conversation` 按需加载 |
| **Schema 驱动的设置 UI** | 设置表单由 Rust 和前端共享的 JSON Schema 描述 |
| **流状态用 Set** | 使用 `Set<string>` 跟踪 generating/stopping/thinking 状态 |
| **Tauri 运行时可选** | `tryGetCurrentWindow()` 让前端可在无 Tauri 环境下运行 |

---

## 四、Rust 后端模块详解

### 4.1 模块总览 (`lib.rs`)

入口点: `main.rs` → `app_lib::run()`

| 模块 | 可见性 | 说明 |
|------|--------|------|
| `config` | `pub` | 应用 + 代理配置系统 (YAML I/O, schema, 提示组合器) |
| `commands` | `mod` | Tauri IPC 命令处理器 (chat, config, tools, models, agents, memory, lcm_health, street, sub_agents, devtools) |
| `error` | `pub(crate)` | 统一错误系统 `AgentJaxError` |
| `error_classifier` | `mod` | Provider 错误分类 |
| `provider_api` | `pub(crate)` | Provider 抽象层 (Anthropic, OpenAI, Gemini, Chat Completions) |
| `runtime` | `pub(crate)` | 代理运行时引擎 (主循环, 工具执行) |
| `tools` | `pub(crate)` | 工具系统 (原生工具, 目录, MCP 挂载, 后台任务) |
| `mcp` | `pub(crate)` | MCP 客户端管理器 (rmcp, stdio + streamable HTTP) |
| `plugin_runtime` | `pub(crate)` | JS 插件运行时 (deno_core) |
| `lcm` | `pub(crate)` | 无损上下文管理引擎 (SQLite, 3 级压缩, DAG) |
| `conversation_store` | `mod` | 对话持久化 (JSONL + metadata.json) |
| `memory` | `pub(crate)` | 异步持久化记忆 (Markdown + YAML frontmatter) |
| `rag` | `pub(crate)` | RAG 系统 (LanceDB + SQLite FTS5) |
| `street` | `pub(crate)` | 异步通知队列 (跨轮事件) |
| `sub_agents` | `pub(crate)` | 子代理运行时 |
| `models` | `mod` | 模型缓存子系统 |
| `atomic_io` | `pub(crate)` | 原子文件写入 (临时文件+重命名) |
| `jsonl_store` | `pub(crate)` | 通用 JSONL I/O 工具 |
| `http_util` | `pub(crate)` | HTTP 头部解析 |
| `message_phase` | `mod` | 助手阶段枚举 (Commentary/FinalAnswer) |
| `time_context` | `mod` | 时间上下文系统项 |
| `agentjax_home` | `mod` | 家目录解析 |

### 4.2 错误系统 (`error.rs` + `error_classifier.rs`)

```rust
pub struct AgentJaxError {
    pub kind: ErrorKind,       // ProviderAuth, ProviderRateLimited, ProviderUnavailable,
                               // ProviderOutputIncomplete, Network, Config, ToolExecution,
                               // NotFound, SubAgent, Memory, Embedding, Internal
    pub message: String,
    pub retryable: bool,       // 根据 ErrorKind 自动设置
    pub provider_key: Option<String>,
    pub source: Option<String>,
}
```

- `AgentJaxResult<T>` 类型别名
- `agentjax_err!()` 宏用于快速错误构造
- `From` 实现: `String`, `LcmError`, `PluginRuntimeError`, `serde_json::Error`, `io::Error`
- `Serialize` 实现输出 `{kind, message, retryable, providerKey}` JSON 供前端消费
- `RawProviderError::classify()`: 401/403 → ProviderAuth, 429 → ProviderRateLimited, 5xx → ProviderUnavailable

### 4.3 配置系统 (`config/`)

**核心文件**: `schema.rs`, `app_config.rs`, `agent_config.rs`, `io.rs`, `prompt_composer.rs`

**配置分层**:
```
~/.agentjax/
├── config.yaml            # AppConfig — 共享配置 (providers, mcp, plugins)
└── agents/
    └── {agent_id}/
        ├── agent.yaml     # AgentConfig — 代理特定配置
        └── sessions/      # 对话存储
```

**关键类型** (`schema.rs`):
- `AppConfig`: providers, mcp_servers, plugin_manager, tool_manager 等
- `AgentConfig`: active_provider, default_model, prompt_composer, context_management, sub_agent, memory, rag, tool_manager
- `ProviderConfig`: kind, api_endpoint, models (BTreeMap<String, ProviderModelConfig>)
- `ProviderModelConfig`: enabled, name, model (model ID), request (ModelRequestConfig)
- `FullConfig { shared: AppConfig, agent: AgentConfig }`
- `ModelRequestConfig`: max_tokens, temperature, reasoning, extra_body (Value)
- `McpServerConfig`, `ToolManagerConfig`, `PluginManagerConfig` 等

**模型配置键收敛**: models map 的 key 是模型 ID (如 `gpt-5.4-mini`), `name` 字段为可选的显示名。

**提示组合器** (`prompt_composer.rs`):
- `PromptBlock`: id, title, role (System/Developer), content, enabled, source (User/Builtin/Plugin)
- `compile_prompt_composer()` → `CompiledPromptAssembly { instructions_text, system_items }`
- YAML 缩写: 写入 YAML 时 builtin/plugin blocks 只保留 `{id, enabled}`, 用户 blocks 完整序列化
- 归一化: 自动恢复缺失的 builtin blocks, 按 role 排序

**设置快照系统**:
- `SettingsSnapshot`: config_path, revision, values, dynamic_options, secret_statuses
- `SettingsPatch`: path, value, expected_revision, operation (Set/Delete), agent_id
- `SettingsUiSnapshot`: snapshot + sections (8 个内置 section + 插件 sections)

### 4.4 Provider API (`provider_api/`)

**入口文件**: `mod.rs` — 公开函数:
- `stream_response()` — 分发到原生协议或 JS 插件
- `embed()` — 分发 embedding 请求
- `get_capabilities()`, `get_tool_schema_format()`
- `extract_pending_tool_calls()`, `build_tool_result_input_item()`, `build_user_input_item()`

**关键类型** (`types.rs`):
- `ProviderStreamEvent`: delta, tool_call, error, done 等
- `ResponseStreamRequest`: items, tools, tool_choice, model 等
- `ProviderCapabilities`: requires_instructions, supports_parallel_tool_calls, supports_json_mode 等
- `ReasoningConfig`, `ReasoningEffort`

**注册表** (`registry.rs`):
- `DynamicProviderDefinition`: kind, display_name, config_schema, capabilities, tool_schema_format, model_routing, builtin_models
- `builtin_provider_definitions()` — 支持: openai (Responses + Chat Completions + Embeddings), deepseek (Chat Completions), 以及 google/gemini, anthropic, openrouter, grok, x-ai, together, fireworks, openai-compatible

**协议层** (`protocol/`):
- `Protocol` trait: `stream_response()`, `embed()`
- 内置协议: `responses` (OpenAI Responses API), `chat` (Chat Completions), `embeddings`

**重试 + 熔断器**:
- `RetryStrategy`: exponential backoff + deterministic jitter
- 策略: rate_limit, server_error, network_error, empty_response, no_retry
- `CircuitBreakerRegistry`: 三态 (Closed → Open → HalfOpen → Closed), 每 provider 独立

### 4.5 插件运行时 (`plugin_runtime/`)

**架构**: deno_core JsRuntime 包装器

**插件清单** (`manifest.rs`):
```rust
pub struct PluginManifest {
    pub id: String,            // e.g. "agentjax.provider.deepseek"
    pub name: String,
    pub version: String,
    pub api_version: u32,      // 必须是 1
    pub entrypoint: String,
    pub description: String,
    pub tools: Vec<PluginToolDefinition>,
    pub settings_sections: Vec<...>,
    pub providers: Vec<PluginProviderDefinition>,
    pub sandbox: SandboxPolicy,
}
```

**内置插件** (`src-tauri/builtin-plugins/`):
| 插件 | 提供者 | 说明 |
|------|--------|------|
| `deepseek/` | `deepseek` | 声明式, JS 是存根, 使用原生 Chat Completions |
| `openai/` | `openai` | 声明式, 路由到 Responses/Chat Completions/Embeddings |
| `sdk/` | — | `sdk.js` + `sdk-bootstrap.js` 共享 SDK 模块 |

**关键规则**: providers 有 `model_routing` 或 `builtin_models` 时跳过 JS 提取 (无需创建 JsRuntime)

**沙箱策略**: `SandboxPolicy` — file_read, file_write, network, process_spawn, env_read 权限控制

**工具命名**: `plugin__{sanitized_id}__{sanitized_name}`

### 4.6 工具系统 (`tools/`)

**核心 Trait**:
```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn icon(&self) -> Option<&'static str>;
    fn parameters_schema(&self) -> Value;
    async fn execute(&self, args: &Value, ctx: &ToolExecutionContext) -> AgentJaxResult<Value>;
}
```

**原生工具列表**:

| 工具 | 名称 | 说明 |
|------|------|------|
| CalculatorTool | `calculator` | fend-core 计算器 |
| SystemTimeTool | `get_system_time` | 当前日期/时间 |
| FileReaderTool | `read` | 读取文件 |
| FileWriterTool | `write` | 写入文件 |
| EditFileTool | `edit` | 搜索替换编辑 |
| ListFilesTool | `list_files` | 列出目录 |
| MkdirTool | `make_directory` | 创建目录 |

**上下文工具** (LCM/Memory/KB):
- `MemoryWriteTool`, `MemorySearchTool`, `MemoryRecallTool`
- `KbListTool`, `KbSearchTool`, `KbGetTool`, `KbIndexTool`
- `SubAgentTool` — 生成/状态/取消/批量子代理
- `LcmGrepTool`, `LcmDescribeTool`, `LcmExpandTool` (限于子代理), `LlmMapTool`

**工具命名约定**:
- 原生: 无前缀 (如 `calculator`)
- MCP: `mcp__{server_id}__{tool_name}`
- 插件: `plugin__{plugin_id}__{tool_name}`
- 后台: `background_task` (单一合并工具, action 分发)

**ToolCatalog** (`tools/catalog.rs`):
```rust
pub struct ToolCatalog {
    tools: Vec<ToolEntry>,         // 统一注册表 (Native + Context)
    mcp_manager: Arc<McpManager>,
    mcp_runtime: McpRuntimeConfig,
    mcp_config: BTreeMap<String, McpServerConfig>,
    tool_manager: ToolManagerConfig,
    plugin_manager: PluginManagerConfig,
    plugin_manifests: BTreeMap<String, PluginManifest>,
    plugin_packages: BTreeMap<String, PluginPackage>,
}
```

**两个快照路径**:
1. **模型快照** (`snapshot_with_format_and_mounted_servers()`) — 运行时使用, 决定 AI 模型看到哪些工具
2. **工具管理器快照** (`tool_manager_snapshot()`) — 设置 UI 使用, 决定用户看到/管理哪些工具

**工具启用的层次控制**:
```
native_tools.<name>.enabled (默认 true)
mcp_tools.<server_id>.enabled → mcp_tools.<server_id>.tools.<tool>.enabled
plugin_tools.<plugin_id>.enabled → plugin_tools.<plugin_id>.tools.<tool>.enabled
MCP exposure: unfolded (所有工具可见) vs folded (单个 control tool, 按需挂载)
```

### 4.7 MCP 系统 (`mcp.rs` + `mcp/`)

**核心结构**: `McpManager` — 管理 BTreeMap<String, ManagedService>

**传输类型**:
- **Stdio**: rmcp `TokioChildProcess`, 命令通过 $PATH 解析, kill_on_drop=true
- **Streamable HTTP**: rmcp `StreamableHttpClientTransport`, 支持 auth_header, headers, allow_stateless

**连接生命周期**:
1. `get_peer()` → 检查指纹缓存 → 指纹匹配返回缓存, 不匹配 shutdown+重启
2. 指纹 = McpConnectionSpec 的 JSON 序列化
3. `shutdown_service()` → 3 秒超时优雅关闭

**工具发现**: `list_tools()` → `peer.list_tools(None)` → 挂载到工具目录

**挂载持久化**: 在 `metadata.json` 的 `mounted_mcp_servers` / `mounted_tool_sources` 键下

### 4.8 LCM 系统 (`lcm/`)

**核心**: `LcmEngine` — 上下文控制循环

**SQLite 数据库**: `{conversation_dir}/lcm.db` (WAL 模式)
- 表: `messages`, `summaries`, `summary_children`, `summary_parents`, `file_refs`, `conversation_meta`, `messages_fts` (FTS5)

**三级压缩协议**:
| 级别 | 名称 | 策略 | 需要 LLM |
|------|------|------|----------|
| 1 | Normal | 保留细节 (`preserve_details`) | 是 |
| 2 | Aggressive | 要点 (`bullet_points`) | 是 |
| 3 | Truncation | 确定性截断 (头部~67%, 尾部~33%) | **否** |

**Summary DAG**: 叶子节点指向消息, 压缩节点指向其他 SummaryNode

**LCM 工具**:
- `lcm_grep`: FTS5 正则搜索
- `lcm_describe`: 实体元数据查询
- `lcm_expand`: 展开摘要 (限于子代理)
- `llm_map`: 并行 JSONL 映射操作符

**消费防护 + 熔断器**:
- SpendGuard: 10 分钟窗口内 24 次调用, 30 分钟退避
- CircuitBreaker: 连续 5 次失败后打开 30 分钟

**完整性检查**: 5 项检查 (conversation_exists, summaries_have_lineage, no_orphan_summaries, message_seq_contiguous, context_token_count)

### 4.9 运行时引擎 (`runtime/`)

**`AgentRuntime::run_turn()`** — 主代理循环:

1. **设置**: 解析模型 → provider 能力 → 工具 schema 格式 → 系统项
2. **子代理检测**: 检查 conversation_id 是否包含 `/sub-agent/`
3. **MCP 挂载**: 加载持久化的 mounted servers
4. **工具快照**: 冻结工具可见性
5. **存档**: 存档不可用的历史工具调用
6. **循环** (最多 10 轮):
   - Hop 1: 前缀 (系统项 + 恢复笔记 + street 项) + LCM 历史 + 用户消息
   - 后续 Hops: 仅 LCM 上下文
   - 冻结工具可见性 → 构建请求 → 流式响应 → 提取工具调用
   - 如果 `is_final_hop` (无挂起的工具) → 中断
   - 否则: 调度并执行挂起的工具 → 反馈到循环

**工具执行调度器**: 基于信号量的并行执行 (默认 max 4), 心跳每 5 秒, 超时 300 秒

**工具存档** (`tool_archiving.rs`): 当工具被注销时, 历史 function_call/function_call_output 被转换为 user-role 文本项, 使用 `━━━ Archived Tool Call ━━━` 分隔符

### 4.10 对话存储 (`conversation_store/`)

**持久化格式**: 每个对话两个文件

```
~/.agentjax/agents/{agent_id}/sessions/{conversation_id}/
├── metadata.json      # ConversationMeta (version 6, title, counts, timestamps, dynamic_tools, mounted_mcp_servers, token_usage)
├── messages.jsonl     # JSONL — 每行一个 ConversationLine tag-union
├── lcm.db             # LCM SQLite 数据库
├── workspace/          # 工作区文件
└── notifications.jsonl # Street 通知
```

**ConversationLine** tag-union:
```rust
pub enum ConversationLine {
    User(UserLine { id, ts, request_id, text }),
    Tool(ToolLine { id, ts, started_ts, completed_ts, request_id, call_id, name, args, output, status }),
    Assistant(AssistantLine { id, ts, request_id, response_id, text, status, phase, ... }),
}
```

**锁定机制**: 基于进程内 `Mutex<()>` 注册表, 每对话锁定

### 4.11 Tauri IPC 命令 (`commands/`)

**所有 IPC 命令 (前端 → Rust)**:

| 命令 | 文件 | 说明 |
|------|------|------|
| `chat_stream` | `chat.rs` | 主要聊天流端点 |
| `cancel_chat_stream` | `chat.rs` | 取消活动聊天请求 |
| `list_conversations` | `chat.rs` | 列出所有对话 |
| `load_conversation` | `chat.rs` | 加载对话数据 |
| `load_conversation_dynamic_tools` | `chat.rs` | 获取动态工具 |
| `replace_conversation_dynamic_tools` | `chat.rs` | 替换工具集 |
| `upsert_conversation_dynamic_tool` | `chat.rs` | 添加/更新工具 |
| `remove_conversation_dynamic_tool` | `chat.rs` | 移除工具 |
| `rename_conversation` | `chat.rs` | 重命名对话 |
| `delete_conversation` | `chat.rs` | 删除对话 |
| `list_agents` | `agents.rs` | 列出所有代理 |
| `create_agent` | `agents.rs` | 创建新代理 |
| `delete_agent` | `agents.rs` | 删除代理 |
| `get_settings_snapshot` | `config.rs` | 获取设置 (可选 agent_id) |
| `get_settings_ui_snapshot` | `config.rs` | 获取 UI 设置 (含 sections) |
| `apply_settings_patch` | `config.rs` | 应用设置补丁 |
| `get_tool_manager_snapshot` | `tools.rs` | 工具目录快照 |
| `get_plugin_manager_snapshot` | `tools.rs` | 插件管理器 UI |
| `get_plugin_settings_snapshot` | `tools.rs` | 插件设置数据源 |
| `get_model_catalog` | `models.rs` | 获取模型目录 |
| `force_sync_model_cache` | `models.rs` | 触发远程同步 |
| `open_devtools` | `devtools.rs` | 打开 DevTools |
| `cancel_sub_agent` | `sub_agents.rs` | 取消子代理 |
| `list_sub_agents` | `sub_agents.rs` | 列出子代理 |
| `get_street_items` | `street.rs` | 获取未完成的 Street 项 |
| `dismiss_street_item` | `street.rs` | 解除 Street 项 |
| `list_memories` | `memory.rs` | 列出记忆 |
| `get_memory` | `memory.rs` | 获取记忆内容 |
| `search_memories` | `memory.rs` | 搜索记忆 |
| `delete_memory` | `memory.rs` | 删除记忆 |
| `open_memory_file` | `memory.rs` | 在编辑器中打开 |
| `get_lcm_health` | `lcm_health.rs` | LCM 健康面板 |
| `reset_circuit_breaker` / `reset_spend_guard` | `lcm_health.rs` | LCM 管理 |
| `record_summarization_failure` / `...success` | `lcm_health.rs` | LCM 管理 |

**Tauri 事件 (后端 → 前端)**:
- `chat_stream_event` — 实时流事件
- `config_snapshot_changed` — 设置实时更新

---

## 五、前端 React 模块详解

### 5.1 Hook 清单与职责

| Hook | 文件 | IPC 调用 | 职责 |
|------|------|----------|------|
| `useActiveAgent` | `useActiveAgent.ts` | `list_agents`, `create_agent`, `delete_agent`, `get_settings_snapshot` | Agent 生命周期管理 |
| `useAppConfig` | `useAppConfig.ts` | `get_model_catalog`, `get_settings_snapshot` | 模型目录 + 应用设置 |
| `useChatSessions` | `useChatSessions.ts` | `chat_stream` | 发送消息编排器 |
| `useConversationRegistry` | `useConversationRegistry.ts` | `list_conversations`, `load_conversation`, `rename_conversation`, `delete_conversation` | 对话 CRUD |
| `useConversationStreaming` | `useConversationStreaming.ts` | (监听 `chat_stream_event`) | 实时流事件处理 |
| `useChatComposerState` | `useChatComposerState.ts` | — | 输入文本 + 附件 + 高级选项 |
| `useComposerMeasurements` | `useComposerMeasurements.ts` | — | ResizeObserver 布局测量 |
| `useTitlebarDragging` | `useTitlebarDragging.ts` | — | Tauri 标题栏拖拽 |
| `useDeveloperToolsShortcut` | `useDeveloperToolsShortcut.ts` | `open_devtools` | F12/Cmd+Shift+I |
| `useContextMenuGuard` | `useContextMenuGuard.ts` | — | 右键菜单拦截 |
| `useAnimatedNumber` | `useAnimatedNumber.ts` | — | 数字动画 |

**IPC 调用约定**: `import { invoke } from '@tauri-apps/api/core'` → `invoke<ReturnType>('command_name', { argName: value })`

### 5.2 流事件处理

**事件种类** (从 `chat_stream_event` Tauri 事件收到):

| 种类 | 阶段 | 说明 |
|------|------|------|
| `thinking` | — | 模型开始推理 |
| `thinking_delta` | — | 推理文本增量 |
| `thinking_completed` | — | 推理完成 |
| `output_started` | — | 停止思考 → 开始输出 |
| `delta` | Commentary/FinalAnswer | 逐 token 流式传输 |
| `assistant_message` | Commentary/FinalAnswer | 完整消息块 |
| `tool_call_started` | — | 工具调用开始 |
| `tool_call_done` | — | 工具调用注册 |
| `tool_call_exec` | — | 执行结果 |
| `tool_call_progress` | — | 进度更新 |
| `tool_call_delta` | — | 工具参数增量 |
| `token_usage` | — | 上下文 token 计数更新 |
| `street_notification` | — | 异步工作结果 |
| `done` | — | 请求完成 |

**状态更新函数** (`features/conversations/sessionState.ts`):
- `applyOptimisticUserMessage()`, `applyAssistantDelta()`, `applyThinkingDelta()`
- `applyAssistantMessage()`, `appendPendingToolCall()`, `applyToolExecution()`, `applyToolProgress()`, `applyToolDelta()`
- `applyCompletedRequest()`, `applyLoadedConversationDetail()`, `finalizeLingeringAssistantDrafts()`

**对话分组** (`transcriptGrouping.ts`): 扁平行数组 → 按 `requestId` 分组的 `ConversationTurn` { userLines, workItems, finalLines }

### 5.3 设置 UI Schema 渲染系统

**架构**: `SettingsModal → SettingsRenderer → SchemaRenderer (递归)`
- 字段 → `FieldControlRegistry` → SwitchField, SelectField, TextField, NumberField, TagsField, KeyValueField, JsonField
- group → GroupRenderer, collection → CollectionLayoutRenderer
- ui 节点 → UiLayoutRenderer / DataSourceRenderer / ActionRenderer

**数据源系统**: 插件式 provider 为 schema 节点提供动态数据
- `useToolManagerDataProvider` — 工具目录数据
- `usePluginSettingsDataProvider` — 插件设置数据
- `usePluginManagerDataProvider` — 插件管理器状态
- `useMemoryDataProvider` — 记忆条目

**搜索**: `filterSchemaNodesForSearch()` 递归匹配 id, title, description, dataSource

### 5.4 组件树

```
App.tsx
  <div.app-shell>
    <div.background-gradients />          ← 条件动画辉光
    <Sidebar                              ← conversations, agents, activeAgentId
      AgentSwitcher
      SidebarConversationRow[]
      SidebarActionMenu
    />
    <AppHeader                            ← titlebarRef, modelOptions, reasoningMode
      ChatArea                              ← lines, isGenerating, isThinking
        WorkLogPanel
        markdownRenderer → CodeBlock
    />
    <main>
      <div.composer-stage>                ← 居中/停靠动画
        <h1.welcome-headline />           ← 空状态时显示
        <ChatComposer                     ← input, attachment, onSend
          FullscreenEditorModal
        />
      </div>
    </main>
    <ConfirmModal />                      ← 条件显示
    <LcmHealthModal />                    ← 条件显示
    <SettingsModal />                     ← 条件显示
```

### 5.5 前端特性模块

| 模块 | 文件 | 说明 |
|------|------|------|
| `features/conversations/types.ts` | ~400 行 | ConversationLine tag-union, Conversation, ModelOption, 流事件类型 |
| `features/conversations/sessionState.ts` | ~500 行 | 不可变会话更新程序 |
| `features/conversations/conversationUtils.ts` | 工具函数 | createLocalConversation, buildConversationTurns, getConversationDisplayTitle |
| `features/conversations/conversationState.ts` | ~80 行 | 辅助函数: restoreConversationPreview, ensureAtLeastOneConversation |
| `features/i18n/I18nProvider.tsx` | i18n | 从后端加载语言设置, 监听配置更改, 支持 en/zh/auto |
| `features/models/modelCatalog.ts` | 模型目录 | normalizeModelOption, DEFAULT_MODEL_PROFILE = 'openai/gpt-5-mini' |
| `features/tauri/runtime.ts` | Tauri 防护 | tryGetCurrentWindow, isTauriWindowRuntimeAvailable |
| `features/settings/types.ts` | 设置类型 | SettingsFieldSchema, SettingsSectionSchema 等 |
| `features/settings/configAccess.ts` | 配置 getter | getAppConfigValue<T>, getCollectionItemValue<T> |
| `features/settings/utils.ts` | 路径工具 | getValueAtPath, setValueAtPath, resolvePath |
| `features/icons/lucide.ts` | 图标 | resolveLucideIcon, resolveToolLucideIcon |

---

## 六、代码生成与 Schema

**生成流程**:
1. `cd src-tauri && cargo run --bin gen_schemas` — 使用 `schemars` 从 Rust 类型生成 JSON Schema (输出到 `gen/schemas/`)
2. `node scripts/generate-config-types.mjs` — 从 JSON Schema 生成 TypeScript 类型 (`src/features/settings/__generated__/config-types.ts`)

**关键依赖版本**:
| 依赖 | 版本 | 用途 |
|------|------|------|
| Rust | 1.95.0 | — |
| Tauri | 2.11.2 | 桌面框架 |
| React | 19.2.6 | 前端 |
| TypeScript | 6.0.3 | 前端 |
| Vite | 8.0.12 | 构建 |
| Tailwind CSS | 4.3.0 | 样式 |
| deno_core | 0.402.0 | 插件运行时 |
| rmcp | 1.7.0 | MCP 客户端 |
| reqwest | 0.13.4 | HTTP 客户端 |
| lancedb | 0.30.0 | 向量数据库 |
| rusqlite | 0.40 | SQLite (bundled) |
| tokenizers | 0.23.1 | Token 计数 |
| fend-core | 1.5.8 | 计算器 |

---

## 七、重要模式与注意事项

### 错误处理
- 所有模块使用 `AgentJaxResult<T>` 而非 `Result<T, String>`
- Tauri 命令返回 `Result<T, AgentJaxError>` → 前端收到结构化 `{kind, message, retryable, providerKey}`
- `FnMut(ProviderStreamEvent) -> Result<(), String>` 保留在 Tauri 边界

### 工具存档
- 工具注销时, 历史 function_call 对 → user-role 文本 (用 `━━━ Archived Tool Call ━━━` 分隔)
- 每次运行时新鲜计算, 不修改存储数据
- 工具重新注册时自动恢复 (无需显示取消存档逻辑)

### 配置 I/O
- YAML 文件使用 `serde_yaml`, 先读后写, 写入时使用缩写 (builtin blocks 只保留 id+enabled)
- 有个测试 (`load_config_does_not_rewrite_file_on_startup`) 确保启动时不重写文件

### 前端测试
- 使用 Node 内置 `node:test` runner, 非 Jest/Vitest
- 通过 SSR 将 React 组件渲染为字符串
- 不依赖浏览器环境

### Tauri 窗口特性
- `tauri.conf.json` 中启用 `devtools`
- 窗口管理通过 `@tauri-apps/api/window`
- `__TAURI_INTERNALS__` 检查用于运行时环境检测
