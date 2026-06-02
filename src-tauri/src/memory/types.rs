//! Memory type definitions — data structures for the async memory system.

use serde::{Deserialize, Serialize};

// ── Memory Type ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryType {
    /// User profile / preferences.
    User,
    /// User feedback and corrections.
    Feedback,
    /// Project-specific knowledge.
    Project,
    /// External reference (URL, doc).
    Reference,
}

impl MemoryType {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryType::User => "user",
            MemoryType::Feedback => "feedback",
            MemoryType::Project => "project",
            MemoryType::Reference => "reference",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "user" => Some(MemoryType::User),
            "feedback" => Some(MemoryType::Feedback),
            "project" => Some(MemoryType::Project),
            "reference" => Some(MemoryType::Reference),
            _ => None,
        }
    }
}

// ── Memory Frontmatter ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFrontmatter {
    /// Short kebab-case slug used as the filename and link target.
    pub name: String,
    /// One-line summary used to decide relevance during recall.
    pub description: String,
    /// The memory type.
    #[serde(rename = "type")]
    pub memory_type: MemoryType,
    /// Optional tags for categorization.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Optional list of `[[wikilinks]]` to other memories.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
}

// ── Parsed Memory ─────────────────────────────────────────────────────────────

/// A fully parsed memory entry: frontmatter + markdown body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedMemory {
    pub frontmatter: MemoryFrontmatter,
    pub body: String,
}

// ── Index Entry ───────────────────────────────────────────────────────────────

/// A lightweight entry in the MEMORY.md index.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryIndexEntry {
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub memory_type: String,
    pub file_name: String,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_type_roundtrip() {
        for variant in &[
            MemoryType::User,
            MemoryType::Feedback,
            MemoryType::Project,
            MemoryType::Reference,
        ] {
            let s = variant.as_str();
            let parsed = MemoryType::from_str(s);
            assert_eq!(parsed, Some(variant.clone()));
        }
    }

    #[test]
    fn test_memory_type_from_str_unknown() {
        assert_eq!(MemoryType::from_str("unknown"), None);
    }

    #[test]
    fn test_frontmatter_serialization() {
        let fm = MemoryFrontmatter {
            name: "test-memory".to_string(),
            description: "A test memory".to_string(),
            memory_type: MemoryType::Project,
            tags: vec!["rust".to_string(), "test".to_string()],
            links: vec!["other-memory".to_string()],
        };
        let json = serde_json::to_string(&fm).unwrap();
        assert!(json.contains("test-memory"));
        assert!(json.contains("project"));
        let parsed: MemoryFrontmatter = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test-memory");
        assert_eq!(parsed.tags.len(), 2);
    }
}
