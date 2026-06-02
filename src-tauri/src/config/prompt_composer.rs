use crate::config::constants::{
    BUILTIN_CORE_SYSTEM_BLOCK_CONTENT, BUILTIN_CORE_SYSTEM_BLOCK_ID, BUILTIN_CORE_SYSTEM_SOURCE_ID,
    BUILTIN_CORE_SYSTEM_TITLE,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// ── Enums ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PromptBlockRole {
    #[default]
    System,
    Developer,
}

impl PromptBlockRole {
    #[allow(dead_code)] // Reserved API
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Developer => "developer",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PromptBlockSource {
    #[default]
    User,
    Builtin,
    Plugin,
}

impl PromptBlockSource {
    #[allow(dead_code)] // Reserved API
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Builtin => "builtin",
            Self::Plugin => "plugin",
        }
    }
}

// ── Structs ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PromptBlock {
    pub id: String,
    pub title: String,
    pub role: PromptBlockRole,
    pub content: String,
    pub enabled: bool,
    pub source: PromptBlockSource,
    pub source_id: Option<String>,
    pub locked: bool,
}

/// User-facing prompt composer configuration.
///
/// Internally stores the **fully resolved** block list (both user-defined and
/// built-in blocks merged).  When serialized to YAML, built-in/plugin blocks
/// are abbreviated to only `{id, enabled}` — see
/// [`Self::abbreviated_for_yaml`].  The abbreviated form is transparently
/// expanded back during [`normalize_prompt_composer`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PromptComposerConfig {
    pub blocks: Vec<PromptBlock>,
}

#[derive(Debug, Clone)]
pub struct CompiledPromptAssembly {
    pub instructions_text: String,
    pub developer_items: Vec<Value>,
    #[allow(dead_code)] // Reserved API
    pub preview_markdown: String,
}

// ── Defaults ───────────────────────────────────────────────────────────────

impl Default for PromptBlock {
    fn default() -> Self {
        Self {
            id: "prompt-block".to_string(),
            title: "Prompt block".to_string(),
            role: PromptBlockRole::System,
            content: String::new(),
            enabled: true,
            source: PromptBlockSource::User,
            source_id: None,
            locked: false,
        }
    }
}

impl Default for PromptComposerConfig {
    fn default() -> Self {
        Self {
            blocks: default_builtin_blocks(),
        }
    }
}

/// Return the canonical list of built-in (framework-provided) prompt blocks.
///
/// Each built-in block carries the full `title`, `content`, `role`, `source`,
/// `source_id` and `locked` flag.  These properties are **not** stored in the
/// user's config YAML — only the `id` and `enabled` state are persisted.
pub fn default_builtin_blocks() -> Vec<PromptBlock> {
    vec![PromptBlock {
        id: BUILTIN_CORE_SYSTEM_BLOCK_ID.to_string(),
        title: BUILTIN_CORE_SYSTEM_TITLE.to_string(),
        role: PromptBlockRole::System,
        content: BUILTIN_CORE_SYSTEM_BLOCK_CONTENT.to_string(),
        enabled: true,
        source: PromptBlockSource::Builtin,
        source_id: Some(BUILTIN_CORE_SYSTEM_SOURCE_ID.to_string()),
        locked: true,
    }]
}

/// Build a lookup map from built-in block ID → PromptBlock.
fn builtin_block_map() -> std::collections::BTreeMap<String, PromptBlock> {
    let mut map = std::collections::BTreeMap::new();
    for block in default_builtin_blocks() {
        map.insert(block.id.clone(), block);
    }
    map
}

// ── Normalization ──────────────────────────────────────────────────────────

