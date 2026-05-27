# Codex 式旁白 / 最终回答链路排查

## 这次排查要回答什么

这次主要确认两件事：

1. AgentJax 里的“旁白”到底是模型真实输出的，还是最后由 API / 前后端重新拼出来的。
2. 当前实现和 `/Volumes/Data/codex/codex-rs` 相比，真正的差距在哪里。

结论先放前面：

- 当前 AgentJax 已经不是“纯 UI 猜测”模式，而是有明确的 `commentary` / `final_answer` 相位链路。
- “最终回答不要重复旁白”这件事，当前主要是后端 runtime 主动做的，不是前端渲染时临时过滤。
- 也就是说，最终回答里是否不重复旁白，不完全取决于模型自觉；框架本身已经在做一次去重/清洗。
- 但 UI 层和会话摘要层仍然把 commentary 当成普通 assistant 文本的一部分来计数、预览和展示，这就是当前“摸不着头脑”的主要来源之一。

## 现在的 AgentJax 到底怎么跑

## 1. 提示词层已经要求模型区分旁白和最终回答

见：

- `/Volumes/Data/AgentJax/src-tauri/src/config/constants.rs`

内置 system block 已明确要求：

- commentary 只用于进度更新
- final answer 必须和 commentary 分开
- final answer 不要复述 earlier commentary

所以从提示词设计上说，框架本来就在模仿 Codex 的双相位语义。

## 2. provider 流解析层已经能识别 `phase`

见：

- `/Volumes/Data/AgentJax/src-tauri/src/providers/responses/stream/parser.rs`
- `/Volumes/Data/AgentJax/src-tauri/src/message_phase.rs`

当前解析器已经做了这些事：

- 在 `response.output_item.added` / `response.output_item.done` 中读取 assistant message 的 `phase`
- 识别 `commentary` 和 `final_answer`
- 把 `item_id -> phase` 记到 `assistant_message_phase_by_item`
- 在 `response.output_text.delta` 里把 phase 一起带出来

这点很重要，因为它说明：

- phase 不是前端自己猜的
- 也不是 chat command 事后硬编码猜的
- 它来自流式响应里的 assistant item 元数据

## 3. runtime 会主动挑选“最终回答”，并去掉前置旁白

见：

- `/Volumes/Data/AgentJax/src-tauri/src/runtime/engine.rs`

这里有三段关键逻辑：

### `resolve_hop_phase`

如果 provider 没给 phase，就按 hop 是否还有 pending tools 推断：

- 还有工具：默认 `commentary`
- 没有工具：默认 `final_answer`

这和 codex-rs 的兼容思路一致，都是“phase 有则用之，没有则降级”。

### `select_final_output_text`

runtime 会优先从 assistant message items 里选“最后一个不是 commentary 的消息”作为最终答案。

### `strip_commentary_prefixes`

runtime 还会维护一份 `commentary_history`，然后把最终文本开头与历史旁白逐行对上的部分剥掉。

也就是说，如果模型输出了这种东西：

```text
我先检查文件。
接下来我运行测试。
已经修复完成。
```

而前两行已经作为 commentary 出现过，runtime 会把最终结果清成：

```text
已经修复完成。
```

这已经明确回答了核心问题：

## “最终回答不重复旁白”目前主要是后端 runtime 在做，不是单靠模型自律

## 4. provider 返回的 `output_text` 也优先取 final，而不是简单拼全量 delta

见：

- `/Volumes/Data/AgentJax/src-tauri/src/providers/responses/stream/transport.rs`
- `/Volumes/Data/AgentJax/src-tauri/src/providers/responses/stream/parser.rs`

当流结束后，如果实时累计的 `output_text` 为空，transport 会回退到：

- `extract_output_text(root)`

而 `extract_output_text()` 的第一步就是：

- `extract_final_output_text(root)`

它会优先选：

- 最后一个 `phase != commentary` 的 assistant message

如果没有 phase，再退化到最后一条 assistant message。

这说明当前 `ChatResponse.output_text` 的设计目标本身就是：

- “最终答案文本”

而不是：

