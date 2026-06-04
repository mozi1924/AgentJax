# 多Agent框架重构计划

## 背景

当前 AgentJax 是一个**单 Agent** 架构，所有配置集中在 `~/.agentjax/config.yaml`（`AppConfig`），会话存储在 `~/.agentjax/sessions/`，记忆存储在 `~/.agentjax/memory/`。

目标是迁移为**多 Agent 框架**，结构为：

```
~/.agentjax/
├── config.yaml              # 共享的全局配置
├── plugins/                 # 插件目录（不变）
├── tmp/                     # 临时文件（新增）
├── cache/                   # 缓存（新增）
├── vector/                  # 全局向量存储（可选，用于全局知识库，或迁移到 agent 作用域）
└── agents/
    ├── main/                # 主 Agent Profile
    │   ├── agent.yaml       # Agent 专属配置
    │   ├── memories/
    │   │   ├── index.db     # 记忆索引（SQLite）
    │   │   └── raw/         # 原始记忆文件（.md）
    │   ├── sessions/        # 会话存储
    │   │   └── {id}/
    │   │       ├── metadata.json
    │   │       ├── messages.jsonl
    │   │       ├── lcm.db
    │   │       └── workspace/
    │   └── vector/          # 向量存储（RAG）
    └── coding/              # 示列Coding Agent Profile
        ├── agent.yaml
        ├── memories/
        ├── sessions/
        ├── vector/
        └── ...
```

---

## 当前架构分析

### `AppConfig` 结构（`config.rs`）

```rust
pub struct AppConfig {
    // ── 共享配置（应保留在 config.yaml）──
    pub language: String,
    pub providers: BTreeMap<String, ProviderConfig>,    // 多 Agent 共享
    pub mcp: McpConfig,                                 // MCP 服务器定义
    pub plugin_manager: PluginManagerConfig,            // 插件管理器

    // ── Agent 专属配置（应迁移到 agent.yaml）──
    pub active_provider: String,
    pub default_model: String,
    pub utility_small_model: String,
    pub request_timeout_seconds: u64,
    pub show_advanced_request_options: bool,
    pub enable_developer_tools: bool,
    pub prompt_composer: PromptComposerConfig,        // 提示词拼装器
    pub context_management: ContextManagementConfig,   // LCM + Street
    pub sub_agent: SubAgentConfig,                     // 子代理配置
    pub memory: MemoryConfig,                          // 记忆系统
    pub rag: RagConfig,                                // RAG/向量
    pub tool_manager: ToolManagerConfig,               // 工具管理
}
```

### 当前数据存储路径

| 数据类型 | 当前路径 | 目标路径 |
|---------|---------|---------|
| 主配置文件 | `~/.agentjax/config.yaml` | `~/.agentjax/config.yaml`（精简） |
| Agent 配置 | （在 config.yaml 内） | `~/.agentjax/agents/main/agent.yaml` |
| 会话存储 | `~/.agentjax/sessions/{id}/` | `~/.agentjax/agents/main/sessions/{id}/` |
| LCM 数据库 | `{sessions}/{id}/lcm.db` | `{agent_sessions}/{id}/lcm.db` |
| 记忆文件 | `~/.agentjax/memory/*.md` | `~/.agentjax/agents/main/memories/raw/*.md` |
| RAG 向量 | `~/.agentjax/rag/` | `~/.agentjax/agents/main/vector/` |
| 插件 | `~/.agentjax/plugins/` | `~/.agentjax/plugins/`（不变） |

### 相关模块

| 模块 | 需要修改 | 备注 |
|------|---------|------|
| `config/` | ★★★ | 核心：拆分 AppConfig，新增 AgentConfig |
| `agentjax_home.rs` | ★★★ | 路径解析：添加 agents/ 子目录支持 |
| `conversation_store/paths.rs` | ★★★ | 会话路径改为 agent 作用域 |
| `lcm/` | ★★ | LCM 数据库路径随 sessions 迁移 |
| `memory/` | ★★ | 记忆路径改为 agent 作用域 |
| `commands/chat.rs` | ★★★ | 需要同时加载 AppConfig + AgentConfig |
| `runtime/` | ★★ | runtime 接收 AgentConfig |
| `tools/` | ★★ | ToolCatalog 需要 AgentConfig 作用域 |
| `sub_agents/` | ★★ | 子代理会话路径需要 agent 作用域 |
| `rag/` | ★★ | 向量路径改为 agent 作用域 |
| `settings_ui_sections/` | ★★ | 设置 UI 适应新的结构 |
| `frontend/` | ★ | 前端可能需调整设置 |

---

## 分阶段实施计划

### Phase 1: 引入 `AgentConfig` 结构（预计 2-3 天）

**目标**：定义 `AgentConfig`，将其从 `AppConfig` 分离，建立配置分层机制。

具体任务：