/// Normalize the prompt composer after deserialization from YAML.
///
/// 1. Detects abbreviated built-in/plugin blocks (only `id` + `enabled` in
///    the config) and fills in their full properties from the canonical
///    definitions.
/// 2. Auto-restores any built-in blocks that are missing from the config.
/// 3. Sorts the final list so that all **system** blocks appear before
///    **developer** blocks.
pub fn normalize_prompt_composer(composer: PromptComposerConfig) -> PromptComposerConfig {
    let builtins = builtin_block_map();

    // Separate user blocks from builtin/plugin references.
    let mut user_blocks: Vec<PromptBlock> = Vec::new();
    let mut builtin_order: Vec<(String, bool)> = Vec::new(); // (id, enabled)

    for block in composer.blocks {
        let normalized = normalize_block(block);
        if builtins.contains_key(&normalized.id) {
            // Even if deserialized as `source: User` (because the YAML was
            // abbreviated and serde filled defaults), treat it as builtin.
            builtin_order.push((normalized.id, normalized.enabled));
        } else {
            user_blocks.push(normalized);
        }
    }

    // Resolve builtin blocks: merge with canonical definitions.
    // Locked blocks are always enabled (cannot be disabled).
    let mut resolved_builtins: Vec<PromptBlock> = Vec::new();
    for (builtin_id, canonical) in &builtins {
        let enabled = builtin_order
            .iter()
            .find(|(id, _)| id == builtin_id)
            .map(|(_, e)| *e)
            .unwrap_or(true); // default to enabled
        let mut block = canonical.clone();
        block.enabled = if block.locked { true } else { enabled };
        resolved_builtins.push(block);
    }

    // Merge: system blocks first, then developer blocks.
    // Within each role: user blocks first (preserving config order), then
    // builtin blocks (in canonical order).
    let mut system_blocks: Vec<PromptBlock> = Vec::new();
    let mut developer_blocks: Vec<PromptBlock> = Vec::new();

    for block in user_blocks {
        match block.role {
            PromptBlockRole::System => system_blocks.push(block),
            PromptBlockRole::Developer => developer_blocks.push(block),
        }
    }
    for block in resolved_builtins {
        match block.role {
            PromptBlockRole::System => system_blocks.push(block),
            PromptBlockRole::Developer => developer_blocks.push(block),
        }
    }

    system_blocks.extend(developer_blocks);
    PromptComposerConfig {
        blocks: system_blocks,
    }
}

/// Return a JSON value suitable for YAML serialization.
///
/// Built-in and plugin blocks are abbreviated to only `{id, enabled}` so the
/// user's config file is clean and does not expose framework-internal content.
pub fn abbreviate_prompt_composer_for_yaml(config: &PromptComposerConfig) -> Value {
    let blocks: Vec<Value> = config
        .blocks
        .iter()
        .map(|block| {
            if block.source == PromptBlockSource::User {
                // User blocks are serialized in full.
                serde_json::to_value(block).unwrap_or_default()
            } else {
                // Built-in / plugin blocks: only id + enabled.
                json!({
                    "id": block.id,
                    "enabled": block.enabled
                })
            }
        })
        .collect();
    json!({ "blocks": blocks })
}

// ── Compilation ────────────────────────────────────────────────────────────

pub fn compile_prompt_composer(composer: &PromptComposerConfig) -> CompiledPromptAssembly {
    let active_system_blocks = composer
        .blocks
        .iter()
        .filter(|block| block.enabled && block.role == PromptBlockRole::System)
        .filter_map(|block| {
            let content = block.content.trim();
            if content.is_empty() {
                None
            } else {
                Some((block.title.trim(), content))
            }
        })
        .collect::<Vec<_>>();

    let instructions_text = active_system_blocks
        .iter()
        .map(|(_, content)| *content)
        .collect::<Vec<_>>()
        .join("\n\n");

    let developer_blocks = composer
        .blocks
        .iter()
        .filter(|block| block.enabled && block.role == PromptBlockRole::Developer)
        .filter_map(|block| {
            let content = block.content.trim();
            if content.is_empty() {
                None
            } else {
                Some((block.title.trim(), content))
            }
        })
        .collect::<Vec<_>>();

    let developer_items = developer_blocks
        .iter()
        .map(|(_, content)| {
            json!({
                "role": "developer",
                "content": [{
                    "type": "input_text",
                    "text": content,
                }]
            })
        })
        .collect();

    let mut preview_sections = Vec::new();
    preview_sections.push("## System / instructions".to_string());
    if active_system_blocks.is_empty() {
        preview_sections.push("_No active system blocks._".to_string());
    } else {
        for (title, content) in &active_system_blocks {
            preview_sections.push(format!("### {title}\n\n{content}"));
        }
    }

    preview_sections.push("## Developer messages".to_string());
    if developer_blocks.is_empty() {
        preview_sections.push("_No active developer blocks._".to_string());
    } else {
        for (index, (title, content)) in developer_blocks.iter().enumerate() {
            preview_sections.push(format!("### {}. {}\n\n{}", index + 1, title, content));
        }
    }

    CompiledPromptAssembly {
        instructions_text,
        developer_items,
        preview_markdown: preview_sections.join("\n\n"),
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn normalize_block(mut block: PromptBlock) -> PromptBlock {
    block.id = sanitize_block_id(&block.id);
    if block.id.is_empty() {
        block.id = format!("prompt-block-{}", rand_id());
    }

    block.title = block.title.trim().to_string();
    if block.title.is_empty() {
        block.title = match block.role {
            PromptBlockRole::System => format!("System block {}", rand_id()),
            PromptBlockRole::Developer => format!("Developer block {}", rand_id()),
        };
    }

    block.content = block.content.trim().to_string();
    if block.source == PromptBlockSource::User {
        block.source_id = None;
    } else {
        block.source_id = block
            .source_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
    }

    block
}

fn sanitize_block_id(raw: &str) -> String {
    raw.trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn rand_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    format!("{:08x}", nanos)
}
