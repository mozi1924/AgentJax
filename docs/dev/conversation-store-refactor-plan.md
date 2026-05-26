# 会话存储重构方案

## 当前系统问题

### 1. 碎片化存储
一个用户问"帮我搜一下 Rust agent 框架"会产生 **8+ 行** JSONL：

```
Line 1: { recordType:"message", role:"user", text:"帮我搜...", requestId:"req-1" }
Line 2: { recordType:"context_item", context_items:[{type:"function_call",call_id:"call_1",...}] }
Line 3: { recordType:"context_item", context_items:[{type:"function_call_output",call_id:"call_1",...}] }
Line 4: { recordType:"context_item", context_items:[{type:"function_call",call_id:"call_2",...}] }
Line 5: { recordType:"context_item", context_items:[{type:"function_call_output",call_id:"call_2",...}] }
Line 6: { recordType:"context_item", context_items:[{type:"reasoning",...}] } ← persist_completed_exchange 重新保存
Line 7: { recordType:"context_item", context_items:[{type:"function_call",call_id:"call_1",...}] } ← 重复！
Line 8: { recordType:"message", role:"assistant", text:"..." }
```

`persist_tool_progress_event` 和 `persist_completed_exchange` 各保存一遍，造成大量重复。

### 2. Fat Struct 反模式
`ConversationEntryLine` 有 18 个字段，大部分互斥：

```rust
pub struct ConversationEntryLine {
    record_type: String,     // "message" | "context_item"
    role: Option<String>,    // 只有 message 用
    text: Option<String>,    // 只有 message 用
    context_items: Vec<Value>, // 只有 context_item 用, 但又是 Vec...
    tool_name: Option<String>,  // 几乎不用（信息在 context_items 里）
    tool_call_id: Option<String>,
    tool_arguments: Option<Value>,
    tool_output: Option<Value>,
    timeline_events: Option<Vec<Value>>, // 冗余
    // ... + 9 more fields
}
```

### 3. 无完成状态标记
没有 `status` 字段。恢复机制（`build_recovery_developer_note`）必须扫描所有 entry 推断是否"未完成"，逻辑脆弱。

### 4. `context_items` 语义混乱
- `append_message` 把 user/assistant 文本包装成 `context_items`
- `append_context_item` 存的也是 `context_items`（但只有一个元素）
- 加载时 `load_context_for_request` 把两者混到一起

---

## 新设计：Tagged Union JSONL

### 核心原则

> **一行 = 一件事。用 `kind` 区分类型，每种类型只带自己需要的字段。**

### 行类型 (Kind)

```typescript
// ── 用户消息 ────────────────────────
{ kind: "user", id, ts, requestId, text }

// ── 工作阶段标记 ────────────────────
{ kind: "working_start", id, ts, requestId }
{ kind: "working_done", id, ts, requestId }

// ── 工具调用（含结果） ───────────────
{ kind: "tool", id, ts, requestId, callId, name, args, output?, status: "pending"|"done"|"failed" }

// ── 助手消息 ────────────────────────
{ kind: "assistant", id, ts, requestId, responseId, text, status: "draft"|"done" }
```

### 完整示例：多工具工作流

```jsonl
{"kind":"user","id":"msg-u1","ts":1700000000000,"requestId":"req-1","text":"帮我完成一个复杂的数据分析任务"}
{"kind":"working_start","id":"ws-1","ts":1700000001000,"requestId":"req-1"}
{"kind":"assistant","id":"msg-a1","ts":1700000002000,"requestId":"req-1","responseId":"resp-hop1","text":"我先来分析数据结构...","status":"done"}
{"kind":"tool","id":"tool-1","ts":1700000003000,"requestId":"req-1","callId":"call_1","name":"file_read","args":{"path":"data.csv"},"output":{"ok":true,"content":"..."},"status":"done"}
{"kind":"assistant","id":"msg-a2","ts":1700000004000,"requestId":"req-1","responseId":"resp-hop2","text":"数据包含3列，接下来计算统计...","status":"done"}
{"kind":"tool","id":"tool-2","ts":1700000005000,"requestId":"req-1","callId":"call_2","name":"calculator","args":{"expr":"avg(1,2,3)"},"output":{"ok":true,"result":2},"status":"done"}
{"kind":"working_done","id":"wd-1","ts":1700000006000,"requestId":"req-1"}
{"kind":"assistant","id":"msg-a3","ts":1700000007000,"requestId":"req-1","responseId":"resp-hop3","text":"本次我完成了数据分析：数据有3列，平均值为2。","status":"done"}
```

### 简单回复（无工具调用）

