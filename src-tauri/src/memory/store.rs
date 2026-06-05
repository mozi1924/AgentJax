//! MemoryStore — file-based CRUD for memory files in `~/.agentjax/memory/`.
//!
//! Each memory is stored as a Markdown file with YAML frontmatter.
//! The store handles reading, writing, deleting, and listing memories.

use crate::error::{AgentJaxError, AgentJaxResult};
use crate::memory::types::{MemoryFrontmatter, MemoryIndexEntry, ParsedMemory};
use std::path::PathBuf;

// ── MemoryStore ───────────────────────────────────────────────────────────────

pub struct MemoryStore {
    base_dir: PathBuf,
}

impl MemoryStore {
    /// Open (or create) the memory store at the default location.
    pub fn open(base_dir: PathBuf) -> AgentJaxResult<Self> {
        std::fs::create_dir_all(&base_dir).map_err(|e| {
            AgentJaxError::memory(format!(
                "Failed to create memory directory {}: {e}",
                base_dir.display()
            ))
        })?;
        Ok(Self { base_dir })
    }

    /// Write a memory to a Markdown file.
    pub fn write_memory(&self, memory: &ParsedMemory) -> AgentJaxResult<()> {
        let file_path = self.memory_path(&memory.frontmatter.name);
        let content = self.serialize_memory(memory)?;
        std::fs::write(&file_path, &content).map_err(|e| {
            AgentJaxError::memory(format!(
                "Failed to write memory '{}' to {}: {e}",
                memory.frontmatter.name,
                file_path.display()
            ))
        })?;
        Ok(())
    }

    /// Read a memory by name (without `.md` extension).
    pub fn read_memory(&self, name: &str) -> AgentJaxResult<ParsedMemory> {
        let file_path = self.memory_path(name);
        let content = std::fs::read_to_string(&file_path).map_err(|e| {
            AgentJaxError::memory(format!(
                "Failed to read memory '{}' from {}: {e}",
                name,
                file_path.display()
            ))
        })?;
        self.parse_memory(&content, name)
    }

    /// Delete a memory by name.
    pub fn delete_memory(&self, name: &str) -> AgentJaxResult<bool> {
        let file_path = self.memory_path(name);
        if !file_path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(&file_path).map_err(|e| {
            AgentJaxError::memory(format!("Failed to delete memory '{}': {e}", name))
        })?;
        Ok(true)
    }

    /// List all memory index entries.
    pub fn list_memories(&self) -> AgentJaxResult<Vec<MemoryIndexEntry>> {
        let mut entries = Vec::new();
        let dir = std::fs::read_dir(&self.base_dir).map_err(|e| {
            AgentJaxError::memory(format!(
                "Failed to read memory directory {}: {e}",
                self.base_dir.display()
            ))
        })?;

        for entry in dir {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "md") {
                continue;
            }
            // Skip MEMORY.md (the index file itself).
            if path.file_stem().is_some_and(|s| s == "MEMORY") {
                continue;
            }

            let file_name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();

            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    if let Ok(memory) = self.parse_memory(&content, &file_name) {
                        entries.push(MemoryIndexEntry {
                            name: memory.frontmatter.name,
                            description: memory.frontmatter.description,
                            tags: memory.frontmatter.tags,
                            memory_type: memory.frontmatter.memory_type.as_str().to_string(),
                            file_name: format!("{file_name}.md"),
                        });
                    }
                }
                Err(_) => continue,
            }
        }

        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    /// Check if a memory exists.
    pub fn memory_exists(&self, name: &str) -> bool {
        self.memory_path(name).exists()
    }

    // ── Private helpers ──────────────────────────────────────────────────

    fn memory_path(&self, name: &str) -> PathBuf {
        self.base_dir.join(format!("{name}.md"))
    }

    fn serialize_memory(&self, memory: &ParsedMemory) -> AgentJaxResult<String> {
        let frontmatter_yaml = serde_yaml::to_string(&memory.frontmatter)
            .map_err(|e| AgentJaxError::memory(format!("Failed to serialize frontmatter: {e}")))?;
        Ok(format!(
            "---\n{}\n---\n\n{}\n",
            frontmatter_yaml.trim(),
            memory.body.trim()
        ))
    }

    fn parse_memory(&self, content: &str, fallback_name: &str) -> AgentJaxResult<ParsedMemory> {
        // Parse YAML frontmatter between --- delimiters.
        let mut parts = content.splitn(3, "---");
        let _before = parts.next(); // Empty or whitespace before first ---
        let frontmatter_str = parts.next().unwrap_or("");
        let body = parts.next().unwrap_or("").trim().to_string();

        let frontmatter: MemoryFrontmatter =
            serde_yaml::from_str(frontmatter_str).unwrap_or_else(|_| MemoryFrontmatter {
                name: fallback_name.to_string(),
                description: String::new(),
                memory_type: crate::memory::types::MemoryType::Reference,
                tags: Vec::new(),
                links: Vec::new(),
            });

        Ok(ParsedMemory { frontmatter, body })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::MemoryType;

    fn make_test_store() -> (PathBuf, MemoryStore) {
        let dir = std::env::temp_dir().join(format!("memory-test-{}", uuid::Uuid::new_v4()));
        let store = MemoryStore::open(dir.clone()).expect("open store");
        (dir, store)
    }

    #[test]
    fn test_write_and_read_memory() {
        let (dir, store) = make_test_store();
        let memory = ParsedMemory {
            frontmatter: MemoryFrontmatter {
                name: "test-memory".to_string(),
                description: "A test memory entry".to_string(),
                memory_type: MemoryType::Project,
                tags: vec!["test".to_string()],
                links: vec![],
            },
            body: "This is the body content.\n\nWith **markdown**.".to_string(),
        };

        store.write_memory(&memory).expect("write");
        let read = store.read_memory("test-memory").expect("read");
        assert_eq!(read.frontmatter.name, "test-memory");
        assert_eq!(read.frontmatter.description, "A test memory entry");
        assert!(read.body.contains("**markdown**"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_list_memories() {
        let (dir, store) = make_test_store();

        for i in 0..3 {
            let memory = ParsedMemory {
                frontmatter: MemoryFrontmatter {
                    name: format!("memory-{i}"),
                    description: format!("Memory {i}"),
                    memory_type: MemoryType::Project,
                    tags: vec![],
                    links: vec![],
                },
                body: format!("Body {i}"),
            };
            store.write_memory(&memory).expect("write");
        }

        let list = store.list_memories().expect("list");
        assert_eq!(list.len(), 3);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_delete_memory() {
        let (dir, store) = make_test_store();
        let memory = ParsedMemory {
            frontmatter: MemoryFrontmatter {
                name: "to-delete".to_string(),
                description: "Will be deleted".to_string(),
                memory_type: MemoryType::Reference,
                tags: vec![],
                links: vec![],
            },
            body: "Delete me.".to_string(),
        };

        store.write_memory(&memory).expect("write");
        assert!(store.memory_exists("to-delete"));

        let deleted = store.delete_memory("to-delete").expect("delete");
        assert!(deleted);
        assert!(!store.memory_exists("to-delete"));

        // Delete again should return false.
        let deleted_again = store.delete_memory("to-delete").expect("delete");
        assert!(!deleted_again);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_memory_not_found() {
        let (dir, store) = make_test_store();
        let result = store.read_memory("nonexistent");
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