- “commentary + tool narration + final answer 的完整拼接”

## 5. chat command 和持久化层把 commentary / final 分开落盘

见：

- `/Volumes/Data/AgentJax/src-tauri/src/commands/chat.rs`
- `/Volumes/Data/AgentJax/src-tauri/src/commands/chat/chat_persistence.rs`

当前行为是：

- `AssistantMessageCompleted` 且 `phase == commentary` 时，持久化 commentary
- `HopAssistantText` 且 `phase != commentary` 时，持久化 final / unknown

这意味着现在磁盘上的 `messages.jsonl` 已经可以保存：

- assistant text
- phase

而不是像更早版本那样完全混在一起。

## 6. 会话上下文回放也会保留 `phase`

见：

- `/Volumes/Data/AgentJax/src-tauri/src/conversation_store/context.rs`

`build_assistant_input_item()` 会把 assistant line 转成：

- `type: "message"`
- `role: "assistant"`
- `content: [...]`
- 如果有 phase，就带上 `phase`

所以 commentary 并没有在回放历史时丢失语义。

这点和 codex-rs 的设计方向是一致的。

## 现在“看起来诡异”的主要问题在哪

## 1. 前端显示虽然区分样式，但会话级语义还不够干净

见：

- `/Volumes/Data/AgentJax/src/components/ChatArea.tsx`
- `/Volumes/Data/AgentJax/src/features/conversations/sessionState.ts`
- `/Volumes/Data/AgentJax/src/features/conversations/conversationUtils.ts`

当前前端已经做了视觉区分：

- `phase === commentary` 时用小号、缩进、加载图标
- `phase !== commentary` 时按正式 assistant 消息渲染

但还有几个地方仍把 commentary 当“普通 assistant 成果”处理：

- `countUserAndDoneAssistant()` 统计 done assistant 时不区分 phase
- `lastMessagePreview` 可能被 commentary 更新
- 侧边栏标题 fallback 也可能落到 commentary 文本

这会带来几个体验问题：

- 侧边栏预览可能显示“我先看一下代码”
- 消息数会把旁白也算进去
- 用户会误以为 commentary 是正式回复的一部分

这更像是产品层 / UI 层语义没收干净，不是 phase 链路缺失。

## 2. 流式过程中仍有“相位切换时序”带来的理解负担

当前事件流里同时存在两类 assistant 文本事件：

- `delta`
- `assistant_message`

见：

- `/Volumes/Data/AgentJax/src-tauri/src/commands/chat/chat_events.rs`
- `/Volumes/Data/AgentJax/src/hooks/useConversationStreaming.ts`

前端用 `delta` 做草稿流式显示，再用 `assistant_message` 收口为 done。

这个策略本身没错，但用户看到的“旁白块”和“最终回答块”之间是否足够稳定，取决于：

- provider 是否稳定发 phase
- 每个 hop 结束时 `assistant_message` 到达的时机
- 是否存在 phase 缺失导致的 fallback 推断

所以现在的“摸不着头脑”，更像是：

- 底层 phase 语义已有
- 但前端在“列表/摘要/计数/过渡时机”上没有完全按 phase 建模

## 3. `applyAssistantDelta()` 仍按 request 维度复用草稿，没按 phase 强约束

见：

- `/Volumes/Data/AgentJax/src/features/conversations/sessionState.ts`

`applyAssistantDelta()` 只要发现最后一条是同一个 `requestId` 的 draft assistant，就继续追加。

它会更新 phase，但不会先校验“这个草稿是否属于当前 phase”。

在大多数正常流里这问题不大，因为：

- commentary hop 结束后通常会先收到 `assistant_message` 把它收口成 done
- 下一阶段 final delta 到来时会新建 draft

但如果上游 phase 发得不稳定，或者某些 provider 的 delta / done 时序不同，这里会是潜在抖动点。

## AgentJax 和 codex-rs 的关键差距

## 1. 核心建模方向其实已经接近 codex-rs

Codex 的核心不是 `working_start` / `working_done` 去猜文本语义，而是：

- assistant message item 自带 `phase`
- phase 一路保留到历史和 UI

