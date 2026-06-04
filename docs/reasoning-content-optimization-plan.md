# 推理内容（思维链）优化方案

> **状态**: 分析完成，待实施
> **日期**: 2026-06-04
> **范围**: 全栈（Rust 后端 + TypeScript 前端 + SQLite 数据库 + JSONL 备份）

---

## 一、背景与目标

### 1.1 问题陈述

带有推理能力的模型（DeepSeek R1、DeepSeek V3.2、OpenAI o 系列等）会在最终输出前产生大量思维链（Chain of Thought / Reasoning Content）。这些思维链内容通常：

- **非常长**（几百到几千 tokens）
- **噪音多**（模型的内部推敲过程）
- **仅本轮有价值**（往期推理对后续对话几乎无意义）

如果将所有历史推理内容全部塞入上下文窗口，会严重浪费宝贵的 LCM 上下文预算，加速压缩触发，降低整体对话质量。

### 1.2 设计目标

| 目标 | 说明 |
|------|------|
| **上下文隔离** | 仅本轮对话的推理内容进入上下文窗口，往期推理全部剥离 |
| **完整保留** | 所有推理内容在数据库和 JSONL 备份中完整保存，支持用户溯源和研究 |
| **前端可展示** | 历史对话的推理内容可以在前端折叠展示（类似当前流式展示） |
| **存储分离** | 推理内容与实际消息正文分开存储，互不污染 |

---

## 二、当前架构全面审计

### 2.1 数据流全景

```
Provider API (Chat Completions / Responses)
    │
    │  reasoning_content 字段 / reasoning 事件
    ▼
ProviderStreamEvent 枚举
    │
    ├── ReasoningStarted      ──► 前端 "thinking" 事件
    ├── ReasoningDelta        ──► 前端 "thinking_delta" 事件
    ├── ReasoningCompleted    ──► 前端 "thinking_completed" 事件
    │
    ▼
output_items (ResponseStreamResult)
    │
    │  ⚠️ 推理内容**未**写入 output_items
    │
    ▼
LCM StoredMessage (content 字段)
    │
    │  ⚠️ 推理内容**未**持久化到 LCM
    │
    ▼
messages.jsonl (ConversationLine 序列化)
    │
    │  ⚠️ AssistantLine 无 thinking 字段
    │
    ▼
上下文重建 (context/builders.rs + lcm/engine.rs)
    │
    │  ⚠️ 推理内容**未**包含在重建的上下文中
    │
    ▼
下一轮请求 (不含任何历史推理)
```

### 2.2 各层现状详析

#### 2.2.1 Provider 协议层