```jsonl
{"kind":"user","id":"msg-u1","ts":1700000000000,"requestId":"req-1","text":"你好"}
{"kind":"assistant","id":"msg-a1","ts":1700000001000,"requestId":"req-1","responseId":"resp-1","text":"你好！有什么可以帮你的？","status":"done"}
```

无 `working_start/done` — 前端据此直接显示简洁气泡。

### 中断恢复示例

```jsonl
{"kind":"user","id":"msg-u1","ts":1700000000000,"requestId":"req-1","text":"搜索 Rust agent 框架"}
{"kind":"tool","id":"tool-1","ts":1700000001000,"requestId":"req-1","callId":"call_1","name":"web_search","args":{"q":"Rust agent"},"output":{"ok":true,"results":[...]},"status":"done"}
{"kind":"tool","id":"tool-2","ts":1700000002000,"requestId":"req-1","callId":"call_2","name":"web_search","args":{"q":"langchain-rs"},"status":"pending"}
```
↑ 最后一行是 `status:"pending"` 的 tool → 框架知道需要继续执行并续接

---

## 完成状态追踪

### 规则

| 最后一行 kind | 最后一行 status | 请求状态 |
|---|---|---|
| `assistant` | `done` | ✅ 已完成 |
| `working_done` | — | ⚠️ 工作完成但缺最终总结，续接 |
| `assistant` | `draft` | ⚠️ 文本中断，需续接生成 |
| `tool` | `pending` | ⚠️ 工具未执行，需执行后续接 |
| `tool` | `done` | ⚠️ 工具已执行但模型还未回应，续接 |
| `user` | — | ⚠️ 用户发了消息但还没开始处理 |

### 续接流程

```
1. 加载会话，找到最后一个 requestId
2. 检查该 requestId 的最后一行：
   a. tool + pending → 执行该 tool，写入 output，更新 status→done，继续请求 API
   b. tool + done / working_done → 说明工具都执行完了，构建 continuation 请求 API
   c. assistant + draft → 用已有的 tool 结果构建 continuation 请求 API
   d. assistant + done → 不存在！不需要续接
3. API 返回 → 继续写入新行
4. 直到 assistant + done → 标记请求完成
```

---

## 文件格式

保持当前目录结构，简化内容：

```
~/.agentjax/sessions/{conversation_id}/
  metadata.json   — 精简元数据
  messages.jsonl  — 一行一件事
  workspace/      — 文件工具工作区
```

### metadata.json（精简版）

```json
{
  "version": 5,
  "conversationId": "2026-05-27-xxxx",
  "createdAt": 1700000000000,
  "updatedAt": 1700000003000,
  "title": "Rust Agent 框架调研",
  "titleSource": "auto",
  "messageCount": 4,
  "lastMessagePreview": "本次我完成了数据分析...",
  "conversationType": "webchat"
}
```

保留字段：`lastMessagePreview`（列表渲染）、`conversationType`（未来 webchat/groupmessage）。
去掉冗余字段：`recordType`、`lastMessageAt`（用 `updatedAt` 替代）、`utilityModel`。

### messages.jsonl

每行是一个自包含的 JSON 对象，`kind` 字段区分类型。

---

## Rust 类型设计

```rust
// ── 行类型（tagged union） ──

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
enum ConversationLine {
    #[serde(rename = "user")]
    User(UserLine),
    #[serde(rename = "working_start")]
    WorkingStart(WorkingMarkerLine),
    #[serde(rename = "working_done")]
    WorkingDone(WorkingMarkerLine),
    #[serde(rename = "tool")]
    Tool(ToolLine),
    #[serde(rename = "assistant")]
    Assistant(AssistantLine),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserLine {
    id: String,
    #[serde(rename = "ts")]
    created_at_unix_ms: i64,
    #[serde(rename = "requestId")]
    request_id: String,
    text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkingMarkerLine {
    id: String,
    #[serde(rename = "ts")]
    created_at_unix_ms: i64,
    #[serde(rename = "requestId")]
    request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolLine {
    id: String,
    #[serde(rename = "ts")]
    created_at_unix_ms: i64,
    #[serde(rename = "requestId")]
    request_id: String,
    #[serde(rename = "callId")]
    call_id: String,
    name: String,
    args: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<Value>,
    #[serde(default = "default_tool_status")]
    status: ToolStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ToolStatus {
    Pending,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AssistantLine {
    id: String,
    #[serde(rename = "ts")]
    created_at_unix_ms: i64,
    #[serde(rename = "requestId")]
    request_id: String,
    #[serde(rename = "responseId")]
    response_id: String,
    text: String,
    #[serde(default = "default_assistant_status")]
    status: AssistantStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum AssistantStatus {
    Draft,
    Done,
}
```