1. **定义 `AgentConfig` 结构体**
   - 新建文件 `src-tauri/src/config/agent_config.rs`
   - 从 `AppConfig` 提取 agent 专属字段
   - `AgentConfig` 包含：
     ```rust
     pub struct AgentConfig {
         pub active_provider: String,
         pub default_model: String,
         pub utility_small_model: String,
         pub request_timeout_seconds: u64,
         pub show_advanced_request_options: bool,
         pub enable_developer_tools: bool,
         pub prompt_composer: PromptComposerConfig,
         pub context_management: ContextManagementConfig,
         pub sub_agent: SubAgentConfig,
         pub memory: MemoryConfig,
         pub rag: RagConfig,
         pub tool_manager: ToolManagerConfig,
     }
     ```

2. **精简 `AppConfig`**
   - 从 `AppConfig` 中移除上述字段
   - `AppConfig` 保留：`language`, `providers`, `mcp`, `plugin_manager`
   - 添加 `default_agent_id: String`（默认 agent，如 "main"）

3. **添加 `AgentRegistry`**
   - 管理多个 agent 配置
   - 从 `~/.agentjax/agents/` 目录发现 agent
   - 提供根据 `agent_id` 获取 `AgentConfig` 的方法

4. **定义 `AgentId` 类型**
   - `type AgentId = String` 或新类型
   - 作为多 Agent 环境的标识

**影响范围**：`config/mod.rs`, `config/schema.rs`, `AppConfig::normalize()`

---

### Phase 2: 目录结构与路径系统重构（预计 2-3 天）

**目标**：建立 `agents/{agent_id}/` 下的目录结构，更新所有路径解析。

具体任务：

1. **更新 `agentjax_home.rs`**
   - 新增路径函数：
     - `agents_dir()` → `~/.agentjax/agents/`
     - `agent_dir(agent_id)` → `~/.agentjax/agents/{agent_id}/`
     - `agent_config_path(agent_id)` → `~/.agentjax/agents/{agent_id}/agent.yaml`
     - `agent_sessions_dir(agent_id)` → `~/.agentjax/agents/{agent_id}/sessions/`
     - `agent_memories_dir(agent_id)` → `~/.agentjax/agents/{agent_id}/memories/`
     - `agent_vector_dir(agent_id)` → `~/.agentjax/agents/{agent_id}/vector/`
     - `tmp_dir()` → `~/.agentjax/tmp/`
     - `cache_dir()` → `~/.agentjax/cache/`

2. **更新 `config/io.rs`**
   - `load_config()` 只加载共享配置
   - 新增 `load_agent_config(agent_id)` 加载 agent 专属配置
   - 新增 `resolve_full_config(agent_id)` 合并两者

3. **更新 `conversation_store/paths.rs`**
   - 路径基准从 `agentjax_home/sessions/` 改为 `agent_sessions_dir(agent_id)/`
   - 需要传递 `agent_id` 参数

4. **更新 `lcm/lcm_store_path()`**
   - 随会话路径迁移

5. **更新 `memory/store.rs`**
   - 记忆文件存储从 `agentjax_home/memory/` 改为 `agent_memories_dir(agent_id)/raw/`

6. **更新 `rag/`**
   - 向量存储从 `agentjax_home/rag/` 改为 `agent_vector_dir(agent_id)/`

7. **确保 `ensure_*` 函数创建需要的目录**

**影响范围**：`agentjax_home.rs`, `config/io.rs`, `conversation_store/paths.rs`, `lcm/mod.rs`, `memory/store.rs`, `rag/`

---

### Phase 3: 配置加载链路与运行时适配（预计 3-4 天）

**目标**：所有使用 `AppConfig` 的代码改为使用合并后的完整配置或显式分层的 `(AppConfig, AgentConfig)`。

具体任务：

1. **更新 `config/mod.rs`**
   - 将 `load_config()` 扩展为 `load_full_config(agent_id)` 返回 `(AppConfig, AgentConfig)`
   - 或创建 `FullConfig { shared: AppConfig, agent: AgentConfig }`
   - 更新 `get_config_info()` 以反映新结构

2. **更新 `commands/chat.rs`**
   - `chat_stream` 命令改为加载完整配置
   - 将 agent 配置传递给 runtime

3. **更新 `runtime/engine.rs`**
   - `AgentRuntime::run_turn()` 接受 `AgentConfig`
   - 从 `AgentConfig` 读取模型、提示词、工具、子代理等配置

4. **更新 `tools/` 系统**
   - `ToolCatalog` 的构建使用 `AgentConfig.tool_manager`
   - 工具上下文 `ToolExecutionContext` 包含当前 `agent_id`

5. **更新 `sub_agents/`**
   - 子代理会话路径以父 agent 为基准
   - 子代理配置从 `AgentConfig.sub_agent` 读取

