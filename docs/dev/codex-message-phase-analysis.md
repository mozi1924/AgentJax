# Codex 式中间输出/最终回答分层分析

## 目标

当前 AgentJax 已经实现了两类用户可见行为：

- Agent 工作时会持续输出一些文本
- 最后会给出一个最终回答

但框架目前没有稳定地区分：

- 哪些文本是工作中的旁白 / commentary
- 哪些文本是最终回答 / final answer

这会导致 `working_start` / `working_done` 与 assistant 文本错位，进而让 UI 和上下文重建都出现混乱。

这份文档基于三部分内容总结：

- 复现场景：`/Users/jaxlocke/.agentjax/sessions/d709db5a-cf42-41e2-bf98-58bc6f1b392a/messages.jsonl`
- AgentJax 当前实现
- Codex 源码：`/Volumes/Data/codex/codex-rs`

## 这次错乱是怎么发生的

在样本会话里，相关顺序是：

1. assistant 先输出一句中间说明
2. `working_start` 才被写入
3. tool 行被写入
4. assistant 又写入一大段文本
5. `working_done` 最后写入

也就是会出现这种顺序：

```text
assistant(commentary)
working_start
tool
assistant(这里其实已经像 final 了，但仍被当普通 assistant)
working_done
```

这和你想要的心智模型不一致。你希望的是更接近 Codex：

```text
commentary / streaming updates
tool activity
final answer
```

而不是“普通 assistant 消息 + 外围 marker 再去猜语义”。

## AgentJax 当前实现的问题

### 1. assistant 文本没有 phase

当前 `AssistantLine` 只有：

- `text`
- `status`

没有：

- `phase: commentary | final_answer`

见：

- `/Volumes/Data/AgentJax/src-tauri/src/conversation_store/types.rs`
- `/Volumes/Data/AgentJax/src/features/conversations/types.ts`

这意味着一旦一段文本被落盘为 `assistant`，系统就失去了“它到底是工作中旁白还是最终回答”的信息。

### 2. Provider 流解析层完全没保留 phase

`ChatStreamEventPayload` 里虽然已经有注释说明：

- `phase?: string`

但实际 provider 解析层并没有把 OpenAI/Codex 兼容的 `phase` 取出来。

见：

- `/Volumes/Data/AgentJax/src/features/conversations/types.ts`
- `/Volumes/Data/AgentJax/src-tauri/src/providers/responses/stream/parser.rs`

`parser.rs` 只在收：

- `response.output_text.delta`
- `response.output_text.done`
- `response.output_item.done`

但没有读取 message item 上的 `phase`。

### 3. runtime 发事件的顺序本身就会制造错位

在 `run_turn()` 里：

- 先根据本 hop 的 `output_text` 发 `HopAssistantText`
- 如果这一跳后面还有工具，再发 `WorkingStarted`

见：

- `/Volumes/Data/AgentJax/src-tauri/src/runtime/engine.rs`

关键顺序是：

```rust
if !collected.response_result.output_text.is_empty() {
    on_event(ProviderStreamEvent::HopAssistantText { ... })?;
}

if is_final_hop {
    ...
} else if !working_started {
    on_event(ProviderStreamEvent::WorkingStarted)?;
}
```

这意味着第一段中间旁白会天然先于 `working_start` 落盘，所以单靠 marker 包裹 assistant 文本，本来就不稳。

### 4. 所有 hop assistant 文本都会被持久化成普通 assistant

当前：

- `HopAssistantText` 不区分 `is_final`
- `persist_hop_assistant_line()` 直接写 `ConversationLine::Assistant`

见：

- `/Volumes/Data/AgentJax/src-tauri/src/commands/chat.rs`
- `/Volumes/Data/AgentJax/src-tauri/src/commands/chat/chat_persistence.rs`

而且注释已经写明：

```rust
/// Called for every model-response hop — both commentary (working) and final.
```

也就是说，当前设计从一开始就是把 commentary 和 final 混存成同一种 assistant 行。

### 5. 前端流式展示也没有真正按 working/final 分流

`App.tsx` 会：

- 收到 `delta` 时直接往最后一个 draft assistant 追加文本
- 收到 `working_started` / `working_done` 时单独插 marker

见：

- `/Volumes/Data/AgentJax/src/App.tsx`

但 `delta` 分支没有按 `payload.phase` 做真正分流。结果就是：

- 流式文本本身是“相位盲”的
- marker 只是后置补丁

### 6. 更严重的问题：commentary 还会污染后续请求上下文

`load_context_for_request()` 会把所有 `assistant` 行都回放成：

```json
{
  "type": "message",
  "role": "assistant",
  "status": "completed",
  "content": [{ "type": "output_text", "text": a.text, "annotations": [] }]
}
```

见：

- `/Volumes/Data/AgentJax/src-tauri/src/conversation_store/context.rs`

由于 commentary 也被存成普通 assistant，所以后续请求会把这些“我先查一下”“我先去看文档”之类的工作中旁白当作正式 assistant 历史继续送回模型。

这不仅影响 UI，还会影响模型对话上下文质量。

## Codex 是怎么做的

## 核心结论

Codex 不是靠 `working_start` / `working_done` 去推断文本语义。

它的核心建模是：

- assistant 文本本身就是结构化 item
- item 自带 `phase`
- `phase` 取值是：
  - `commentary`
  - `final_answer`

### 1. Codex 的 assistant item 自带 phase

见：

- `/Volumes/Data/codex/codex-rs/protocol/src/models.rs`
- `/Volumes/Data/codex/codex-rs/protocol/src/items.rs`

`MessagePhase`：

```rust
pub enum MessagePhase {
    Commentary,
    FinalAnswer,
}
```

`AgentMessageItem`：