**Chat Completions 协议** — [`src-tauri/src/provider_api/protocol/chat.rs:288-367`](src-tauri/src/provider_api/protocol/chat.rs#L288-L367)

```
流式解析 delta.reasoning_content ──► 发出 ReasoningStarted/ReasoningDelta 事件
                                    └─ ✅ 前端实时展示正常
                                    └─ ❌ 推理内容未写入 output_items
```

- **问题**: 当 `finish_reason` 到达时，`output_items` 中只有 `function_call` 类型的条目（L359），没有 `{"type": "reasoning", "text": "..."}` 条目
- **后果**: 推理内容作为事件流到前端后即被丢弃，无法被后续持久化链路捕获

**Responses API 协议** — [`src-tauri/src/provider_api/protocol/responses.rs:167-262`](src-tauri/src/provider_api/protocol/responses.rs#L167-L262)

```
OpenAI Responses API 可能发出的推理事件:
  response.reasoning.delta      ──► ❌ 完全未处理
  response.reasoning.completed  ──► ❌ 完全未处理
  response.reasoning.summary    ──► ❌ 完全未处理
```

- **问题**: Responses API 协议的推理事件处理完全缺失
- **后果**: 通过 Responses API 路径的推理内容完全丢失，前端也看不到

**Provider 插件路径** — [`src-tauri/src/provider_api/plugin/mod.rs`](src-tauri/src/provider_api/plugin/mod.rs)

- JS 插件需要自行发出 `ReasoningStarted`/`ReasoningDelta`/`ReasoningCompleted` 事件
- 内置插件目前未实现推理事件

#### 2.2.2 ProviderStreamEvent 枚举 — [`src-tauri/src/provider_api/types.rs:32-112`](src-tauri/src/provider_api/types.ts#L32-L112)

```rust
// ✅ 事件定义完整
ReasoningStarted
ReasoningDelta { delta: String }
ReasoningCompleted { total_tokens: Option<usize> }
```

事件定义本身是完整的，问题在于这些事件**仅用于流式传输**，缺少将推理内容持久化到结构化结果的机制。

#### 2.2.3 运行时引擎 — [`src-tauri/src/runtime/engine.rs:310-383`](src-tauri/src/runtime/engine.rs#L310-L383)

**LCM 持久化路径**:
```rust
// engine.rs L331-348: 从 output_items 提取助手消息
for (text, phase) in &hop_messages_for_lcm {
    // 创建 StoredMessage { content: text, ... }
    // ❌ 推理内容不在 hop_messages_for_lcm 中
}
```

**`extract_assistant_messages_from_items`** — [`src-tauri/src/runtime/engine/output.rs:42-74`](src-tauri/src/runtime/engine/output.rs#L42-L74):

```rust
// 仅提取 type == "message" 且 role == "assistant" 的项
// ❌ 跳过 type == "reasoning" 的项（即使未来有此类型）
```

**TurnAccumulator** — [`src-tauri/src/runtime/engine/turn.rs:24-39`](src-tauri/src/runtime/engine/turn.rs#L24-L39):

```rust
// record_hop 展开 output_items 中的所有项
// 注释（L43-54）表明期望推理项在 output_items 中
// ❌ 但 output_items 中从未创建推理项
```

#### 2.2.4 会话存储层

**`AssistantLine` 结构体（Rust 后端）** — [`src-tauri/src/conversation_store/types.rs:220-234`](src-tauri/src/conversation_store/types.ts#L220-L234):

```rust
pub struct AssistantLine {
    pub id: String,
    pub ts: i64,
    pub request_id: String,
    pub response_id: String,
    pub phase: Option<AssistantPhase>,
    pub text: String,                    // ✅ 有 text
    pub status: AssistantStatus,
    // ❌ 无 thinking 字段
}
```

**`AssistantLine` 接口（TypeScript 前端）** — [`src/features/conversations/types.ts:33-45`](src/features/conversations/types.ts#L33-L45):

```typescript
export interface AssistantLine {
  kind: 'assistant';
  // ... 其他字段
  text: string;
  thinking?: string;  // ✅ 有 thinking 字段
}
```

**结构不一致**: 后端有 `thinking` 的缺失导致序列化/反序列化时推理内容丢失。

**JSONL 备份格式** — [`src-tauri/src/conversation_store/file_io.rs:113-121`](src-tauri/src/conversation_store/file_io.rs#L113-L121):

```rust
// messages.jsonl 每行是一个 ConversationLine 的 JSON
// ConversationLine::Assistant(AssistantLine { ... })
// ❌ AssistantLine 序列化时不包含 thinking
```

#### 2.2.5 LCM 存储层

**`StoredMessage` 结构体** — [`src-tauri/src/lcm/types.rs:86-125`](src-tauri/src/lcm/types.rs#L86-L125):

```rust
pub struct StoredMessage {
    pub id: MessageId,
    pub conversation_id: String,
    pub role: MessageRole,
    pub content: String,                                  // 仅正文
    pub token_count: u32,
    pub timestamp_unix_ms: i64,
    pub covered_by: Option<SummaryId>,
    pub metadata: BTreeMap<String, Value>,                 // 可扩展
    pub file_refs: Vec<FileRefId>,
    // ❌ 无 thinking/reasoning 字段
}
```

**SQLite Schema** — [`src-tauri/src/lcm/store.rs:1077-1090`](src-tauri/src/lcm/store.rs#L1077-L1090):

```sql
CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'tool')),
    content TEXT NOT NULL,
    token_count INTEGER NOT NULL DEFAULT 0,
    timestamp_unix_ms INTEGER NOT NULL,
    covered_by TEXT,
    search_text TEXT NOT NULL DEFAULT '',
    metadata_json TEXT NOT NULL DEFAULT '{}',
    file_refs_json TEXT NOT NULL DEFAULT '[]'
    -- ❌ 无 thinking_text 列
);
```

**LCM → ConversationLine 转换** — [`src-tauri/src/lcm/mod.rs:192-219`](src-tauri/src/lcm/mod.rs#L192-L219):

```rust
types::MessageRole::Assistant => {
    Some(ConversationLine::Assistant(AssistantLine {
        text: msg.content.clone(),  // ❌ 无 thinking 映射
        // ...
    }))
}
```

#### 2.2.6 上下文重建层

**`build_assistant_input_item`** — [`src-tauri/src/conversation_store/context/builders.rs:44-61`](src-tauri/src/conversation_store/context/builders.rs#L44-L61):

```rust
fn build_assistant_input_item(line: &AssistantLine) -> Value {
    json!({
        "type": "message",
        "role": "assistant",
        "content": [{
            "type": "output_text",
            "text": render_timed_message("Assistant message", line.ts, &line.text),
            // ❌ 无 reasoning 内容
        }]
    })
}
```

**`context_to_provider_items`** — [`src-tauri/src/lcm/engine.rs:906-1024`](src-tauri/src/lcm/engine.rs#L906-L1024):

```rust
// RawMessage case (L911-984):
// ✅ 处理 function_call / function_call_output 元数据
// ❌ 无 reasoning 类型的处理逻辑
```

#### 2.2.7 前端层

**流式事件处理** — [`src/hooks/useConversationStreaming.ts:227-247`](src/hooks/useConversationStreaming.ts#L227-L247):

```
thinking        → 标记为思考中
thinking_delta  → 追加到 draftLine.thinking
thinking_completed → (无操作)
output_started  → 清除思考标记
```

**思考增量应用** — [`src/features/conversations/sessionState.ts:310-341`](src/features/conversations/sessionState.ts#L310-L341):

```typescript
// ✅ applyThinkingDelta 正确追加到 draftLine.thinking
// ❌ applyLoadedConversationDetail 重新加载时会覆盖 thinking
// ❌ applyCompletedRequest 完成时 thinking 可能丢失
```

**思考展示** — [`src/components/ChatArea.tsx:245-260`](src/components/ChatArea.tsx#L245-L260):

```tsx
// ✅ 使用 <details> 折叠展示，draft 时默认展开
```

#### 2.2.8 令牌计数

**`build_chat_completion_messages`** — [`src-tauri/src/conversation_store/context/token_usage/messages.rs:71-80`](src-tauri/src/conversation_store/context/token_usage/messages.rs#L71-L80):

```rust
// ✅ 已处理 "type": "reasoning" 项
// ✅ 通过 extract_reasoning_summary 提取摘要文本
// ⚠️ 但此为"未来保证"——output_items 中尚无推理项
```

---

## 三、差距汇总

| # | 差距 | 严重程度 | 影响 |
|---|------|---------|------|
| G1 | Chat Completions 协议：`reasoning_content` 未写入 `output_items` | 🔴 高 | 推理内容无法被后续持久化链路捕获 |
| G2 | Responses API 协议：推理事件完全未处理 | 🔴 高 | Responses API 路径推理内容完全丢失 |
| G3 | 后端 `AssistantLine` 无 `thinking` 字段 | 🔴 高 | JSONL 备份和前后端同步丢失推理 |
| G4 | LCM `StoredMessage` 无推理字段 | 🟡 中 | LCM 存储无法保存推理 |
| G5 | LCM SQLite `messages` 表无推理列 | 🟡 中 | 数据库层面无推理存储能力 |
| G6 | 上下文重建不包含推理 | 🟢 低 | 历史推理不进上下文（这其实是期望行为） |
| G7 | 前端加载历史对话时 `thinking` 丢失 | 🟡 中 | 用户无法查看历史推理 |
| G8 | LCM 压缩管道无推理感知 | 🟢 低 | 推理不进压缩管道（期望行为） |

---

## 四、优化方案设计

### 4.1 核心设计原则

```
┌─────────────────────────────────────────────────────────────┐
│                      设计原则                                │
│                                                             │
│  1. 存储分离   推理内容 ↔ 消息正文 分开存储                    │
│  2. 上下文隔离  仅本轮推理可进入上下文，历史推理剥离             │
│  3. 完整保留   所有推理内容永久保留在 DB + JSONL 中            │
│  4. 可追溯     前端可按需展开任意历史消息的推理过程             │
│  5. 向后兼容   旧对话（无推理数据）不受影响                     │
└─────────────────────────────────────────────────────────────┘
```

### 4.2 存储架构设计

#### 4.2.1 新增 LCM 数据库表：`reasoning_chains`

```sql
-- 推理链表：独立存储每轮的思维链内容
CREATE TABLE IF NOT EXISTS reasoning_chains (
    id TEXT PRIMARY KEY,                          -- 唯一标识
    conversation_id TEXT NOT NULL,                -- 所属对话
    request_id TEXT NOT NULL,                     -- 所属请求
    response_id TEXT NOT NULL,                    -- 所属响应
    message_id TEXT NOT NULL,                     -- 关联的 assistant StoredMessage.id
    
    thinking_text TEXT NOT NULL,                  -- 完整推理内容
    thinking_token_count INTEGER NOT NULL DEFAULT 0,  -- 推理内容 token 数
    
    -- 可选的压缩摘要（用于前端预览）
    summary_text TEXT,                            -- 推理摘要（短）
    summary_token_count INTEGER NOT NULL DEFAULT 0,
    
    created_at_unix_ms INTEGER NOT NULL,          -- 创建时间
    
    FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_reasoning_conv
    ON reasoning_chains(conversation_id, created_at_unix_ms);

CREATE INDEX IF NOT EXISTS idx_reasoning_msg
    ON reasoning_chains(message_id);
```

**设计考量**:
- `thinking_text` 存储完整推理原文
- `summary_text` 存储可供前端预览的简短摘要（可后续通过 LLM 生成，或简单截断前 N 字符）
- `message_id` 关联到 messages 表中的助手消息，确保引用完整性
- 独立表设计而非 messages 表加列，因为：
  - 推理内容通常很长，独立存储避免影响 messages 表的查询性能
  - 大多数消息（user、tool、无推理能力的模型）不需要此字段
  - 方便未来扩展（如推理版本、推理类型等）

#### 4.2.2 新增 LCM `StoredMessage` 字段

```rust
pub struct StoredMessage {
    // ... 现有字段保持不变 ...
    
    /// 关联的推理链 ID（如果有推理内容）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_id: Option<String>,
}
```

轻量级引用，不将完整推理内容嵌入 StoredMessage。

#### 4.2.3 新增后端 `AssistantLine` 字段

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantLine {
    // ... 现有字段保持不变 ...
    
    /// 本轮推理/思维链内容（完整文本）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    
    /// 推理 token 数量
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_token_count: Option<u32>,
}
```

**向后兼容**: 使用 `skip_serializing_if = "Option::is_none"`，旧数据不受影响。

#### 4.2.4 JSONL 备份格式变更

`messages.jsonl` 中的 `AssistantLine` 将自然包含 `thinking` 字段：

```jsonl
{"kind":"assistant","id":"msg_001","ts":1717500000,"requestId":"req_001","responseId":"resp_001","phase":"final_answer","text":"根据分析，答案是42。","status":"done","thinking":"首先，我需要分析问题的各个方面...（完整思维链）...综上，42是正确的。","thinkingTokenCount":450}
```

**优势**:
- 完整推理内容直接嵌入 JSONL，用户可直接查看
- 与上下文内容在同一个文件中但不进入上下文重建流程
- 导入/导出时完整保留

### 4.3 数据流设计

#### 4.3.1 写入路径（新）

```
Provider API 流式响应
    │
    ├── reasoning_content ──► ReasoningStarted/Delta/Completed 事件 ──► 前端实时展示
    │                                                                    │
    │                         ┌──────────────────────────────────────────┘
    │                         ▼
    │                    累积 reasoning_buffer: String
    │                         │
    ├── content ──► OutputTextDelta 事件 ──► output_text 累积
    │                                         │
    ▼                                         ▼
ResponseStreamResult                     output_items.push({
    .output_items                             "type": "message",
    .output_text                              "role": "assistant",
    .reasoning_text (新增)                    "content": [...]
    .reasoning_tokens (新增)              })
                                              output_items.push({
                                                  "type": "reasoning",
                                                  "text": reasoning_buffer,
                                                  "summary": truncate(reasoning_buffer, 200)
                                              })
```

**关键变更**:
1. `ResponseStreamResult` 新增 `reasoning_text: Option<String>` 和 `reasoning_tokens: Option<usize>` 字段
2. Chat Completions 协议在 `finish_reason` 时将推理内容写入 `output_items`
3. Responses API 协议新增 `reasoning.*` 事件处理

#### 4.3.2 LCM 持久化路径

```
output_items
    │
    ├── type=="message" ──► StoredMessage { content: text, reasoning_id: Some("rc_xxx") }
    │
    ├── type=="reasoning" ──► ReasoningChain {
    │                            id: "rc_xxx",
    │                            message_id: "msg_xxx",
    │                            thinking_text: "...",
    │                            ...
    │                        }
    │
    └── type=="function_call" ──► 同现有逻辑
```

**运行时引擎变更** ([`engine.rs:320-383`](src-tauri/src/runtime/engine.rs#L320-L383)):
- 在 `hop_messages_for_lcm` 循环之外，检查 `output_items` 中的 `type=="reasoning"` 项
- 创建 `ReasoningChain` 并存储到 LCM
- 将 `reasoning_id` 关联到对应的 `StoredMessage`

#### 4.3.3 上下文重建路径

**核心原则**: 上下文重建时**仅包含当前请求的推理内容**，历史推理全部剥离。

```rust
// context/builders.rs — build_assistant_input_item
fn build_assistant_input_item(line: &AssistantLine) -> Value {
    let mut item = json!({
        "type": "message",
        "role": "assistant",
        "status": "completed",
        "content": [{
            "type": "output_text",
            "text": render_timed_message("Assistant message", line.ts, &line.text),
            "annotations": []
        }]
    });
    
    // ✅ 仅当是本轮消息时，才将 reasoning 纳入上下文
    // 由调用方通过参数控制 is_current_turn
    // ❌ 历史消息的 line.thinking 不进入上下文
    
    item
}
```

**设计决策**: 
- 上下文重建函数不主动包含历史推理
- 只有当前轮次（运行时的最新响应）的推理内容在发送给模型时附带
- 这符合用户需求：仅本轮推理进入上下文

#### 4.3.4 前端加载路径

```
对话加载
    │
    ├── 流式（实时）: thinking 通过 thinking_delta 事件累积到 draftLine.thinking
    │
    └── 历史加载: AssistantLine 中包含 thinking 字段
         │
         ▼
    applyLoadedConversationDetail()
         │
         └── ✅ 保留 line.thinking 用于折叠展示
```

**前端变更** ([`sessionState.ts`](src/features/conversations/sessionState.ts)):
- `applyLoadedConversationDetail` 和 `applyCompletedRequest` 保留 `thinking` 字段
- 已完成的消息（`status: 'done'`）的推理默认折叠，草稿消息的推理默认展开

### 4.4 压缩管道变更

**原则**: 推理内容**不参与** LCM 压缩。

理由：
1. 推理内容不进上下文，压缩无意义
2. 推理内容需要完整保留供用户研究
3. 压缩摘要会丢失推理细节

**唯一例外**: 未来可考虑对 `summary_text` 做 LLM 摘要（仅用于前端预览），但 `thinking_text` 原文永远不变。

---

## 五、分阶段实施计划

### Phase 1: 数据模型层（基础）

| 步骤 | 文件 | 变更 |
|------|------|------|
| P1.1 | `lcm/types.rs` | `StoredMessage` 新增 `reasoning_id: Option<String>` |
| P1.2 | `lcm/types.rs` | 新增 `ReasoningChain` 结构体 |
| P1.3 | `lcm/store.rs` | 新增 `reasoning_chains` 表 DDL |
| P1.4 | `lcm/store.rs` | 新增 `insert_reasoning_chain` / `get_reasoning_chain` / `get_reasoning_chains_for_conversation` 方法 |
| P1.5 | `lcm/store.rs` | Schema migration（版本递增） |
| P1.6 | `conversation_store/types.rs` | `AssistantLine` 新增 `thinking` 和 `thinking_token_count` 字段 |
| P1.7 | `provider_api/types.rs` | `ResponseStreamResult` 新增 `reasoning_text` 和 `reasoning_tokens` 字段 |

### Phase 2: 协议层（捕获推理）

| 步骤 | 文件 | 变更 |
|------|------|------|
| P2.1 | `provider_api/protocol/chat.rs` | 累积 `reasoning_content` 到 buffer，`finish_reason` 时写入 `output_items` |
| P2.2 | `provider_api/protocol/responses.rs` | 新增 `response.reasoning.*` 事件处理 |
| P2.3 | `provider_api/core.rs` | `compose_tool_continuation_input` 中的推理处理保持（已有代码，但需验证） |

### Phase 3: 运行时层（持久化推理）

| 步骤 | 文件 | 变更 |
|------|------|------|
| P3.1 | `runtime/engine.rs` | 从 `output_items` 提取 `type=="reasoning"` 项，创建 `ReasoningChain` 并存储 |
| P3.2 | `runtime/engine.rs` | `StoredMessage` 关联 `reasoning_id` |
| P3.3 | `runtime/engine/turn.rs` | `TurnAccumulator::record_hop` 保留推理项 |
| P3.4 | `runtime/engine/output.rs` | 可选：新增 `extract_reasoning_from_items` 辅助函数 |

### Phase 4: LCM 上下文层（隔离推理）

| 步骤 | 文件 | 变更 |
|------|------|------|
| P4.1 | `lcm/mod.rs` | `stored_messages_to_conversation_lines` 从 `reasoning_chains` 表加载 thinking |
| P4.2 | `lcm/engine.rs` | `context_to_provider_items` 对历史消息排除 reasoning（仅本轮可包含） |
| P4.3 | `conversation_store/context/builders.rs` | `build_assistant_input_item` 支持可选的 reasoning 包含开关 |
| P4.4 | `lcm/compaction.rs` | 确认压缩管道不触及 reasoning_chains 表 |

### Phase 5: 前端层（展示与恢复）

| 步骤 | 文件 | 变更 |
|------|------|------|
| P5.1 | `features/conversations/sessionState.ts` | `applyLoadedConversationDetail` 保留 `thinking` |
| P5.2 | `features/conversations/sessionState.ts` | `applyCompletedRequest` 保留 `thinking` |
| P5.3 | `components/ChatArea.tsx` | 历史消息的 thinking 默认折叠，新增"查看思维链"按钮 |
| P5.4 | `hooks/useConversationStreaming.ts` | 验证流式路径与持久化路径的一致性 |

### Phase 6: 测试与验证

| 步骤 | 内容 |
|------|------|
| P6.1 | 单元测试：`ReasoningChain` 的 CRUD 操作 |
| P6.2 | 集成测试：Chat Completions 协议推理捕获 → LCM 存储 → 前端展示 |
| P6.3 | 集成测试：JSONL 导出/导入包含 reasoning 字段 |
| P6.4 | 回归测试：无推理内容的旧对话向后兼容 |
| P6.5 | 端到端测试：DeepSeek R1 完整对话 → 历史推理可查看 → 上下文不含历史推理 |

---

## 六、风险评估与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| 推理内容极大（>10K tokens） | DB 存储压力 | `thinking_text` 使用 TEXT 类型（SQLite 无上限）；JSONL 每行可能很长但可接受 |
| 旧数据迁移 | 启动时 schema 变更 | Migration 逻辑仅新增表，不修改旧数据 |
| 前后端接口不一致 | 前端崩溃 | 使用 `Option` + `skip_serializing_if` 确保向后兼容 |
| 上下文重建遗漏推理 | 模型缺少本轮推理上下文 | 运行时显式传递当前轮推理；添加集成测试覆盖 |
| LCM 压缩覆盖推理 | 推理内容被意外压缩 | `reasoning_chains` 表独立于 `messages` 表，不在压缩 DAG 范围内 |

---

## 七、附录

### A. 相关文件索引

| 文件 | 相关行 | 内容 |
|------|--------|------|
| `src-tauri/src/provider_api/types.rs` | 32-112 | `ProviderStreamEvent` 枚举定义 |
| `src-tauri/src/provider_api/protocol/chat.rs` | 288-367 | Chat Completions 流式解析 |
| `src-tauri/src/provider_api/protocol/responses.rs` | 167-262 | Responses API 流式解析 |
| `src-tauri/src/provider_api/core.rs` | 172-188 | 工具继续输入组合 |
| `src-tauri/src/runtime/engine.rs` | 310-383 | LCM 持久化 |
| `src-tauri/src/runtime/engine/output.rs` | 42-74 | 从 output_items 提取助手消息 |
| `src-tauri/src/runtime/engine/turn.rs` | 24-54 | TurnAccumulator |
| `src-tauri/src/runtime/stream_collection.rs` | 152-203 | 流事件路由 |
| `src-tauri/src/lcm/types.rs` | 86-125 | `StoredMessage` 结构体 |
| `src-tauri/src/lcm/store.rs` | 1077-1185 | SQLite Schema DDL |
| `src-tauri/src/lcm/mod.rs` | 166-293 | LCM → ConversationLine 转换 |
| `src-tauri/src/lcm/engine.rs` | 906-1024 | 上下文 → Provider Items |
| `src-tauri/src/lcm/compaction.rs` | 全文 | 三级压缩逻辑 |
| `src-tauri/src/conversation_store/types.rs` | 220-254 | `AssistantLine` 结构体 |
| `src-tauri/src/conversation_store/context/builders.rs` | 44-61 | 上下文重建 |
| `src-tauri/src/conversation_store/file_io.rs` | 113-161 | JSONL 序列化 |
| `src-tauri/src/commands/chat/chat_events.rs` | 76-86 | 事件映射（后端→前端） |
| `src/features/conversations/types.ts` | 33-45 | 前端 `AssistantLine` 接口 |
| `src/features/conversations/sessionState.ts` | 310-341 | 前端思考增量应用 |
| `src/hooks/useConversationStreaming.ts` | 227-247 | 前端流式事件处理 |
| `src/components/ChatArea.tsx` | 245-260 | 思考内容展示 |

### B. 术语对照

| 中文 | 英文 | 说明 |
|------|------|------|
| 思维链 / 推理内容 | Reasoning Content / Thinking / Chain of Thought | 模型在最终输出前的内部推理过程 |
| 上下文窗口 | Context Window | 发送给模型的完整对话历史 |
| 压缩 | Compaction | LCM 的三级压缩机制（Normal/Aggressive/Truncation） |
| 持久化 | Persistence | 将消息存储到 SQLite / JSONL |
| 重建 | Reconstruction | 从存储中恢复对话上下文 |
