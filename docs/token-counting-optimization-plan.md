# Token 计数优化方案

> **状态**: 分析完成，待实施
> **日期**: 2026-06-04
> **依赖**: [reasoning-content-optimization-plan.md](reasoning-content-optimization-plan.md)（Phase 1-4 完成后）
> **关联变更**: 当前未提交的 working tree 变更（reasoning 持久化初步实现）

---

## 一、背景与核心矛盾

### 1.1 问题陈述

框架中存在 **两套 Token 计数体系**，服务于不同目的，但对推理 Token 的处理存在冲突：

| 计数体系 | 数据来源 | 用途 | 推理 Token 应该？ |
|----------|---------|------|-------------------|
| **API 计费 Token** | 上游 API 返回的 `usage` 对象 | 用户真实消耗展示、成本核算 | ✅ 应当计入（真实的用户费用） |
| **LCM 上下文 Token** | 本地 tokenizer / 字符估算 | 上下文窗口预算管理、压缩阈值决策 | ❌ 不应计入（推理不进历史上下文） |

**核心矛盾**：上游 API 返回的 `completion_tokens` 通常包含推理 Token（DeepSeek R1 等模型的 reasoning_content 是 completion 的一部分），但我们希望 LCM 的上下文管理将历史推理 Token 排除在外。

### 1.2 当前未提交变更带来的新问题

当前 working tree 中的变更已将推理内容写入 `output_items`（`{"type": "reasoning", "text": "..."}`），这会导致：