见：

- `/Volumes/Data/codex/codex-rs/protocol/src/models.rs`
- `/Volumes/Data/codex/codex-rs/protocol/src/items.rs`
- `/Volumes/Data/codex/codex-rs/app-server-protocol/src/protocol/thread_history.rs`

AgentJax 现在在这些点上已经基本对齐：

- 有 `AssistantPhase`
- provider parser 保留 phase
- conversation store 保留 phase
- 前端也能读 phase

所以大方向并没有跑偏。

## 2. codex-rs 更成熟的地方在“UI 消费 phase 的一致性”

见：

- `/Volumes/Data/codex/codex-rs/tui/src/chatwidget/streaming.rs`

codex-rs 会把 `phase` 直接用于：

- commentary 完成时机
- 状态指示器恢复时机
- final answer 的 transcript 记录

也就是说它不是“只把 phase 存下来”，而是整个 UI 状态机都围着 phase 转。

AgentJax 现在更像是：

- 数据模型已经知道 phase
- 但列表预览、计数、摘要、流式过渡，还没完全 phase-aware

## 3. AgentJax 还保留了额外的一层“文本清洗”

Codex 的主路径更偏向：

- 保留 item 语义
- UI 按 phase 消费 item

AgentJax 目前则多了一层：

- `strip_commentary_prefixes()`

这层是很实用的兜底，但也说明我们当前仍不完全信任：

- provider 的 final message phase
- 模型不会复述 commentary

换句话说：

- codex-rs 更像“结构优先”
- AgentJax 现在是“结构优先 + 文本补丁兜底”

## 这次排查能下的结论

## 关于“是 API 拼出来的还是模型复述的”

更准确的说法是：

- commentary 文本本身通常是模型真实输出的
- final answer 文本也通常来自模型真实输出
- 但 AgentJax runtime 会主动挑选 final message，并剥掉与历史 commentary 完全重合的前缀行

所以最终用户看到的 `output_text` 不是原封不动的“模型最后一段原文”，而是：

- 模型输出
- 经过 phase 选择
- 再经过 commentary-prefix 清洗

因此答案不是二选一。

不是“纯模型复述”。

也不是“前端最后乱拼一坨”。

而是：

- 后端按照 phase 结构选 final
- 再做一次轻量文本去重

## 关于“显示摸不着头脑”更像哪一层的问题

更像前端/产品语义问题，次要才是模型行为问题。

具体说：

- phase 链路已经存在
- 过滤逻辑已经存在
- 真正不稳的是 preview / count / summary / draft merge 这些 UI 行为还没有彻底 phase-aware

## 本次验证

已跑通现有单测：

- `cargo test strips_leading_commentary_lines_from_final_answer --manifest-path src-tauri/Cargo.toml`
- `cargo test selects_unknown_phase_message_as_final_output_when_no_final_phase_exists --manifest-path src-tauri/Cargo.toml`

这两条测试直接验证了：

- 框架会剥掉与 commentary 重复的前缀行
- 即使 phase 不完整，也会尽量把最终回答从 commentary 中分离出来

## 下一步建议

如果下一步要继续模仿 codex-rs，我建议优先做这三件事：

1. 把前端会话摘要彻底 phase-aware。
   - sidebar preview、messageCount、fallback title 默认忽略 commentary。

2. 把流式草稿状态机也做成 phase-aware。
   - `applyAssistantDelta()` 不只按 `requestId` 复用草稿，也要考虑 phase。

3. 给 runtime 增加更强的观测日志或调试开关。
   - 直接记录“原始 assistant items / phase / 选中的 final text / strip 前后结果”，这样以后能快速判断到底是模型复述，还是 runtime 清洗。

如果要直接进入实现阶段，最合适的起点是：

- `/Volumes/Data/AgentJax/src/features/conversations/conversationUtils.ts`
- `/Volumes/Data/AgentJax/src/features/conversations/sessionState.ts`
- `/Volumes/Data/AgentJax/src/components/ChatArea.tsx`

因为第一波收益最大的问题，已经主要落在前端语义收口上了。