```rust
pub struct AgentMessageItem {
    pub id: String,
    pub content: Vec<AgentMessageContent>,
    pub phase: Option<MessagePhase>,
}
```

源码注释已经直接说明用途：

- 用来区分 mid-turn commentary 和 final answer
- 避免 UI 状态抖动

### 2. Codex 会把 phase 一路保留到线程历史和前端协议

见：

- `/Volumes/Data/codex/codex-rs/app-server-protocol/src/protocol/v2/item.rs`
- `/Volumes/Data/codex/codex-rs/app-server-protocol/schema/typescript/v2/ThreadItem.ts`
- `/Volumes/Data/codex/codex-rs/app-server-protocol/src/protocol/thread_history.rs`

也就是说：

- provider / core 层拿到 phase
- turn item 保留 phase
- thread history 保留 phase
- UI 消费的 `ThreadItem::AgentMessage` 仍然保留 phase

这是完整链路，而不是只在某一层临时判断一下。

### 3. Codex 的“中间输出”和“最终回答”是同类 item，不同 phase

这点很重要。

Codex 不是：

- 一类普通 assistant
- 一类 working marker

而是：

- 同一类 `agentMessage`
- `phase=commentary` 或 `phase=final_answer`

marker 只适合表达“当前 turn 是否还在进行中”，不适合承载 assistant 文本语义。

### 4. Codex 的 UI 也是按 phase 响应

见：

- `/Volumes/Data/codex/codex-rs/tui/src/chatwidget/streaming.rs`

`on_agent_message_item_completed()` 里有明确逻辑：

- `Commentary` 完成后，只恢复工作状态指示
- `FinalAnswer` 或 `None` 才按最终回答语义处理

也就是说 UI 的“工作中”状态是围绕消息 phase 运作的，而不是让 phase 依赖 UI 去猜。

### 5. Codex 对 phase 缺失时做兼容，但不依赖兼容路径

Codex 允许：

- `phase: None`

这是为了兼容不支持该字段的 provider / legacy 模型。

但它的结构是“能拿到 phase 时就保真保留”。这和 AgentJax 当前“从头到尾没有 phase”是两个层级的问题。

## 两套模型的本质差异

### AgentJax 当前模型

```text
assistant text
+ working_start / working_done markers
+ tool lines
=> 最后通过时序猜哪些 assistant 是 working，哪些是 final
```

问题：

- 猜测不稳定
- 持久化后语义丢失
- 回放和上下文重建都会出错

### Codex 模型

```text
agentMessage(phase=commentary)
tool items
agentMessage(phase=final_answer)
turn status / runtime status
```

优点：

- 语义在数据层就确定
- 历史回放不需要猜
- UI 只负责展示，不负责定义语义
- 上下文重建可以按 phase 做更细控制

## 对 AgentJax 的修正建议

## 建议方向

最接近 Codex 的修正方式是：

### 方案 A：最小修复

保留 `working_start / working_done`，但新增 assistant phase。

具体做法：

1. 为 `AssistantLine` 增加字段：
   - `phase?: "commentary" | "final_answer"`
2. 为 `ProviderStreamEvent::HopAssistantText` 增加 phase，而不是只有 `is_final`
3. provider 层如果能读到 message phase，就直接透传
4. runtime 层不要再用“是否有 pending tools”去猜文本语义
5. `persist_hop_assistant_line()` 按 phase 落盘
6. `load_context_for_request()` 至少要能区分：
   - commentary 是否回放
   - final answer 一定回放

这是当前代码最容易落地的一条线。

### 方案 B：更接近 Codex 的重构

逐步弱化 `working_start / working_done` 作为语义边界的职责，把它们降级成纯 UI/runtime 状态事件。

目标结构变成：

- `assistant(phase=commentary)`
- `tool`
- `assistant(phase=final_answer)`

而不是：

- `working_start`
- `assistant`
- `tool`
- `assistant`
- `working_done`

在这个模型下：

- working marker 只负责“现在还在干活”
- assistant line 自己负责“这段话是什么性质”

这才是 Codex 风格。

## 推荐的具体改动顺序

建议按下面顺序做，风险最低：

1. 在数据模型里给 `AssistantLine` 增加 `phase`
2. 在前端 `AssistantLine` 类型同步加 `phase`
3. 修改 provider 解析和 runtime 事件，让每段 assistant 文本都带 phase
4. 修改 `persist_hop_assistant_line()`，落盘 phase
5. 修改 `App.tsx` 的流式处理，按 phase 分流 draft assistant
6. 修改 `ChatArea.tsx`，按 phase 展示 commentary 和 final
7. 修改 `load_context_for_request()`，决定 commentary 是否回放给模型

## 一个非常关键的实现原则

不要再让 `working_start` / `working_done` 决定 assistant 文本属于 working 还是 final。

正确方向应该是：

- assistant 文本先天自带 phase
- `working_start` / `working_done` 只是运行状态提示

否则你即使修掉这次顺序问题，未来仍然会在以下情况继续错：

- 模型先说一句再调用工具
- 连续多轮 tool hop
- 工具后又有一段 commentary
- provider 流式顺序和 UI 事件顺序不完全一致

## 最终结论

当前 bug 不是单纯的“marker 插入时机不对”。

更本质的问题是：

- AgentJax 现在没有把 assistant 文本分成 `commentary` 和 `final_answer` 两个语义相位
- 所以系统只能依赖 `working_start` / `working_done` 和时间顺序去猜
- 而 Codex 的做法是把 `phase` 变成消息本身的一部分，并把这个字段贯穿 provider、runtime、history、UI 全链路

所以下一步修正的正确方向不是继续补 marker 判断，而是把 assistant phase 建模补上。
