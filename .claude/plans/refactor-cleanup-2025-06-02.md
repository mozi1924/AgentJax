# AgentJax 重构清理计划

**创建日期**: 2025-06-02  
**基于审查**: [全面代码审查报告]  
**当前分支**: main  
**状态**: 执行中

---

## 背景

经过对 246 个源文件的全面审查，发现了以下系统性问题：
- 三套子代理系统并行运行（SubAgentTool / TaskTool / background_jobs）
- 对话存储双写（JSONL + LCM SQLite）
- 19 个 Rust 编译警告（未使用导入、死代码、未读取字段）
- 15+ 个前端未使用类型、死组件、残留资源
- 重复类型定义和工具函数

---

## 第一阶段：清理（低风险，立即可做）

### 1.1 前端死代码清理

- [ ] 删除 `src/App.css`（184 行 Vite 模板残留样式）
- [ ] 删除 `src/assets/hero.png`（无任何导入）
- [ ] 删除 `src/assets/vite.svg`（无任何导入）
- [ ] 删除 `src/assets/react.svg`（无任何导入）
- [ ] 删除 `src/components/chat/ToolCallWidget.tsx`（死组件，已被 WorkLogPanel 替代）
- [ ] 删除 `src/features/sub_agents/types.ts` 或整个 `src/features/sub_agents/` 目录（前端零引用）
- [ ] 清理 `src/features/conversations/types.ts` 中 15+ 未使用类型（ConversationMessage, MessageRole, MessageStatus, RawConversationMessage, StreetItemSnapshot, StreetSource, StreetPriority, ToolCall, ToolCallStatus, RawToolCallTimelineEvent, RawFunctionCallContextItem）
- [ ] 清理 `src/features/memory/types.ts` 中未使用类型（MemorySearchResult, ParsedMemory）
- [ ] 清理 `src/features/settings/types.ts` 中未使用类型（SettingsModuleSchema, SettingsRegistry, SettingsPatchRequest）
- [ ] 清理 `src/features/conversations/conversationState.ts` 中未使用的 `mergeWithLocalDrafts`
- [ ] 修复 `src/features/i18n/locales/zh.json:228` 格式异常（3 个 key 挤在一行）

### 1.2 Rust 编译警告清零

- [ ] `street/mod.rs`: 移除未使用的 `Priority` pub re-export
- [ ] `sub_agents/mod.rs`: 移除未使用的 `SubAgentManager`, `SubAgentTask`, `sub_agent_event_to_chat_stream_event` pub use
- [ ] `tools/memory_tools.rs`: 移除未使用的 `std::path::PathBuf` import
- [ ] `commands/chat.rs`: 移除未使用的 `StreetItemStatus` import
- [ ] `street/manager.rs`: 移除未使用的 `prune_terminal_items()` + `TERMINAL_ITEM_RETENTION_MS` + `MAX_RETAINED_TERMINAL_ITEMS` + `get_pending_count()` + `cleanup_conversation()` + `unregister_event_channel()`
- [ ] `street/types.rs`: 移除未使用的 `Priority::from_str()`, `level()`, `meets_threshold()`
- [ ] `sub_agents/manager.rs`: 移除未使用的 `DEFAULT_MAX_CONCURRENT`, `mark_running()`, `append_progress()`
- [ ] `sub_agents/worktree.rs`: 移除或使用未读取的 `Worktree.branch` 字段
- [ ] `sub_agents/types.rs`: 移除未读取的 `ProgressMessage.turn_id` 字段 + 永远不被构造的 `SubAgentType::Terminate`
- [ ] `tools/catalog/snapshot.rs`: 处理未读取的 `app_config` 字段
- [ ] `lcm/mod.rs`: 移除未使用的 `now_ms` 变量及 silence hack
- [ ] `runtime/tests.rs`: 移除未使用的 `timeline_events` 变量

### 1.3 模块可见性修正

- [ ] `lib.rs`: 将 9 个 `pub mod` 改为 `pub(crate)`（error, lcm, mcp, memory, plugin_runtime, provider_api, runtime, sub_agents, tools）

### 1.4 移除 #[allow(dead_code)] 并删除对应死代码

- [ ] `conversation_store.rs`: 删除 `conversations_dir_path()`, `ensure_conversations_dir()`
- [ ] `error_classifier.rs`: 整个文件清理（评估是否保留）
- [ ] `conversation_store/context/types.rs`: 移除未使用字段 `estimated_tokens`, `tool_call_count`, `message_count`
- [ ] `conversation_store/context/budget.rs`: 移除未使用字段 `context_window`, `model_id` 和未使用的 `unlimited()` constructor
- [ ] `conversation_store/context/token_usage.rs`: 评估 `count_conversation_context_tokens()`, `count_tool_schema_tokens()` 是否仅用于测试
- [ ] `provider_api/registry.rs`: 移除或评估 `unregister_plugin_provider()`, `provider_definitions()`
- [ ] `provider_api/circuit_breaker.rs`: 移除未使用的 `aggressive()`, `lenient()`
- [ ] `plugin_runtime/manifest.rs`: 移除未使用的 `PluginToolKind::Resource`, `PluginToolKind::Prompt`
- [ ] `plugin_runtime/hooks.rs`: 移除未使用的 `ContextHookPoint::OnBeforeTruncation`

---

## 第二阶段：消除并行系统（中风险，需要评估影响面）

### 2.1 Deprecate TaskTool

- [ ] `lcm/tools/task.rs`: 在 tool description 中添加 "Deprecated: use sub_agent tool instead"
- [ ] 验证 `sub_agent` tool 完全覆盖 `task` tool 的功能
- [ ] 一个版本后移除此 tool

### 2.2 统一子代理系统

