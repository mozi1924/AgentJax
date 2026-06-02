//! Full-text search over memory files.

use crate::error::AgentJaxResult;
use crate::memory::store::MemoryStore;
use crate::memory::types::ParsedMemory;
use serde::Serialize;

// ── Search Result ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySearchResult {
    pub name: String,
    pub description: String,
    pub memory_type: String,
    pub snippet: String,
    pub score: u32,
}

// ── Search ────────────────────────────────────────────────────────────────────

/// Search across all memories for a query string.
///
/// Matches against name (highest weight), description, tags, and body content.
/// Results are ranked by relevance score.
pub fn search_memories(
    store: &MemoryStore,
    query: &str,
    max_results: usize,
) -> AgentJaxResult<Vec<MemorySearchResult>> {
    let entries = store.list_memories()?;
    let query_lower = query.to_lowercase();
    let mut results: Vec<MemorySearchResult> = Vec::new();

    for entry in &entries {
        let mut score: u32 = 0;

        // Exact name match = highest score.
        if entry.name.to_lowercase() == query_lower {
            score += 100;
        } else if entry.name.to_lowercase().contains(&query_lower) {
            score += 50;
        }

        // Description match.
        if entry.description.to_lowercase().contains(&query_lower) {
            score += 30;
        }

        // Tag match.
        for tag in &entry.tags {
            if tag.to_lowercase().contains(&query_lower) {
                score += 20;
            }
        }

        // Body match — load the full memory for content search.
        let snippet = if score == 0 {
            // Only load body if no other matches yet (optimization).
            if let Ok(memory) = load_memory_for_search(store, &entry.name) {
                let body_lower = memory.body.to_lowercase();
                if body_lower.contains(&query_lower) {
                    score += 10;
                    extract_snippet(&memory.body, query)
                } else {
                    continue; // No match at all, skip this result.
                }
            } else {
                continue;
            }
        } else if let Ok(memory) = load_memory_for_search(store, &entry.name) {
            extract_snippet(&memory.body, query)
        } else {
            String::new()
        };

        if score > 0 {
            results.push(MemorySearchResult {
                name: entry.name.clone(),
                description: entry.description.clone(),
                memory_type: entry.memory_type.clone(),
                snippet,
                score,
            });
        }
    }

    // Sort by score descending.
    results.sort_by(|a, b| b.score.cmp(&a.score));
    results.truncate(max_results.max(1));

    Ok(results)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn load_memory_for_search(store: &MemoryStore, name: &str) -> AgentJaxResult<ParsedMemory> {
    store.read_memory(name)
}

fn extract_snippet(body: &str, query: &str) -> String {
    let query_lower = query.to_lowercase();
    let body_lower = body.to_lowercase();

    if let Some(pos) = body_lower.find(&query_lower) {
        let start = pos.saturating_sub(40);
        let end = (pos + query.len() + 40).min(body.len());
        let snippet = &body[start..end];
        let prefix = if start > 0 { "…" } else { "" };
        let suffix = if end < body.len() { "…" } else { "" };
        format!("{prefix}{snippet}{suffix}")
    } else {
        body.chars().take(100).collect()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::{MemoryFrontmatter, MemoryType, ParsedMemory};
    use std::path::PathBuf;

    fn make_search_store() -> (PathBuf, MemoryStore) {
        let dir = std::env::temp_dir().join(format!("memory-search-{}", uuid::Uuid::new_v4()));
        let store = MemoryStore::open(dir.clone()).expect("open");

        let memories = vec![
            ("project-architecture", "Overview of AgentJax architecture", vec!["architecture", "rust"], "The project uses Tauri v2 with React frontend and Rust backend."),
            ("sub-agent-design", "Design of the sub-agent module", vec!["sub-agent", "async"], "Sub-agents run asynchronously via tokio::spawn."),
            ("getting-started", "Getting started guide", vec!["guide"], "To start developing, run pnpm install and pnpm dev."),
        ];

        for (name, desc, tags, body) in &memories {
            let memory = ParsedMemory {
                frontmatter: MemoryFrontmatter {
                    name: name.to_string(),
                    description: desc.to_string(),
                    memory_type: MemoryType::Project,
                    tags: tags.iter().map(|t| t.to_string()).collect(),
                    links: vec![],
                },
                body: body.to_string(),
            };
            store.write_memory(&memory).expect("write");
        }

        (dir, store)
    }

    #[test]
    fn test_search_by_name_exact() {
        let (dir, store) = make_search_store();
        let results = search_memories(&store, "sub-agent-design", 10).expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "sub-agent-design");
        assert!(results[0].score >= 100);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_search_by_description() {
        let (dir, store) = make_search_store();
        let results = search_memories(&store, "architecture", 10).expect("search");
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "project-architecture");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_search_by_tag() {
        let (dir, store) = make_search_store();
        let results = search_memories(&store, "async", 10).expect("search");
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "sub-agent-design");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_search_by_body() {
        let (dir, store) = make_search_store();
        let results = search_memories(&store, "tokio::spawn", 10).expect("search");
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "sub-agent-design");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_search_no_results() {
        let (dir, store) = make_search_store();
        let results = search_memories(&store, "nonexistent-query-xyz", 10).expect("search");
        assert!(results.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_extract_snippet() {
        let body = "This is a long text that contains the target word somewhere in the middle of the content.";
        let snippet = extract_snippet(body, "target");
        assert!(snippet.contains("target"));
        assert!(snippet.starts_with('…') || snippet.contains("long text"));
    }
}
