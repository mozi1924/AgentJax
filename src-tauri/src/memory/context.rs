//! Context injector — builds memory-augmented system messages for the LLM.
//!
//! At the start of each conversation turn, the memory index is loaded and
//! injected as a developer message so the main agent knows what memories exist.

use crate::config::MemoryConfig;
use crate::error::AgentJaxResult;
use crate::memory::index::MemoryIndex;
use crate::memory::store::MemoryStore;
use serde_json::{Value, json};

/// Build a memory context developer item for injection into the conversation.
///
/// If the memory system is enabled and `auto_inject` is true, this returns
/// a developer message containing the MEMORY.md index, allowing the model
/// to know what memories exist and use `memory_search` / `memory_recall`
/// to retrieve full contents.
pub fn build_memory_context(
    store: &MemoryStore,
    config: &MemoryConfig,
) -> AgentJaxResult<Option<Value>> {
    if !config.enabled || !config.auto_inject {
        return Ok(None);
    }

    let index_content = MemoryIndex::rebuild(store)?;

    // Check if there are any actual memory entries (look for "## " headings).
    if !index_content.contains("\n## ") {
        // Only the header — no actual memories stored.
        return Ok(None);
    }

    // Truncate to max_index_tokens (rough heuristic: 1 token ≈ 4 chars).
    let max_chars = (config.max_index_tokens as usize).saturating_mul(4);
    let truncated = if index_content.len() > max_chars {
        let truncated: String = index_content.chars().take(max_chars).collect();
        format!("{truncated}\n\n(Index truncated — use memory_search to find more)")
    } else {
        index_content
    };

    Ok(Some(json!({
        "role": "developer",
        "content": [{
            "type": "input_text",
            "text": format!(
                "## Agent Memory Index\n\n{}\n\n---\n\
                 Use memory_search(query) to search across all memories.\n\
                 Use memory_recall(name) to retrieve the full content of a specific memory.\n\
                 Memories are automatically written by the background memory sub-agent.",
                truncated
            )
        }]
    })))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::{MemoryFrontmatter, MemoryType, ParsedMemory};

    #[test]
    fn test_build_context_with_memories() {
        let dir = std::env::temp_dir().join(format!("memory-ctx-{}", uuid::Uuid::new_v4()));
        let store = MemoryStore::open(dir.clone()).expect("open");

        let memory = ParsedMemory {
            frontmatter: MemoryFrontmatter {
                name: "test-memory".to_string(),
                description: "A test memory".to_string(),
                memory_type: MemoryType::Project,
                tags: vec!["test".to_string()],
                links: vec![],
            },
            body: "Test body content.".to_string(),
        };
        store.write_memory(&memory).expect("write");

        let config = MemoryConfig::default();
        let result = build_memory_context(&store, &config).expect("build");
        assert!(result.is_some());
        let item = result.unwrap();
        let text = item["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Agent Memory Index"));
        assert!(text.contains("test-memory"));
        assert!(text.contains("memory_search"));
        assert!(text.contains("memory_recall"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_build_context_empty() {
        let dir = std::env::temp_dir().join(format!("memory-ctx-empty-{}", uuid::Uuid::new_v4()));
        let store = MemoryStore::open(dir.clone()).expect("open");
        let config = MemoryConfig::default();
        let result = build_memory_context(&store, &config).expect("build");
        // No memories → no context injected.
        assert!(result.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_build_context_disabled() {
        let dir = std::env::temp_dir().join(format!("memory-ctx-dis-{}", uuid::Uuid::new_v4()));
        let store = MemoryStore::open(dir.clone()).expect("open");
        let config = MemoryConfig {
            enabled: false,
            ..MemoryConfig::default()
        };
        let result = build_memory_context(&store, &config).expect("build");
        assert!(result.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
