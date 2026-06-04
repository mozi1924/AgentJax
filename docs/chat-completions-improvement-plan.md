# AgentJax Chat Completions 统一 API 改进计划

## 一、现状总览

### 1.1 架构概要

框架采用 **字符串匹配的协议分发** 模式，核心在三个层级：

```
前端 ChatRequest
  → runtime/engine/request.rs (build_request → ResponseStreamRequest 统一请求)
    → provider_api/mod.rs (resolve_protocol 协议分发)
      → protocol/mod.rs (match "responses" | "chat_completions" | "embeddings")
        → protocol/chat.rs 或 protocol/responses.rs (原生 Rust 实现)
      或 → plugin/mod.rs (JS 插件回退路径)
```

**关键发现：框架名义上支持 Chat Completions，但实际上是围绕 Responses API 设计的。** 这体现在多个层面：

| 层面 | Responses API 偏向 | 影响 |
|------|-------------------|------|
| 插件声明 | `toolSchemaFormat: "responses"` | 工具 Schema 格式用 Responses 风格 |
| 协议解析 | `supportsProtocols[0] = "responses"` | 默认走 Responses 协议 |
| 能力声明 | `emitsFinalOutputItems: true` | 声称的 Chat 能力与实际代码不匹配 |
| 统一请求体 | `include`、`text`、`generate` 字段 | 这些是 Responses API 特有字段 |
| 流事件 | 无 `ReasoningStarted` 实际产出 | `ReasoningStarted` 是死代码 |

### 1.2 Chat Completions 适配器现状

`src-tauri/src/provider_api/protocol/chat.rs` 仅 316 行，是一个**最小可行实现**。

**已实现**：
- 基础文本流式响应 (SSE `delta.content`)
- 工具调用流式传输 (SSE `delta.tool_calls`，基于 index 跟踪)
- `reasoning_effort` 参数传递
- `response_format: { type: "json_object" }` 映射
- `stream_options: { include_usage: true }` 用量统计
- 系统消息/指令的转换（`instructions_override` → `system` role）

---

## 二、缺失功能清单

### 🔴 P0 — 关键缺失（影响基本可用性）

| # | 缺失功能 | 位置 | 影响 |
|---|---------|------|------|
| 1 | **思考/推理内容流式传输** | chat.rs:233 只检查 `delta.content`，忽略 `delta.reasoning_content` | DeepSeek-R1、OpenAI o-series、任何 reasoning 模型的思考过程全部丢失 |
| 2 | **ReasoningStarted 事件** | chat.rs 完全不发，但 types.rs:42 已定义，chat_events.rs:77 已消费 | 前端 `thinking` 动画永不触发 |
| 3 | **JSON Schema 结构化输出** | chat.rs:147-153 仅处理 `json_object`，不支持 `json_schema` + `strict` | 无法使用结构化 JSON 输出 |
| 4 | **多模态消息** | chat.rs:185-188 将 content 摊平为纯文本，丢弃图片/音频 | 无法使用视觉模型、音频模型 |

### 🟡 P1 — 重要缺失（影响高级功能）

| # | 缺失功能 | 说明 |
|---|---------|------|
| 5 | 采样参数 | `temperature`、`top_p`、`presence_penalty`、`frequency_penalty` 全未实现 |
| 6 | max_tokens / max_completion_tokens | 无法限制生成长度 |
| 7 | finish_reason 完整处理 | `content_filter` 等终止原因不处理 |
| 8 | 中间用量事件 | `UsageUpdated` 仅在最终结果中返回，不实时推送 |
| 9 | 开发者角色消息 | 新版 OpenAI API 的 `developer` role 不支持 |
| 10 | 多内容块消息 | 输入消息被摊平为纯文本，丢失多模态结构 |

### 🟢 P2 — 改进项（锦上添花）

| # | 缺失功能 | 说明 |
|---|---------|------|
| 11 | logprobs / top_logprobs | 概率输出 |
| 12 | stop sequences | 自定义停止序列 |
| 13 | tool_choice 灵活化 | 当前硬编码 `"auto"`，无 UI 控制 |
| 14 | service_tier | 字段存在但不写入 Chat 负载 |

---

## 三、架构改进路线图

### 核心理念

> **框架是一套统一 API，各模型接入接口仅需将其路由转换为框架统一接口。**

当前问题在于 `ProviderTurnRequest`（统一请求）和 `ProviderStreamEvent`（统一事件）的设计**天然偏向 Responses API**。改进的目标是让这套统一抽象**真正协议无关**。

### 3.1 第一阶段：补齐 Chat Completions 协议适配器（P0）

#### 3.1.1 实现思考/推理内容流式传输

**问题**：Chat Completions API 的 `delta.reasoning_content`（DeepSeek）和 `delta.thinking` 字段完全被忽略。

**涉及文件**：
- chat.rs:229-290 — SSE 事件处理
- types.rs:42 — `ReasoningStarted` 变体
- chat_events.rs:77-79 — 前端事件映射

**方案**：
1. 在 `ProviderStreamEvent` 中增加 `ReasoningDelta { delta: String }` 变体
2. Chat Completions 适配器检测 `delta.reasoning_content` 并发出 `ReasoningStarted` + `ReasoningDelta`
3. 推理内容用特殊标记在前端折叠显示（可展开的"思考过程"）

**需考虑的 API 差异**：

| 提供商 | 请求参数 | 流式字段 | 说明 |
|--------|---------|---------|------|
| OpenAI o-series | `reasoning_effort` (top-level) | `delta.reasoning_content` | 推理内容在 Chat Completions 流中 |
| DeepSeek-R1 | 无额外参数 | `delta.reasoning_content` | 自动进入深度思考模式 |
| Anthropic (扩展思考) | `thinking: { type: "enabled", budget_tokens: N }` | 独立 thinking 事件 | 不走 Chat Completions 路径 |