1. **`build_chat_completion_messages`**（[token_usage/messages.rs:71-80](src-tauri/src/conversation_store/context/token_usage/messages.rs#L71-L80)）会将 `type=="reasoning"` 的项计入上下文 Token —— **错误**，历史推理不应占用上下文预算
2. **`estimate_input_items_tokens`**（[budget.rs:87-96](src-tauri/src/conversation_store/context/budget.rs#L87-L96)）序列化整个 `output_items` JSON 来估算 Token，推理项会被计入 —— **错误**
3. **LCM `estimate_context_tokens`** 对 `RawMessage.content` 计数，不涉及推理 —— **目前正确**，但需确认后续不变

---

## 二、当前 Token 计数体系全面审计

### 2.1 体系架构

```
                    ┌──────────────────────────────┐
                    │      Provider API Response     │
                    │  usage.prompt_tokens           │
                    │  usage.completion_tokens       │  ← 通常包含推理 Token
                    │  usage.total_tokens            │
                    │  (reasoning_tokens 字段缺失)    │  ← ⚠️ 未解析
                    └──────────────┬─────────────────┘
                                   │
            ┌──────────────────────┼──────────────────────┐
            ▼                      ▼                      ▼
   ┌─────────────────┐   ┌──────────────────┐   ┌──────────────────┐
   │ ProviderUsage   │   │ 本地 Tokenizer    │   │ LCM 估算         │
   │ (计费用途)      │   │ (上下文预算用途)  │   │ (压缩阈值用途)   │
   │                 │   │                  │   │                  │
   │ prompt_tokens   │   │ count_model_     │   │ estimate_tokens  │
   │ completion_     │   │ tokens()         │   │ (4:1 char ratio) │
   │   tokens        │   │                  │   │                  │
   │ total_tokens    │   │ tokenizers crate │   │ StoredMessage    │
   │                 │   │ 退化: 4:1 ratio  │   │   .token_count   │
   └────────┬────────┘   └────────┬─────────┘   └────────┬─────────┘
            │                     │                       │
            ▼                     ▼                       ▼
    ┌────────────────┐  ┌─────────────────┐  ┌─────────────────────┐
    │ 前端展示       │  │ TokenBudget     │  │ LCM 压缩触发判断   │
    │ 用户实际消耗   │  │ 上下文窗口裁剪  │  │ τ_soft / τ_hard    │
    │                │  │ truncate_items  │  │                     │
    │ UsageUpdated   │  │ _to_budget()    │  │ process_messages    │
    │ 事件           │  │                 │  │ _batch()            │
    └────────────────┘  └─────────────────┘  └─────────────────────┘
```

### 2.2 API 计费 Token 路径

**`ProviderUsage` 结构体** — [`provider_api/types.rs:155-184`](src-tauri/src/provider_api/types.rs#L155-L184):

```rust
pub struct ProviderUsage {
    pub prompt_tokens: usize,       // 输入 token
    pub completion_tokens: usize,   // 输出 token (包含 reasoning!)
    pub total_tokens: usize,        // 总计
    // ⚠️ 缺少 reasoning_tokens 字段
}
```

**数据来源**:
- Chat Completions: `parse_chat_usage()` 解析 `value["usage"]`，映射 `prompt_tokens`/`completion_tokens`/`total_tokens`
- Responses API: `parse_responses_usage()` 同样

**关键问题**: 
- 上游 API (DeepSeek, OpenAI o-series) 返回的 `completion_tokens` 包含推理 Token
- 但 `usage` 对象中有时也包含 `completion_tokens_details.reasoning_tokens`（如 DeepSeek）或 `reasoning_tokens` 独立字段
- 当前代码 **未解析** 这些字段，无法区分推理 Token 和普通输出 Token

**运行时使用** — [`runtime/engine.rs:263-271`](src-tauri/src/runtime/engine.rs#L263-L271):

```rust
// UsageUpdated 事件 — 聚合所有 hop 的 ProviderUsage
accumulator.usage.saturating_add(hop_usage);
// → 发送到前端，用户看到的是包含推理的总 token 消耗
```

### 2.3 LCM 上下文 Token 路径

#### 2.3.1 本地 Tokenizer 计数

**入口函数**: `count_request_prompt_tokens` / `count_conversation_prompt_tokens`
→ `build_chat_completion_messages` → `count_messages_tokens` → `count_model_tokens`

**`build_chat_completion_messages`** — [`token_usage/messages.rs:12-91`](src-tauri/src/conversation_store/context/token_usage/messages.rs#L12-L91):

```rust
match item_type {
    "message" => { /* 转换 message → TokenCountMessage */ }
    "function_call" | "custom_tool_call" => { /* 工具调用 Token */ }
    "function_call_output" => { /* 工具输出 Token */ }
    "reasoning" => {                          // ⚠️ 当前代码
        if let Some(summary) = extract_reasoning_summary(item) {
            messages.push(TokenCountMessage {
                role: "assistant",
                content: Some(summary),       // ← 推理内容计入上下文!
            });
        }
    }
}
```

**问题**: 此代码原本设计目的是未来保证（从注释可知），但在未提交变更将推理项写入 `output_items` 后，这段代码将被激活并**错误地将历史推理计入上下文 Token**。

#### 2.3.2 上下文预算管理

**`TokenBudget::for_model`** — [`budget.rs:30-61`](src-tauri/src/conversation_store/context/budget.rs#L30-L61):

```rust
pub fn for_model(provider_kind: &str, model_id: &str) -> Self {
    let window_tokens = get_model_metadata(provider_kind, model_id)
        .context_window.unwrap_or(128_000);
    // 预留空间给 instructions + tool schemas + current turn
    let reserved = if window_tokens >= 1_000_000 { 20_000 }
                   else if window_tokens >= 128_000 { 16_000 }
                   ...
    Self {
        context_window: window_tokens,
        context_budget: window_tokens.saturating_sub(reserved),
    }
}
```

**`estimate_input_items_tokens`** — [`budget.rs:87-96`](src-tauri/src/conversation_store/context/budget.rs#L87-L96):

```rust
pub fn estimate_input_items_tokens(items: &[Value]) -> usize {
    items.iter()
        .map(|item| {
            let serialized = serde_json::to_string(item).unwrap_or_default();
            serialized.len().saturating_add(3) / 4  // 4:1 char ratio
        })
        .sum()
}
```

**问题**: 如果 `output_items` 包含 `{"type":"reasoning","text":"非常长的推理内容..."}` 项，`estimate_input_items_tokens` 会将其计入上下文预算。

**但是**：`load_context_for_request` 通过 LCM→`stored_messages_to_conversation_lines`→`build_context_items` 路径生成 input items。当前 `build_assistant_input_item` 只使用 `line.text`，不包含 `line.thinking`，所以推理不会进入这个路径。这个路径是安全的。

**真正需要关注的路径**: 
- `count_conversation_prompt_tokens` 中调用 `build_chat_completion_messages`，它直接处理 items
- 如果未来 items 中包含 reasoning 项，就会错误计数

#### 2.3.3 LCM 压缩 Token 计数

**`StoredMessage.token_count`**: 使用 `estimate_tokens(content)` 设置（[lcm/types.rs:624-627](src-tauri/src/lcm/types.rs#L624-L627)），仅计消息正文，不涉及推理。

**`ReasoningChain.token_count`**: 已独立存储（[lcm/store.rs 变更](src-tauri/src/lcm/store.rs)），不影响 LCM 压缩判断。

**`CompactionEngine`** — [`compaction.rs:73-79`](src-tauri/src/lcm/compaction.rs#L73-L79):

```rust
pub struct CompactionEngine {
    count_tokens: TokenCounter,  // 当前指向 estimate_tokens (4:1 ratio)
}
```

压缩器连接消息正文来，计数基于正文——推理不在这个通路中。**目前正确**。

**`estimate_context_tokens`** — [`lcm/types.rs:632-643`](src-tauri/src/lcm/types.rs#L632-L643):

```rust
pub fn estimate_context_tokens(entries: &[ContextEntry]) -> u32 {
    entries.iter()
        .map(|entry| match entry {
            ContextEntry::RawMessage { content, .. } => estimate_tokens(content),
            // ...
        })
        .sum()
}
```

仅对 `RawMessage.content` 计数（消息正文），不包含推理。**正确**。

### 2.4 前端 Token 展示

**`context_token_count`** 通过两个渠道发送：

1. **流式事件**: `ChatStreamObserver` 在每次事件循环中更新 `context_token_count`
2. **最终事件**: 在对话完成时通过 `ChatStreamEvent` 发送

前端在 [`ChatArea.tsx`](src/components/ChatArea.tsx) 中展示 `contextTokenCount`，这个值来自本地 tokenizer 计数。

---

## 三、问题差距汇总

| # | 差距 | 位置 | 严重程度 | 影响 |
|---|------|------|---------|------|
| T1 | `ProviderUsage` 无 `reasoning_tokens` 字段，无法区分推理和普通输出 Token | [`provider_api/types.rs:155-184`](src-tauri/src/provider_api/types.rs#L155-L184) | 🔴 高 | 用户无法知道多少 Token 花在推理上 |
| T2 | `build_chat_completion_messages` 对 `type=="reasoning"` 项计入上下文计数 | [`token_usage/messages.rs:71-80`](src-tauri/src/conversation_store/context/token_usage/messages.rs#L71-L80) | 🔴 高 | 未提交变更激活后，历史推理会虚增上下文 Token |
| T3 | `ProviderUsage` 的 `completion_tokens` 未拆分出推理部分 | [`provider_api/types.rs:175`](src-tauri/src/provider_api/types.rs#L175) | 🟡 中 | 计费展示精度不足 |
| T4 | `ReasoningCompleted` 事件中的 `total_tokens` 未从 API 解析 | [`protocol/chat.rs:307`](src-tauri/src/provider_api/protocol/chat.rs#L307) | 🟡 中 | 流式推理 Token 计数丢失 |
| T5 | LCM `StoredMessage.token_count` 不包含推理，但缺少显式的分离保证 | [`lcm/types.rs:107`](src-tauri/src/lcm/types.rs#L107) | 🟢 低 | 目前正确，但缺少测试防护 |
| T6 | 前端只展示 `contextTokenCount`，不展示总消耗（含推理） | [`ChatArea.tsx`](src/components/ChatArea.tsx) | 🟢 低 | 用户看不到真实计费 Token |

---

## 四、优化方案设计

### 4.1 核心设计原则

```
┌─────────────────────────────────────────────────────────────────┐
│                      Token 计数分离原则                          │
│                                                                 │
│  1. 计费 Token (Billing)   = prompt + completion (含 reasoning) │
│     → 来源: 上游 API usage 对象                                  │
│     → 用途: 用户展示真实消耗                                      │
│                                                                 │
│  2. 上下文 Token (Context)  = prompt + completion (不含 reasoning)│
│     → 来源: 本地 tokenizer / 字符估算                             │
│     → 用途: LCM 上下文窗口预算、压缩阈值                           │
│                                                                 │
│  3. 推理 Token (Reasoning)  = reasoning_content 的 token 数      │
│     → 来源: API usage.reasoning_tokens / 本地估算                 │
│     → 用途: 前端展示推理占比、成本分析                             │
│                                                                 │
│  4. 上下文计数显式过滤 reasoning 项                               │
│     → build_chat_completion_messages 跳过 type=="reasoning"      │
│     → estimate_input_items_tokens 跳过 reasoning 项              │
└─────────────────────────────────────────────────────────────────┘
```

### 4.2 数据结构变更

#### 4.2.1 `ProviderUsage` 新增 `reasoning_tokens`

```rust
// provider_api/types.rs
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,   // 可能包含 reasoning（取决于 API）
    pub total_tokens: usize,
    
    // 新增：
    /// Reasoning/thinking tokens reported by the provider.
    /// When available, completion_tokens includes reasoning_tokens.
    /// The non-reasoning output is: completion_tokens - reasoning_tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<usize>,
}
```

#### 4.2.2 `ConversationTokenUsage` 新增 `reasoning_tokens`

```rust
// context/token_usage/types.rs
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationTokenUsage {
    pub context_tokens: usize,      // 仅上下文（不含推理）
    pub prompt_tokens: usize,       // context_tokens + tool schemas
    pub reasoning_tokens: usize,    // 新增：推理 token 数
}
```

### 4.3 协议层变更

#### 4.3.1 Chat Completions — 解析 `reasoning_tokens`

上游 API（DeepSeek, OpenAI o-series）在 usage 中可能返回：

```json
// DeepSeek 格式
{ "completion_tokens_details": { "reasoning_tokens": 512 } }

// OpenAI o-series 格式  
{ "completion_tokens_details": { "reasoning_tokens": 512 } }
```

**变更** — [`protocol/chat.rs:369-405`](src-tauri/src/provider_api/protocol/chat.rs#L369-L405):

```rust
fn parse_chat_usage(value: &Value) -> Option<ProviderUsage> {
    let mut usage: ProviderUsage = serde_json::from_value(value.get("usage")?.clone()).ok()?;
    
    // 提取 reasoning_tokens
    if let Some(details) = value.get("usage")
        .and_then(|u| u.get("completion_tokens_details"))
    {
        usage.reasoning_tokens = details.get("reasoning_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
    }
    
    // ... 现有逻辑
}
```

#### 4.3.2 Responses API — 同样提取 reasoning_tokens

同上模式。

#### 4.3.3 `ReasoningCompleted` 事件的 `total_tokens` 填充

```rust
// protocol/chat.rs — 在 reasoning 完成时
state.reasoning_buffer.push_str(content);
on_delta(ProviderStreamEvent::ReasoningCompleted { 
    total_tokens: Some(estimate_tokens(&state.reasoning_buffer) as usize)
});
```

### 4.4 上下文 Token 计数变更

#### 4.4.1 `build_chat_completion_messages` — **跳过 reasoning 项**

```rust
// token_usage/messages.rs:71-80
// 变更前：
"reasoning" => {
    if let Some(summary) = extract_reasoning_summary(item) {
        messages.push(TokenCountMessage {
            role: "assistant",
            content: Some(summary),
            ...
        });
    }
}

// 变更后：
"reasoning" => {
    // 推理内容不进入上下文，跳过
    // 仅记录 token 数量用于独立统计
    continue;
}
```

**理由**: `build_chat_completion_messages` 的输出用于 `count_messages_tokens`，此函数计算的是上下文的 Token。推理不应计入上下文预算。

#### 4.4.2 新增独立的 `count_reasoning_tokens` 函数

```rust
// token_usage.rs 新增
pub fn count_reasoning_tokens_from_items(items: &[Value]) -> usize {
    items.iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("reasoning"))
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .map(|text| count_model_tokens("default", text)) // 使用默认 tokenizer
        .sum()
}
```

#### 4.4.3 `estimate_input_items_tokens` — 过滤 reasoning 项

```rust
// budget.rs
pub fn estimate_input_items_tokens(items: &[Value]) -> usize {
    items.iter()
        .filter(|item| {
            // 跳过 reasoning 项 — 不进上下文
            item.get("type").and_then(Value::as_str) != Some("reasoning")
        })
        .map(|item| {
            let serialized = serde_json::to_string(item).unwrap_or_default();
            serialized.len().saturating_add(3) / 4
        })
        .sum()
}
```

#### 4.4.4 LCM `estimate_context_tokens` — 无需变更

当前实现仅对 `RawMessage.content` 计数，推理存储在独立的 `ReasoningChain` 表中（通过 `reasoning_id` 关联），不在 `content` 字段中。**保持现状**。

但需加强测试防护，确保未来变更不会将推理内容混入 `content`。

### 4.5 运行时层的聚合逻辑

#### 4.5.1 `TurnAccumulator` — 分别累加推理 Token

```rust
// runtime/engine/turn.rs
impl TurnAccumulator {
    pub fn record_hop(&mut self, result: &ResponseStreamResult) {
        // ... 现有逻辑 ...
        
        // 新增：记录推理 token
        if let Some(reasoning) = &result.reasoning_tokens {
            self.total_reasoning_tokens += reasoning;
        }
    }
}
```

#### 4.5.2 `ProviderUsage` 累加时保留 reasoning_tokens

```rust
impl ProviderUsage {
    pub fn saturating_add(&mut self, other: &ProviderUsage) {
        self.prompt_tokens = self.prompt_tokens.saturating_add(other.prompt_tokens);
        self.completion_tokens = self.completion_tokens.saturating_add(other.completion_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
        // 新增：
        if let Some(rt) = other.reasoning_tokens {
            *self.reasoning_tokens.get_or_insert(0) += rt;
        }
    }
}
```

### 4.6 前端 Token 展示增强

#### 4.6.1 `ChatStreamEvent` 新增 `reasoningTokenCount`

```rust
// commands/chat/chat_events.rs
pub struct ChatStreamEvent {
    // ... 现有字段 ...
    pub reasoning_token_count: Option<usize>,
}
```

#### 4.6.2 前端展示

在 [`ChatArea.tsx`](src/components/ChatArea.tsx) 中增强 Token 显示：

```
当前展示：
  📊 上下文: 12,345 tokens

建议展示：
  📊 上下文: 12,345 tokens | 🧠 推理: 512 tokens | 💰 总计消耗: 15,200 tokens
```

---

## 五、上下文预算的推理感知

### 5.1 预算预留策略

当前 `TokenBudget::for_model` 预留空间给 instructions + tool schemas + **当前轮输出**。在推理模型场景下，当前轮的推理内容也会进入上下文，因此需要：

```rust
// 在发送请求前，额外估算本轮可能的推理 token 消耗
// 从 ProviderTurnRequest.reasoning_budget_tokens / reasoning_effort 获取
```

但 LCM 的 `context_budget` 仍然应**不包括历史推理**，因为在上下文重建时历史推理已被剥离。

### 5.2 Token Budget 计算流程

```
TokenBudget::for_model(model)
    │
    ├── context_window: 128,000 (来自 model metadata)
    ├── reserved: 16,000 (instructions + tool schemas + current turn)
    └── context_budget: 112,000 (可用于历史上下文)
    
load_context_for_request(conversation_id, budget)
    │
    ├── 加载 context items (不含历史 reasoning)
    ├── 检查 estimate_input_items_tokens(items) <= budget.context_budget
    └── 如需截断: truncate_items_to_budget()
```

---

## 六、分阶段实施计划

### Phase 1: 数据模型增强

| 步骤 | 文件 | 变更 |
|------|------|------|
| P1.1 | `provider_api/types.rs` | `ProviderUsage` 新增 `reasoning_tokens: Option<usize>` |
| P1.2 | `context/token_usage/types.rs` | `ConversationTokenUsage` 新增 `reasoning_tokens: usize` |

### Phase 2: 协议层 Token 解析

| 步骤 | 文件 | 变更 |
|------|------|------|
| P2.1 | `provider_api/protocol/chat.rs` | `parse_chat_usage()` 提取 `completion_tokens_details.reasoning_tokens` |
| P2.2 | `provider_api/protocol/responses.rs` | `parse_responses_usage()` 同样提取 |
| P2.3 | `provider_api/protocol/chat.rs` | `ReasoningCompleted.total_tokens` 用本地估算填充 |
| P2.4 | `provider_api/protocol/responses.rs` | 同上 |

### Phase 3: 上下文计数的推理排除

| 步骤 | 文件 | 变更 |
|------|------|------|
| P3.1 | `context/token_usage/messages.rs` | `"reasoning"` case: 改为 `continue`（跳过不计数） |
| P3.2 | `context/token_usage.rs` | 新增 `count_reasoning_tokens_from_items()` 函数 |
| P3.3 | `context/budget.rs` | `estimate_input_items_tokens` 过滤 reasoning 项 |
| P3.4 | `context/budget.rs` | `truncate_items_to_budget` 过滤时保留 reasoning 关联性 |

### Phase 4: 运行时聚合

| 步骤 | 文件 | 变更 |
|------|------|------|
| P4.1 | `runtime/engine/turn.rs` | `TurnAccumulator` 新增 `total_reasoning_tokens` 并聚合 |
| P4.2 | `provider_api/types.rs` | `ProviderUsage::saturating_add` 处理 `reasoning_tokens` |
| P4.3 | `runtime/engine.rs` | 确认 `ReasoningChain.token_count` 使用本地估算值 |

### Phase 5: 前端展示

| 步骤 | 文件 | 变更 |
|------|------|------|
| P5.1 | `commands/chat/chat_events.rs` | `ChatStreamEvent` 新增 `reasoning_token_count` |
| P5.2 | `commands/chat/chat_stream_observer.rs` | 在事件中传递推理 Token 计数 |
| P5.3 | `hooks/useConversationStreaming.ts` | 接收并展示推理 Token |
| P5.4 | `components/ChatArea.tsx` | 增强 Token 显示组件 |

### Phase 6: 测试与回归

| 步骤 | 内容 |
|------|------|
| P6.1 | 单元测试：`ProviderUsage` 解析 `reasoning_tokens` |
| P6.2 | 单元测试：`build_chat_completion_messages` 跳过 reasoning 项 |
| P6.3 | 单元测试：`estimate_input_items_tokens` 不包含 reasoning |
| P6.4 | 集成测试：完整流程 — API 返回 reasoning_tokens → 存储 → 上下文计数排除 |
| P6.5 | 回归测试：无推理模型（GPT-4o 等）的计费不受影响 |
| P6.6 | 回归测试：`ProviderUsage` 向后兼容（旧 API 不返回 reasoning_tokens 的 Provider） |

---

## 七、与 reasoning-content-optimization-plan.md 的关系

两个方案相互配合，在实施时需注意顺序：

| reasoning-content plan Phase | token-counting plan Phase | 关系 |
|------------------------------|---------------------------|------|
| Phase 1 (数据模型) | Phase 1 (ProviderUsage 字段) | 可并行 |
| Phase 2 (协议层) | Phase 2 (协议层 Token 解析) | **合并实施**：同一个 `process_chat_event` 函数既写入 reasoning 到 output_items，又解析 reasoning_tokens |
| Phase 3 (运行时持久化) | Phase 3 (上下文计数的排除) | **顺序依赖**：先有 reasoning 写入 output_items，再需要排除 |
| Phase 4 (LCM 上下文) | Phase 4 (运行时聚合) | 可并行 |
| Phase 5 (前端) | Phase 5 (前端展示) | **合并实施**：前端同时适配 thinking 展示和 Token 展示 |

---

## 八、风险评估

| 风险 | 影响 | 缓解 |
|------|------|------|
| 旧 Provider 不返回 `reasoning_tokens` | `Option::None` → 前端显示 0 | `Option<usize>` 类型 + `skip_serializing_if` 确保兼容 |
| `build_chat_completion_messages` 跳过 reasoning 后计数偏低 | 上下文预算宽松 → 可能超过窗口 | 预留足够的 buffer（当前 16K for 128K window 已足够） |
| `estimate_input_items_tokens` 过滤 reasoning 后的截断逻辑 | 截断点变化影响对话连续性 | 仅过滤 reasoning 项是语义正确的；截断基于正文 token |
| 上游 API usage.reasoning_tokens 不可用 | 退化使用本地估算 | `ReasoningChain.token_count` 已用 4:1 估算 |

---

## 九、附录：关键文件对照

| 文件 | 核心功能 | 本方案相关行 |
|------|---------|-------------|
| `provider_api/types.rs` | `ProviderUsage`, `ResponseStreamResult` | 155-218, 237-244 |
| `provider_api/protocol/chat.rs` | Chat Completions 流解析 + usage | 288-405 |
| `provider_api/protocol/responses.rs` | Responses API 流解析 + usage | 167-340 |
| `context/token_usage.rs` | Token 计数入口函数 | 全文 |
| `context/token_usage/messages.rs` | input_items → TokenCountMessage 转换 | 12-91 |
| `context/token_usage/tokenizer.rs` | 本地 tokenizer 管理 | 全文 |
| `context/token_usage/types.rs` | ConversationTokenUsage 等类型 | 全文 |
| `context/budget.rs` | TokenBudget, 上下文窗口预算 | 30-96, 106-126 |
| `context/mod.rs` | 上下文加载 + 组装管道 | 41-141 |
| `lcm/types.rs` | `estimate_tokens`, `StoredMessage.token_count` | 624-643 |
| `lcm/compaction.rs` | LCM 三级压缩（使用 TokenCounter） | 22, 73-79 |
| `runtime/engine.rs` | 运行时引擎，LCM 持久化 + usage 聚合 | 263-271, 320-383, 550-578 |
| `runtime/engine/turn.rs` | TurnAccumulator | 24-54 |
| `commands/chat/chat_prompt_tokens.rs` | UI 上下文 Token 计数 | 47-124 |
| `commands/chat/chat_events.rs` | 前端事件映射 | 76-86 |