- [x] 评估 `background_jobs` 是否可以迁移到 `sub_agents` 框架
- [x] **结论：保留独立** — 两个系统服务不同目的（单工具异步执行 vs 完整 agent loop），强行合并弊大于利

### 2.3 移除 JSONL 双写

- [x] 确定 LCM 作为唯一数据源
- [x] 将 JSONL 写入路径改为可选项（feature flag）
- [x] 让 JSONL 读取路径保留作为 fallback 兼容

### 2.4 合并重复定义

- [x] 合并两个 `ConversationMeta` 结构体为一个
- [x] 合并 `now_unix_ms()` 重复定义
- [x] 统一 `title_source` 值处理

---

## 第三阶段：架构加固（需要更多测试覆盖）

### 3.1 Error 系统完善

- [ ] 完成 `error_classifier.rs` 的 `AgentJaxError` 迁移
- [ ] 确认所有 provider streaming 错误路径使用统一错误类型

### 3.2 LCM 优化

- [ ] 修复 `ToolCatalog::new()` 中的 dummy in-memory LCM store（避免无效初始化）
- [ ] 提取 LcmConfig 参数：grep page_size 等硬编码值 → 可配置项
- [ ] 优化 `lcm_store_path()` 每次都 `create_dir_all` 的性能问题

### 3.3 会话列表增强

- [ ] `list_conversations()` 支持从 LCM 元数据表查询
- [ ] 统一 LCM 和 JSONL 的会话发现逻辑

### 3.4 Plugin 系统清理

- [ ] 移除 `register_manifest()` 遗留包装器（调用者已迁移到 `register_package()`）
- [ ] 评估 `PluginRuntime` trait 是否需要作为 trait 存在

---

## 执行日志

| 日期 | 阶段 | 步骤 | 状态 |
|------|------|------|------|
| 2025-06-02 | 1 | 1.1 前端死代码清理 | ✅ |
| 2025-06-02 | 1 | 1.2 Rust 编译警告清零 (19→0) | ✅ |
| 2025-06-02 | 1 | 1.3 模块可见性 (pub→pub(crate)) | ✅ |
| 2025-06-02 | 1 | 1.4 暴露并清理 90+ 死代码 (0 warnings) | ✅ |
| 2025-06-02 | 2 | 2.1 Deprecate TaskTool | ✅ |
| 2025-06-02 | 2 | 2.4 合并 now_unix_ms() 重复定义 | ✅ |
| | | | |
| 2025-06-02 | 1 | 1.2 Rust 编译警告清零 (19→0) | ✅ |
| 2025-06-02 | 1 | 1.3 模块可见性 (street: pub→pub(crate)) | ✅ |
| 2025-06-02 | 2 | 2.1 Deprecate TaskTool | ✅ |
| 2025-06-02 | 2 | 2.4 合并 now_unix_ms() 重复定义 | ✅ |
| 2025-06-02 | 2 | 2.3 JSONL 双写 → 可选备份 (默认开启) | ✅ |
| 2025-06-02 | 2 | 2.2 background_jobs 评估 | ✅ (评估后保留) |
| 2025-06-02 | 2 | 2.4 合并 ConversationMeta 结构体 | ✅ |
| 2025-06-02 | 2 | 2.4 统一 title_source 值处理 | ✅ |
| | | | |

## 当前状态摘要

- **编译**: 零警告，零错误
- **测试**: 325 passed, 0 failed
- **第二阶段完成**: 2.1 (TaskTool deprecation), 2.2 (评估后保留), 2.3 (JSONL 可选备份), 2.4 (ConversationMeta 合并 + title_source 统一)
- **待处理**: 第三阶段 (架构加固: 3.1-3.4)

## 2.2 评估结论

`background_jobs` 和 `sub_agents` 服务不同目的，不宜强行合并：
- `background_jobs`: 单工具调用的异步执行（立即 spawn，无 agent loop）
- `sub_agents`: 完整 LLM agent 的异步执行（延迟 spawn，有 agent loop）
- 共享的注册表样板代码 (~150 行) 已足够简洁，无需泛型抽象

## 2.4 ConversationMeta 合并详情

### 统一结构体 (在 `lcm/types.rs`)
- 新增字段: `version: u32`, `last_message_preview: String`
- `metadata: BTreeMap<String, Value>` 替代 `metadata_json: String` (LCM SQL 层序列化/反序列化)
- `message_count: u32` 统一类型 (JSON store 原来用 `usize`)
- `conversation_store/types.rs` 移除重复定义，改为 `pub use crate::lcm::types::ConversationMeta`

### title_source 统一
- 全链路统一为 "manual" / "auto" / "pending"
- LCM 读/写路径增加 `normalize_title_source()` 调用
- `update_conversation_meta` 参数 `metadata_json: Option<&str>` → `metadata: Option<&BTreeMap<String, Value>>`

## 2.3 JSONL 备份详情

### 新增配置
- `AppConfig.conversation.jsonl_backup_enabled: bool` (默认 `true`)
- 设置 UI: "对话" → "存储" → "JSONL 备份" 开关

### 写入路径（已条件化）
- `chat_persistence::persist_tool_progress_event` — 当 `false` 时立即返回
- `chat_persistence::persist_assistant_line` — 当 `false` 时立即返回
- `chat_stream_observer` — 透传 backup flag
- `chat::chat_stream` — 用户消息 `append_line` 条件化
- `update_conversation_token_usage` — 条件化

### 保持不变
- `ensure_conversation` — 总是运行（创建 metadata.json + LCM 元数据）
- `update_auto_title` — 总是运行（元数据，非消息日志）
- `update_conversation_dynamic_tools` 等 — 总是运行（元数据）
- JSONL 读取路径 — 保持作为 fallback（现有对话兼容）