---

## API 变更（前后端接口）

### 后端 → 前端

```typescript
// 会话列表（不变）
interface ConversationSummary {
  conversationId: string;
  title: string;
  titleSource: string;
  messageCount: number;
  updatedAt: number;
}

// 会话详情（简化）
interface ConversationDetail {
  conversationId: string;
  title: string;
  lines: ConversationLine[];  // ← 替代 messages: ConversationMessage[]
}

type ConversationLine = UserLine | ToolLine | AssistantLine;
```

### 前端适配

前端当前用 `ConversationMessage.context_items` 来渲染 tool widgets。改为直接消费 `ConversationLine[]`：

- `kind: "user"` → 渲染用户气泡
- `kind: "tool"` → 渲染工具调用卡片（含参数 + 输出）
- `kind: "assistant"` → 渲染助手气泡

---

## 持久化时机

| 事件 | 操作 |
|---|---|
| 用户发送消息 | 立即写入 `user` 行 |
| 模型开始工作阶段（首次 tool call） | 写入 `working_start` 行 |
| 模型发起工具调用（`ToolCallCompleted`） | 写入 `tool` 行，`status: "pending"` |
| 工具执行完成（`ToolCallExecuted`） | **更新** 该 `tool` 行，写入 `output`，`status→"done"` |
| 模型输出中间文本（hop 中途的 assistant） | 写入 `assistant` 行，`status: "done"`（这些是中间的短消息） |
| 工作阶段结束 | 写入 `working_done` 行 |
| 模型输出最终总结文本 | 写入 `assistant` 行，`status` 先 `"draft"` 后更新为 `"done"` |

### 简单回复（无工具调用）

| 事件 | 操作 |
|---|---|
| 用户发送消息 | 写入 `user` 行 |
| 模型直接输出文本 | 写入 `assistant` 行，`status` 先 `"draft"` 后更新为 `"done"` |

无 `working_start/done`。

关键变化：`tool` 和 `assistant` 行不再是 append-only，而是 **先占位再更新**（就地修改 JSONL 文件的最后一行或最后几行）。

---

## 实现策略

### 方案 A：原地更新（推荐）
- 工具调用时 append `tool` 行（pending）
- 工具执行完后，重写整个 messages.jsonl（文件通常 < 1MB）
- 优点：简单、原子性好（写临时文件 + rename）
- 缺点：大会话 O(n) 写入

### 方案 B：只追加 + 读时合并
- 工具调用 append `tool` 行（pending）
- 工具执行完后 append 另一行 `tool_output`
- 加载时合并相邻的 tool + tool_output 对
- 优点：纯追加，写入快
- 缺点：加载逻辑复杂，需要合并步骤

**推荐方案 A**，因为：
- 会话文件通常很小（< 1MB），全量写入 < 1ms
- 代码简单直观
- 支持原地更新 status 和 text

---

## 迁移策略

用户说"直接大改"，所以：
1. 重写 `conversation_store` 模块
2. 旧会话文件不兼容，清空或手动删除即可（内部开发阶段）
3. 一次性改完，不留兼容层

---

## 变更清单

| 文件 | 操作 | 说明 |
|------|------|------|
| `conversation_store/types.rs` | 🔄 重写 | 新的 tagged union 类型 |
| `conversation_store/file_io.rs` | 🔄 重写 | 简化的读写逻辑 |
| `conversation_store/mutations.rs` | 🔄 重写 | append + update 操作 |
| `conversation_store/queries.rs` | 🔄 重写 | list/load 查询 |
| `conversation_store/context.rs` | 🔄 重写 | 构建 API input items |
| `conversation_store/recovery.rs` | 🔄 简化 | 基于 status 的续接检测 |
| `conversation_store/paths.rs` | ✅ 保持 | 路径逻辑不变 |
| `conversation_store/items.rs` | 🗑️ 删除 | 不再需要 |
| `conversation_store.rs` | 🔄 更新 | 更新 re-export |
| `commands/chat/chat_persistence.rs` | 🔄 重写 | 新的持久化调用 |
| `commands/chat/chat.rs` | 🔄 适配 | 新的 API 调用 |
| 前端相关组件 | 🔄 适配 | `ConversationLine[]` 渲染 |

---

## 待审批

请确认以上方案，特别是：
1. Tagged union 行设计（`user` / `tool` / `assistant`）是否满足需求？
2. `tool` 行先写 `pending` 后更新为 `done` 的方式是否可以？
3. 方案 A（原地更新 messages.jsonl）是否可接受？
4. 是否需要保留 `metadata.json` 中的 `lastMessagePreview` 用于列表预览？
