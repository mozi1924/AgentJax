# AgentJax Phase 1: Local MCP-Backed Tool Calling Loop

## Summary

把 AgentJax 从“可流式聊天的 Responses 客户端”升级为“本地执行 tool loop 的 Agent runtime”，并且明确分成三层：

- **Agent runtime core**：统一管理 turn、tool loop、continuation、事件流和持久化。
- **Provider adapter**：只处理不同 Responses 风格 provider 的请求/流式事件差异，不负责工具执行。
- **Local tool layer**：把内建工具和本地 MCP `stdio` server 暴露的 tools 统一桥接成 function tools，供任意兼容 Responses 风格的上游使用。

当前状态补充（2026-05-28）：

- 内建 `calculator` 已统一为 `fend-core` 单引擎实现
- 旧符号模式（`simplify/differentiate/integrate/solve/solve_system/limit`）已从工具契约中移除
- 细节见 [Calculator engine status](calculator-fend-core.md)

这一阶段不依赖 OpenAI 的远程 `mcp` 内建 tool，而是完全本地处理 MCP，以兼容 Codex 风格网关和其他非官方 Responses 实现。设计对齐官方 Responses function calling 与 MCP 协议语义：
[Function calling](https://developers.openai.com/api/docs/guides/function-calling),
[Conversation state](https://developers.openai.com/api/docs/guides/conversation-state),
[MCP intro](https://modelcontextprotocol.io/introduction),
[MCP lifecycle](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle).

## Key Changes

### 1. Backend architecture

新增一个 provider-agnostic 的 agent runtime，替换当前“provider 里顺手执行工具”的实现：

- 新建 `AgentTurnRequest / AgentTurnEvent / AgentTurnResult / AgentContinuation` 核心类型。
- `providers/*` 只负责：
  - 发送 Responses 请求
  - 解析流式 event
  - 输出标准化后的 `message delta / tool_call delta / item done / response completed / error`
- 把当前 `openai_responses.rs` 里的 `ToolRegistry.execute(...)`、continuation 拼接逻辑上提到 runtime。
- runtime 收到 `tool_call_done` 后统一执行本地工具，再构造下一跳 `function_call_output` input item，继续同一轮循环，直到得到最终 assistant message 或错误。

关键约束：

- **所有 provider** 统一走“本地输入项重放 + 本地 tool loop continuation”，不依赖云端 `previous_response_id`。
- **Codex-style provider** 保持 `store=false` 与流式 item 驱动解析，不依赖 completed payload 的 `response.output`。
- **OpenAI-standard provider** 保持与官方 Responses 请求/事件兼容，但 continuation 仍以本地 item 驱动为主路径。
- provider 返回的 canonical 完成 item 统一以 `output item` 为准，而不是只信最终 completed payload。

### 2. Local MCP integration

引入本地 MCP host/client 管理层，仅支持 **`stdio` transport + tools/list + tools/call**：

- 新增 `McpServerConfig`：
  - `id`
  - `command`
  - `args`
  - `env`
  - `cwd`
  - `startup_timeout_ms`
  - `tool_timeout_ms`
  - `enabled`
  - `allowed_tools`
- 在应用启动或首次使用时建立 `McpManager`，负责：
  - 启动/复用本地 MCP server 进程
  - 执行 MCP initialize + capability negotiation
  - 拉取 `tools/list`
  - 执行 `tools/call`
  - 管理生命周期、超时、stderr/退出状态、重连
- 只接入 MCP 的 **tools** 能力；`resources`、`prompts`、`sampling`、`elicitation` 暂不进入 Phase 1。
- 把 MCP tools 归一化为内部 function tools，命名固定为 `mcp__<server_id>__<tool_name>`，并保存一张 reverse map 用于执行时反查 server/tool。
- 保留现有内建 `ToolRegistry`，但升级成统一 `ToolCatalog`：
  - `native` tools
  - `mcp` tools
  - 后续可扩展 `plugin`/`workspace` tools
- provider 层永远只看到标准 function tools，不直接知道 MCP。

### 3. Config and interfaces

扩展配置与内部接口，避免后续返工：

- 在 [config/mod.rs](/Volumes/Data/AgentJax/src-tauri/src/config/mod.rs) 所在配置模型中新增 `mcp_servers` 顶层配置。
- `ResponseStreamRequest` 不再直接承担 agent loop 全部语义；新增更高层 request/result 类型，由 chat command 调用 runtime，再由 runtime 调 provider。
- `ProviderStreamEvent` 扩充为可表达：
  - `response_started`
  - `message_item_started/delta/done`
  - `tool_call_started/args_delta/done`
  - `tool_output_submitted`
  - `loop_iteration_completed`
  - `provider_warning`
  - `response_completed`
- conversation 持久化升级到下一版记录格式，assistant entry 保存：
  - 最终文本
  - 完整 output items
  - tool timeline/event log
  - provider continuation metadata
- 保持旧会话可读；旧记录缺少 event log 时按“仅文本消息”降级展示。

### 4. Frontend UX

前端第一阶段做“时间线 + 可展开详情”：

- 每条 assistant message 下展示一个 execution timeline，至少包含：
  - thinking started
  - tool selected
  - arguments streaming
  - tool executing
  - tool completed / failed
  - final response
- 对 MCP tool 明确展示来源：
  - server label / server id
  - tool name
  - 参数
  - 输出
  - 错误
- 保留当前富文本消息主体，但把 tool widget 从“附属数组”升级为“按事件顺序组织的 timeline blocks”。
- 页面刷新或重新加载历史会话时，能从持久化记录完整恢复 tool/MCP 展示，而不是只恢复最终文本。
- 错误态要区分：
  - provider 失败
  - tool 执行失败
  - MCP server 启动失败
  - MCP call timeout / protocol error

## Test Plan

需要补齐以下测试与验收场景：

- provider adapter 单测：
  - Codex 风格流式 `function_call_arguments.delta/done`
  - `response.output_item.done` 为主输出来源
  - same-socket continuation
- runtime 单测：
  - 单工具调用完成
  - 同轮多工具调用
  - 工具失败后将错误字符串作为 `function_call_output` 回传
  - 循环多跳后得到最终回答
- MCP 集成测试：
  - 启动一个本地 `stdio` MCP feature/test server
  - 成功完成 initialize + tools/list + tools/call
  - server crash / timeout / malformed JSON-RPC 的错误处理
- conversation store 测试：
  - 保存并恢复完整 tool timeline
  - 旧格式记录兼容读取
- 前端验收：
  - 流式参数增量实时可见
  - 刷新后历史 tool 轨迹可恢复
  - 失败状态在 UI 上可区分且不吞错
  - 切换 conversation 不打断进行中的其他会话展示状态

## Assumptions

- Phase 1 只做 **本地 `stdio` MCP servers + tools**，不做 HTTP/SSE transport，不做 resources/prompts。
- MCP 不走 OpenAI `type: "mcp"` 远程内建 tool；所有 provider 一律走本地 function tool bridge。
- 会话持久化保存 **完整 tool/MCP 轨迹**，作为后续调试、多 provider 对齐和 UI 重建的基础。
- 前端第一阶段目标是 **可观测性优先**，不是最简洁 UI。
- 如无额外要求，MCP server 配置放入现有 `config.yaml`，不单独新建第二套配置系统。
