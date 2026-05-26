# AgentJax Backend Tool Loop Fix Plan

## 诊断总结

经过对后端代码的全面审查，发现工具调用循环存在 **一个致命缺陷 + 四个次要缺陷**。

---

## 🔴 致命缺陷：Tool Loop 上下文丢失

### 位置
`src-tauri/src/runtime/engine.rs` — `AgentRuntime::run_turn()` 的主循环

### 根因

```rust
// engine.rs — 循环中每次迭代的延续输入构建

// 第一次迭代：
//   input_items = [完整对话历史, user_message]
//   context_items = input_items.clone()  → 完整上下文

// 工具执行后：
//   continuation_delta_items = compose_tool_continuation_input(
//       collected.response_result.output_items,  // 只有 reasoning + function_call
//       tool_results_items,                        // function_call_output
//   )
//   next_input_items = Some(continuation_delta_items)  // ❌ 只有最新跳的输出

// 第二次迭代：
//   input_items = continuation_delta_items  // ❌ 历史全丢！
//   context_items = input_items.clone()      // ❌ 被覆盖！
//   传给模型的内容 = [reasoning, tool_call, tool_output]
//   模型看到的内容：没有任何历史、没有原始问题
```

**根本原因**：
1. `store: false` — 服务端不保存对话状态
2. 没有 `previous_response_id` — 无法做服务端状态链
3. `next_input_items` 只包含当前跳的模型输出 + 工具结果，不包含累积历史

**影响**：工具执行完毕后，模型收到的延续请求里只有当前跳的工具调用/结果，完全丢失了原始用户问题、对话历史和之前跳的上下文。这就是"工具调用完回来就忘了上下文"的原因。

---

## 🟡 次要缺陷

### 1. `output_items` 累积不完整
`accumulator.output_items` 只在 `ensure_tool_call_output_pairs` 中收集了 `function_call_output` 类型的项目。但 `function_call` 项目（和 `reasoning` 项目）没有进入最终结果。这意味着前端收到的 `output_items` 缺少工具调用的配对信息。

### 2. `absorb_response` 只累积文本和空壳 output_items
`absorb_response` 对 `ResponseStreamResult` 的 `output_items` 做完整收集，但在同一个循环中 `response_result.output_items`（来自 `CollectedProviderTurn`）里可能没有完整的 output items — 它们是通过事件流逐步解析的。

### 3. 无 `previous_response_id` 支持
`ResponseStreamRequest` 和 `ProviderTurnRequest` 结构体没有 `previous_response_id` 字段。`build_streaming_request_payload` 主动过滤掉 `previous_response_id`。这意味着即使想用 Responses API 的原生续接能力也无法使用。

### 4. 恢复机制脆弱
`build_recovery_developer_note` 依赖持久化的 `context_items`，但持久化是在流事件期间逐步进行的（`persist_tool_progress_event`），如果崩溃发生在中间状态，恢复的上下文可能不完整。

---

## 修复方案

### 核心修复：全量上下文重放 (Full Context Replay)

由于使用 `store: false`（不依赖服务端存储），每次工具循环延续都必须发送 **完整的累积上下文**。

#### 修改 `engine.rs` 中的循环逻辑

**当前逻辑**：
```rust
next_input_items = Some(continuation_delta_items); // 仅上一跳的输出
```

**修复后**：
```rust
// continuation_delta_items 包含 [reasoning, function_call, function_call_output]
// 将所有累积的上下文项目（包括历史 + 当前跳）传递给下一次迭代
let full_context = build_full_continuation_input(
    &mut context_items,          // 累积的完整上下文（带历史）
    continuation_delta_items,    // 当前跳的新项目
);
next_input_items = Some(full_context);
```

`context_items` 不再在每次迭代中被覆盖，而是持续累积所有项目。

#### 具体改动

1. **`engine.rs` — `run_turn()`** ✅ 已完成:
   - 移除 `context_items = input_items.clone()` 的覆盖赋值
   - 引入 `base_context`：首次构建 [历史 + 用户消息]，保持不变
   - 引入 `accumulated_context`：从 Hop 1 开始累积所有 delta
   - 后续迭代时 `input_items` = 完整的 `accumulated_context`（已累积所有历史 + 所有跳）
   - `previous_response_id` 作为 hint 传递给 API（不依赖其管理上下文）

2. **`TurnAccumulator` 增强** ✅ 已完成:
   - 新增 `absorb_continuation_batch()` 方法，收集完整 hop delta
   - `output_items` 现在包含 reasoning + function_call + function_call_output，不只是 tool outputs

3. **`build_request()` 扩展** ✅ 已完成:
   - 新增 `previous_response_id` 参数
   - 每次延续请求都带上上一次的 response_id

### 次要修复

#### A. 添加 `previous_response_id` 支持 ✅ 已完成

- `ProviderTurnRequest` 新增 `previous_response_id: Option<String>` 字段
- `build_streaming_request_payload` 在 payload 中传递 `previous_response_id`
- `extra_body` 中保留对 `store` 和 `previous_response_id` 的过滤（防止配置泄露）
- 策略：使用 `previous_response_id` 作为续接 hint，但 `store: false` 始终不变，本地上下文全量重放是权威来源

#### B. 改进恢复机制 ✅ 已完成

`build_recovery_developer_note` 增强：
- 区分"已完成的 tool calls"和"未解决的 tool calls"
- 添加 `interruption_reason` 字段，描述中断原因
- 更详细的指引：告诉模型不要重复已完成的工作，先检查上下文
- 覆盖三种场景：assistant 消息缺失、tool calls 未解决、两者兼有

#### C. 编译与测试 ✅ 已完成

- `cargo check` 通过，无编译错误
- `cargo test`：63 passed, 0 failed, 1 ignored（ignored = 需要真实网关的集成测试）

---

## 文件变更清单

| 文件 | 变更类型 | 说明 |
|------|----------|------|
| `src-tauri/src/runtime/engine.rs` | 🔧 重写 | 修复核心循环，上下文全量重放 |
| `src-tauri/src/providers/types.rs` | ➕ 扩展 | `ResponseStreamRequest` 增加 `previous_response_id` |
| `src-tauri/src/providers/responses/stream/payload.rs` | 🔧 修改 | 允许接收 `previous_response_id`，不再过滤 |
| `src-tauri/src/conversation_store/recovery.rs` | 🔧 增强 | 改进恢复提示的完整性 |
| `src-tauri/src/runtime/tests.rs` | ➕ 新增 | 添加上下文保持相关的单元测试 |

---

## 实施步骤

1. ✅ 诊断完成 — 本文档
2. 修复 `engine.rs` 核心循环（上下文全量重放）
3. 修复 `output_items` 累积
4. 添加 `previous_response_id` 到请求模型
5. 改进恢复机制
6. 编写测试验证
7. 手动端到端测试
