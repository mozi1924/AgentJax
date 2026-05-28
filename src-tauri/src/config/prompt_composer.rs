use crate::config::constants::{
    BUILTIN_CORE_SYSTEM_BLOCK_CONTENT, BUILTIN_CORE_SYSTEM_BLOCK_ID, BUILTIN_CORE_SYSTEM_SOURCE_ID,
    BUILTIN_CORE_SYSTEM_TITLE,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PromptBlockRole {
    #[default]
    System,
    Developer,
}

impl PromptBlockRole {
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
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Builtin => "builtin",
            Self::Plugin => "plugin",
        }
    }
}

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PromptComposerConfig {
    pub blocks: Vec<PromptBlock>,
}

#[derive(Debug, Clone)]
pub struct CompiledPromptAssembly {
    pub instructions_text: String,
    pub developer_items: Vec<Value>,
    pub preview_markdown: String,
}

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

pub fn normalize_prompt_composer(mut composer: PromptComposerConfig) -> PromptComposerConfig {
    if composer.blocks.is_empty() {
        composer.blocks = default_builtin_blocks();
    }

    let defaults = default_builtin_blocks();
    let mut normalized = composer
        .blocks
        .into_iter()
        .enumerate()
        .map(|(index, block)| normalize_block(block, index))
        .collect::<Vec<_>>();

    for default_block in defaults {
        match normalized
            .iter_mut()
            .find(|block| block.id == default_block.id)
        {
            Some(existing) => {
                existing.title = default_block.title.clone();
                existing.role = default_block.role;
                existing.content = default_block.content.clone();
                existing.source = default_block.source;
                existing.source_id = default_block.source_id.clone();
                existing.locked = default_block.locked;
            }
            None => normalized.push(default_block),
        }
    }

    let mut system_blocks = Vec::new();
    let mut developer_blocks = Vec::new();
    for block in normalized {
        if block.role == PromptBlockRole::System {
            system_blocks.push(block);
        } else {
            developer_blocks.push(block);
        }
    }

    system_blocks.extend(developer_blocks);
    PromptComposerConfig {
        blocks: system_blocks,
    }
}

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

fn normalize_block(mut block: PromptBlock, index: usize) -> PromptBlock {
    block.id = sanitize_block_id(&block.id);
    if block.id.is_empty() {
        block.id = format!("prompt-block-{}", index + 1);
    }

    block.title = block.title.trim().to_string();
    if block.title.is_empty() {
        block.title = match block.role {
            PromptBlockRole::System => format!("System block {}", index + 1),
            PromptBlockRole::Developer => format!("Developer block {}", index + 1),
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