6. **更新 `lcm/`**
   - 总结模型配置从 `AgentConfig` 读取
   - LCM 配置从 `AgentConfig.context_management` 读取

**影响范围**：`commands/chat.rs`, `runtime/`, `tools/`, `sub_agents/`, `lcm/summarizer.rs`

---

### Phase 4: 设置 UI 适配与插件系统（预计 2-3 天）

**目标**：settings UI 反映新的配置结构，插件系统适配多 Agent。

具体任务：

1. **更新 settings UI sections**
   - `general.json` - 调整以反映 agent 选择
   - `providers.json` - 保留在共享层
   - `tools.json` - 移到 agent 层
   - `prompt_composer.json` - 移到 agent 层
   - `context_management.json` - 移到 agent 层
   - `memory.json` - 移到 agent 层
   - `mcp.json` - 保留在共享层
   - `plugin_manager.json` - 保留在共享层

2. **更新 Tauri 命令**
   - `get_settings_snapshot` - 合并共享 + agent 配置
   - `apply_settings_patch` - 分别写入 config.yaml 和 agent.yaml

3. **更新前端（如需要）**
   - Agent 选择器 UI
   - 各设置面板的归属指示

**影响范围**：`config/settings.rs`, `config/settings_ui.rs`, `commands/config.rs`, `src/components/settings/`

---

### Phase 5: 向后兼容与自动迁移（预计 2-3 天）

**目标**：现有用户升级时自动迁移数据到新结构，不丢失任何数据。

具体任务：

1. **检测旧结构**
   - 启动时检测 `~/.agentjax/sessions/` 是否存在
   - 检测 `~/.agentjax/config.yaml` 是否包含 agent 字段

2. **自动迁移 config.yaml**
   - 读取旧 `config.yaml`
   - 提取 agent 专属字段 → 写入 `agents/main/agent.yaml`
   - 将共享字段写回 `config.yaml`

3. **自动迁移会话**
   - 将 `~/.agentjax/sessions/{id}/` → `~/.agentjax/agents/main/sessions/{id}/`
   - 更新 LCM 数据库中存储的路径引用

4. **自动迁移记忆**
   - 将 `~/.agentjax/memory/*.md` → `~/.agentjax/agents/main/memories/raw/*.md`

5. **自动迁移 RAG**
   - 将 `~/.agentjax/rag/` → `~/.agentjax/agents/main/vector/`

6. **迁移标志**
   - 在 `config.yaml` 中添加 `migrated_version` 字段
   - 避免重复迁移

7. **回退机制**
   - 迁移前备份关键文件
   - 迁移失败时告警并保留原数据

**影响范围**：`config/io.rs`, `agentjax_home.rs`, 迁移工具函数

---

### Phase 6: 前端 Agent 管理 UI（预计 3-4 天）

**目标**：用户可以在前端创建、切换、管理多个 Agent。

具体任务：

1. **Agent 管理 Tauri 命令**
   - `list_agents` - 列出所有 agent
   - `create_agent` - 创建新 agent（复制模板）
   - `delete_agent` - 删除 agent
   - `switch_agent` - 切换当前 agent
   - `get_agent_config` - 获取 agent 配置

2. **前端 Agent 切换器**
   - 侧边栏或顶栏的 Agent 选择下拉
   - 显示当前 agent 名称/图标

3. **Agent 配置编辑器**
   - 在前端设置页面编辑 agent.yaml
   - 修改模型、工具、提示词等

**影响范围**：`commands/`, `src/components/`, `src/features/`

---

## 优先级与建议

### 关键路径

```
Phase 1 (AgentConfig) → Phase 2 (Paths) → Phase 3 (Runtime) → Phase 5 (Migration)
                                                                      ↓
                                                             Phase 4 (Settings UI)
                                                                      ↓
                                                             Phase 6 (Agent UI)
```

### MVP 范围

**Phase 1 + Phase 2** 是必须完成的基础设施重构，完成后代码已支持多 Agent 概念但前向兼容。

**Phase 3** 是运行时适配，完成后所有功能在 Agent 范围内正常运行。

**Phase 4 + Phase 5** 可以并行进行。

**Phase 6** 如果暂时不需要前端 Agent 管理，可以延后。

### 风险点

1. **路径硬编码**：需要检查所有硬编码路径，特别是 `conversation_store_path` 相关函数
2. **序列化兼容性**：config.yaml 格式变化后的序列化/反序列化兼容
3. **现有会话**：迁移过程中会话数据的完整性
4. **前端状态**：设置 UI 的状态管理需要适配双层配置

---

## 下一步建议

1. **先读当前代码**，确认上述分析是否完整（我已做了初步分析）
2. **从 Phase 1 开始**，定义 `AgentConfig` 结构体
3. **逐个字段确认归属**：哪些留在共享层、哪些进入 agent 层
4. **建立测试**：配置迁移的自动化测试