#### 3.1.2 实现 JSON Schema 结构化输出

**问题**：chat.rs:147-153 只支持 `json_object` 模式。

**方案**：扩展 `build_chat_payload` 的 response_format 逻辑以支持完整的 `json_schema` 格式：
- `{ type: "json_schema", json_schema: { name, schema, strict } }`

#### 3.1.3 实现多模态消息支持

**问题**：chat.rs:185-188 将所有 content 摊平为文本串。

**方案**：重构 `input_items_to_messages` 保留完整的 content 结构（支持 image_url 等）。

### 3.2 第二阶段：统一请求体的协议无关化（P0-P1）

#### 3.2.1 ProviderTurnRequest 语义清理

将字段分为三层：
1. **通用层**：所有协议都必须处理的字段
2. **协议扩展层**：通过 `extensions: HashMap<String, Value>` 传递协议特有参数
3. **采样参数层**：`temperature`、`top_p` 等

#### 3.2.2 采样参数标准化

在 `ProviderTurnRequest` 中增加：
- `temperature`, `top_p`, `presence_penalty`, `frequency_penalty`
- `max_tokens`, `max_completion_tokens`, `stop`
- `reasoning_budget_tokens`

### 3.3 第三阶段：流事件系统的完善（P0-P1）

#### 3.3.1 新增 ReasoningDelta 流事件

```rust
pub enum ProviderStreamEvent {
    ReasoningStarted,              // 已有，但从未被发出 — 需修复
    ReasoningDelta {               // 🆕 推理内容增量
        delta: String,
    },
    ReasoningCompleted {           // 🆕 推理完成（可选）
        total_tokens: Option<usize>,
    },
    // ... 其余不变
}
```

#### 3.3.2 前端 Reasoning 展示

前端已有 `thinking` 状态管理（useConversationStreaming.ts:53、ChatArea.tsx:186），但从未收到 `ReasoningStarted` 事件。只需**后端产出事件即可**激活现有前端代码。

### 3.4 第四阶段：协议路由的智能化（P1-P2）

#### 3.4.1 按模型自动选择协议

在插件中增加智能检测，Chat Completions 本地模型和兼容 API 自动走 Chat Completions。

#### 3.4.2 工具 Schema 格式与协议自动匹配

当前 `toolSchemaFormat` 硬绑定到 `"responses"`。应该与当前使用的协议保持一致。

---

## 四、分阶段实施路径

### 阶段 1：Chat Completions 核心能力补齐（P0）

| # | 任务 | 涉及文件 | 优先级 |
|---|------|---------|--------|
| 1 | 实现 ReasoningStarted + ReasoningDelta 事件产出 | protocol/chat.rs, types.rs, chat_events.rs | 🔴 P0 |
| 2 | 实现 json_schema + strict 结构化输出 | protocol/chat.rs | 🔴 P0 |
| 3 | 实现多模态消息（image_url）支持 | protocol/chat.rs (input_items_to_messages) | 🔴 P0 |
| 4 | 添加 max_tokens/temperature/top_p 等采样参数 | protocol/chat.rs, types.rs, runtime/engine/request.rs | 🟡 P1 |
| 5 | 处理 finish_reason: content_filter | protocol/chat.rs | 🟡 P1 |

### 阶段 2：统一 API 抽象层净化（P1）

| # | 任务 | 涉及文件 |
|---|------|---------|
| 1 | 在 ProviderTurnRequest 中增加通用采样参数字段 | types.rs, runtime/engine/request.rs, commands/chat/chat_types.rs |
| 2 | 增加 reasoning_budget_tokens 支持 | types.rs, chat.rs, responses.rs |
| 3 | 将 Responses 特有字段标记为协议扩展 | types.rs |

### 阶段 3：协议路由智能化（P2）

| # | 任务 | 涉及文件 |
|---|------|---------|
| 1 | 插件中实现智能协议选择 | builtin-plugins/openai/plugin.js |
| 2 | 工具 Schema 格式跟随协议自动切换 | runtime/engine.rs |
| 3 | 完善能力声明以准确反映各协议实际支持 | capabilities.rs, plugin.js |

---

## 五、关键设计原则

### 5.1 统一 API 分层

```
┌─────────────────────────────────────────────┐
│                前端 UI 层                     │
└─────────────────┬───────────────────────────┘
                  │
┌─────────────────▼───────────────────────────┐
│           统一请求层 (ProviderTurnRequest)     │
└─────────────────┬───────────────────────────┘
                  │ resolve_protocol()
┌─────────────────▼───────────────────────────┐
│            协议适配层                         │
│  Responses 适配器 / Chat Completions 适配器   │
│                    / Plugin 适配器            │
│         │              │               │      │
│         ▼              ▼               ▼      │
│    统一流事件 (ProviderStreamEvent)           │
└─────────────────────────────────────────────┘
```

### 5.2 每个协议的职责

- **接收统一请求**：从 ProviderTurnRequest 提取自身需要的参数
- **转换为协议格式**：Responses → POST /responses，Chat → POST /chat/completions
- **产出统一事件**：所有流式事件归一化为 ProviderStreamEvent
- **返回统一结果**：ResponseStreamResult 保持协议无关

### 5.3 向后兼容策略

- 不删除现有字段，新增字段使用 Option 类型
- 现有的 text、include、generate 字段保留但标记为 Responses 协议优先
- 新增的采样参数在 Chat Completions 适配器中优先使用，Responses 适配器忽略不支持的参数
- 协议扩展通过 HashMap<String, Value> 转发，不破坏现有结构
