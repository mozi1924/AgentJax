//! MEMORY.md index generation and maintenance.

use crate::error::AgentJaxResult;
use crate::memory::store::MemoryStore;

/// Generates and maintains the MEMORY.md index file.
pub struct MemoryIndex;

impl MemoryIndex {
    /// Rebuild the MEMORY.md index from all stored memories.
    pub fn rebuild(store: &MemoryStore) -> AgentJaxResult<String> {
        let entries = store.list_memories()?;

        let mut index = String::from("# AgentJax Memory Index\n\n");
        index.push_str(&format!("Total memories: {}\n\n", entries.len()));

        for entry in &entries {
            index.push_str(&format!("## {}\n", entry.name));
            index.push_str(&format!("- **Type**: {}\n", entry.memory_type));
            if !entry.tags.is_empty() {
                index.push_str(&format!(
                    "- **Tags**: {}\n",
                    entry.tags
                        .iter()
                        .map(|t| format!("`{t}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            index.push_str(&format!("- **File**: {}\n", entry.file_name));
            index.push_str(&format!("- **Description**: {}\n", entry.description));
            index.push_str("\n");
        }

        Ok(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::{MemoryFrontmatter, MemoryType, ParsedMemory};

    #[test]
    fn test_rebuild_index() {
        let dir = std::env::temp_dir().join(format!("memory-idx-{}", uuid::Uuid::new_v4()));
        let store = MemoryStore::open(dir.clone()).expect("open");

        let memory = ParsedMemory {
            frontmatter: MemoryFrontmatter {
                name: "test-memory".to_string(),
                description: "A test memory".to_string(),
                memory_type: MemoryType::Project,
                tags: vec!["rust".to_string(), "test".to_string()],
                links: vec![],
            },
            body: "Test body.".to_string(),
        };
        store.write_memory(&memory).expect("write");

        let index = MemoryIndex::rebuild(&store).expect("rebuild");
        assert!(index.contains("AgentJax Memory Index"));
        assert!(index.contains("test-memory"));
        assert!(index.contains("A test memory"));
        assert!(index.contains("rust"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rebuild_empty_index() {
        let dir = std::env::temp_dir().join(format!("memory-idx-empty-{}", uuid::Uuid::new_v4()));
        let store = MemoryStore::open(dir.clone()).expect("open");

        let index = MemoryIndex::rebuild(&store).expect("rebuild");
        assert!(index.contains("Total memories: 0"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
