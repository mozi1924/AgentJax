//! Large file handling for the LCM system.
//!
//! When tool results include file contents that exceed the
//! `large_file_token_threshold`, LCM stores them externally as
//! `FileReference`s with exploration summaries rather than
//! loading the raw content into the active context.
//!
//! ## Exploration Strategies
//!
//! | File Type | Strategy |
//! |-----------|----------|
//! | JSON | Schema + shape extraction |
//! | CSV | Column names, row count, sample rows |
//! | Code (.rs, .ts, .py, etc.) | Function signatures, struct/class hierarchy |
//! | Text | LLM-generated summary |
//! | Other | Basic metadata only |

use crate::lcm::types::{FileRefId, FileReference, LcmConfig, LcmError, estimate_tokens};
use std::path::Path;

// ── File Handler ────────────────────────────────────────────────────────────

/// Manages large file detection and exploration summary generation.
/// Currently only used in tests; will be wired into the LCM engine
/// when large-file handling is activated.
#[allow(dead_code)]
pub struct FileHandler {
    /// Token threshold above which files are stored as references.
    large_file_threshold: u32,
    /// Type-specific explorers, keyed by MIME type or extension.
    explorers: Vec<Box<dyn FileExplorer>>,
}

#[allow(dead_code)]
impl FileHandler {
    /// Create a new FileHandler from LCM configuration.
    pub fn new(config: &LcmConfig) -> Self {
        Self {
            large_file_threshold: config.large_file_token_threshold,
            explorers: vec![
                Box::new(JsonExplorer),
                Box::new(CsvExplorer),
                Box::new(CodeExplorer),
                Box::new(TextExplorer),
            ],
        }
    }

    /// Check if a file's content exceeds the large file threshold.
    pub fn is_large_file(&self, content: &str) -> bool {
        estimate_tokens(content) > self.large_file_threshold
    }

    /// Generate an exploration summary for a file.
    ///
    /// Returns `None` if the file is small enough to be loaded directly
    /// into the active context.
    pub fn explore_file(
        &self,
        path: &Path,
        content: &str,
        mime_type: &str,
    ) -> Result<Option<String>, LcmError> {
        if !self.is_large_file(content) {
            return Ok(None);
        }

        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        // Try each explorer; the first one that supports this file type wins.
        for explorer in &self.explorers {
            if explorer.supports(extension, mime_type) {
                return Ok(Some(explorer.explore(content)?));
            }
        }

        // Fallback: basic metadata.
        Ok(Some(self.basic_exploration(path, content)))
    }

    /// Register a large file as a FileReference.
    pub fn register_file(
        &self,
        path: &Path,
        content: &str,
        mime_type: &str,
        conversation_id: &str,
        timestamp_unix_ms: i64,
    ) -> Result<Option<FileReference>, LcmError> {
        let exploration = match self.explore_file(path, content, mime_type)? {
            Some(summary) => summary,
            None => return Ok(None), // File is below threshold — no reference needed.
        };

        Ok(Some(FileReference {
            id: FileRefId::new(),
            conversation_id: conversation_id.to_string(),
            path: path.to_string_lossy().to_string(),
            mime_type: mime_type.to_string(),
            token_count: estimate_tokens(content),
            exploration_summary: exploration,
            registered_at_unix_ms: timestamp_unix_ms,
        }))
    }

    /// Basic exploration for unknown file types.
    fn basic_exploration(&self, path: &Path, content: &str) -> String {
        let line_count = content.lines().count();
        let char_count = content.chars().count();
        let token_estimate = estimate_tokens(content);
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        format!(
            "File: {file_name}\n\
             Lines: {line_count}\n\
             Characters: {char_count}\n\
             Estimated tokens: {token_estimate}\n\
             (Use file reading tools to inspect specific sections)"
        )
    }
}

// ── File Explorer Trait ─────────────────────────────────────────────────────

/// A type-aware file explorer that generates exploration summaries.
/// Currently only used in tests via the FileHandler.
#[allow(dead_code)]
pub trait FileExplorer: Send + Sync {
    /// Whether this explorer supports the given file extension and MIME type.
    fn supports(&self, extension: &str, mime_type: &str) -> bool;

    /// Generate an exploration summary for the file content.
    fn explore(&self, content: &str) -> Result<String, LcmError>;
}

// ── JSON Explorer ───────────────────────────────────────────────────────────

struct JsonExplorer;

impl FileExplorer for JsonExplorer {
    fn supports(&self, extension: &str, _mime_type: &str) -> bool {
        matches!(extension, "json" | "jsonl")
    }

    fn explore(&self, content: &str) -> Result<String, LcmError> {
        let mut summary = String::from("[JSON File]\n");

        // Try to parse as a single JSON value.
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(content) {
            summary.push_str(&format!("Type: {}\n", describe_json_type(&value)));

            if let Some(obj) = value.as_object() {
                summary.push_str(&format!("Keys ({})\n", obj.len()));
                for (key, val) in obj.iter().take(15) {
                    summary.push_str(&format!(
                        "  {}: {}\n",
                        key,
                        describe_json_type(val)
                    ));
                }
                if obj.len() > 15 {
                    summary.push_str(&format!(
                        "  ... and {} more keys\n",
                        obj.len() - 15
                    ));
                }
            }

            if let Some(arr) = value.as_array() {
                summary.push_str(&format!("Items: {}\n", arr.len()));
                if let Some(first) = arr.first() {
                    summary.push_str(&format!(
                        "First item type: {}\n",
                        describe_json_type(first)
                    ));
                }
            }
        } else {
            // Try JSONL (one JSON object per line).
            let lines: Vec<&str> = content.lines().collect();
            let parsed_count = lines
                .iter()
                .filter(|line| serde_json::from_str::<serde_json::Value>(line).is_ok())
                .count();

            summary.push_str(&format!(
                "Format: JSON Lines ({} total lines, {} valid JSON)\n",
                lines.len(),
                parsed_count
            ));

            if let Some(first_valid) = lines
                .iter()
                .find(|line| serde_json::from_str::<serde_json::Value>(line).is_ok())
            {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(first_valid) {
                    summary.push_str(&format!(
                        "Sample record shape: {}\n",
                        describe_json_type(&val)
                    ));
                }
            }
        }

        summary.push_str(&format!(
            "Total size: {} tokens estimated\n",
            estimate_tokens(content)
        ));

        Ok(summary)
    }
}

#[allow(dead_code)]
fn describe_json_type(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(_) => "boolean".to_string(),
        serde_json::Value::Number(_) => "number".to_string(),
        serde_json::Value::String(s) => {
            if s.len() > 50 {
                format!("string ({} chars)", s.len())
            } else {
                format!("string \"{}\"", s)
            }
        }
        serde_json::Value::Array(arr) => format!("array[{}]", arr.len()),
        serde_json::Value::Object(obj) => format!("object{{{}}}", obj.len()),
    }
}

// ── CSV Explorer ────────────────────────────────────────────────────────────

struct CsvExplorer;

impl FileExplorer for CsvExplorer {
    fn supports(&self, extension: &str, _mime_type: &str) -> bool {
        matches!(extension, "csv" | "tsv")
    }

    fn explore(&self, content: &str) -> Result<String, LcmError> {
        let mut summary = String::from("[CSV File]\n");
        let lines: Vec<&str> = content.lines().collect();

        if lines.is_empty() {
            summary.push_str("(empty file)\n");
            return Ok(summary);
        }

        // Header row.
        let delimiter = if content.contains('\t') { '\t' } else { ',' };
        let headers: Vec<&str> = lines[0].split(delimiter).map(|s| s.trim()).collect();

        summary.push_str(&format!("Columns ({}): {}\n", headers.len(), headers.join(", ")));
        summary.push_str(&format!("Total rows (including header): {}\n", lines.len()));

        // Sample first data row.
        if lines.len() > 1 {
            let sample: Vec<&str> = lines[1].split(delimiter).map(|s| s.trim()).collect();
            summary.push_str("Sample row: ");
            for (i, val) in sample.iter().enumerate().take(10) {
                let display = if val.len() > 30 {
                    format!("{}...", &val[..27])
                } else {
                    val.to_string()
                };
                summary.push_str(&format!("{}={} ", headers.get(i).unwrap_or(&""), display));
            }
            summary.push('\n');
        }

        Ok(summary)
    }
}

// ── Code Explorer ───────────────────────────────────────────────────────────

struct CodeExplorer;

impl FileExplorer for CodeExplorer {
    fn supports(&self, extension: &str, _mime_type: &str) -> bool {
        matches!(
            extension,
            "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "java" | "c" | "cpp" | "h"
                | "css" | "html" | "sql" | "sh" | "toml" | "yaml" | "yml"
        )
    }

    fn explore(&self, content: &str) -> Result<String, LcmError> {
        let mut summary = String::from("[Code File]\n");

        let lines: Vec<&str> = content.lines().collect();
        summary.push_str(&format!("Lines: {}\n", lines.len()));

        // Extract function signatures and type definitions via simple regex.
        let fn_patterns = [
            ("fn ", "Rust function"),
            ("pub fn ", "Rust public function"),
            ("def ", "Python function"),
            ("function ", "JS/TS function"),
            ("const ", "JS/TS const"),
            ("class ", "Class definition"),
            ("struct ", "Rust struct"),
            ("enum ", "Enum definition"),
            ("interface ", "TS interface"),
            ("type ", "TS type alias"),
            ("export ", "Export"),
            ("impl ", "Rust impl block"),
        ];

        let mut found_definitions: Vec<String> = Vec::new();
        for line in &lines {
            let trimmed = line.trim();
            for (pattern, label) in &fn_patterns {
                if trimmed.starts_with(pattern) && trimmed.len() > pattern.len() {
                    let short = if trimmed.len() > 80 {
                        format!("{}...", &trimmed[..77])
                    } else {
                        trimmed.to_string()
                    };
                    found_definitions.push(format!("  [{label}] {short}"));
                    break;
                }
            }
        }

        if found_definitions.is_empty() {
            summary.push_str("(no top-level definitions detected)\n");
        } else {
            let show_count = found_definitions.len().min(30);
            summary.push_str(&format!(
                "Definitions found: {} (showing first {})\n",
                found_definitions.len(),
                show_count
            ));
            for def in found_definitions.iter().take(show_count) {
                summary.push_str(def);
                summary.push('\n');
            }
            if found_definitions.len() > show_count {
                summary.push_str(&format!(
                    "  ... and {} more\n",
                    found_definitions.len() - show_count
                ));
            }
        }

        Ok(summary)
    }
}

// ── Text Explorer ───────────────────────────────────────────────────────────

struct TextExplorer;

impl FileExplorer for TextExplorer {
    fn supports(&self, _extension: &str, mime_type: &str) -> bool {
        // Catch-all for text/* MIME types not handled by other explorers.
        mime_type.starts_with("text/")
    }

    fn explore(&self, content: &str) -> Result<String, LcmError> {
        let lines: Vec<&str> = content.lines().collect();
        let char_count = content.chars().count();
        let word_count = content.split_whitespace().count();

        let mut summary = format!(
            "[Text File]\n\
             Lines: {}\n\
             Words: ~{}\n\
             Characters: {}\n\
             Estimated tokens: {}\n",
            lines.len(),
            word_count,
            char_count,
            estimate_tokens(content),
        );

        // Show first few non-empty lines as preview.
        let preview_lines: Vec<&str> = lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .take(5)
            .copied()
            .collect();

        if !preview_lines.is_empty() {
            summary.push_str("Preview:\n");
            for line in preview_lines {
                let short = if line.len() > 100 {
                    format!("{}...", &line[..97])
                } else {
                    line.to_string()
                };
                summary.push_str(&format!("  {short}\n"));
            }
        }

        Ok(summary)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lcm::types::LcmConfig;

    #[test]
    fn test_small_file_not_explored() {
        let config = LcmConfig::default();
        let handler = FileHandler::new(&config);

        let content = "Hello, world!";
        let result = handler
            .explore_file(Path::new("test.txt"), content, "text/plain")
            .unwrap();
        assert!(result.is_none()); // Below threshold.
    }

    #[test]
    fn test_large_file_explored() {
        let mut config = LcmConfig::default();
        config.large_file_token_threshold = 5; // Very low threshold for testing.
        let handler = FileHandler::new(&config);

        let content = "This is a long text that exceeds the threshold for large file detection and should be explored.";
        let result = handler
            .explore_file(Path::new("test.txt"), content, "text/plain")
            .unwrap();
        assert!(result.is_some());
        let summary = result.unwrap();
        assert!(summary.contains("[Text File]"));
        assert!(summary.contains("Lines:"));
    }

    #[test]
    fn test_json_exploration() {
        let mut config = LcmConfig::default();
        config.large_file_token_threshold = 5;
        let handler = FileHandler::new(&config);

        let content = r#"{"name": "Alice", "age": 30, "city": "New York", "skills": ["rust", "python"]}"#;
        let result = handler
            .explore_file(Path::new("data.json"), content, "application/json")
            .unwrap();
        assert!(result.is_some());
        let summary = result.unwrap();
        assert!(summary.contains("[JSON File]"));
        assert!(summary.contains("name"));
        assert!(summary.contains("age"));
        assert!(summary.contains("skills"));
    }

    #[test]
    fn test_csv_exploration() {
        let mut config = LcmConfig::default();
        config.large_file_token_threshold = 5;
        let handler = FileHandler::new(&config);

        let content = "name,age,city\nAlice,30,New York\nBob,25,London\n";
        let result = handler
            .explore_file(Path::new("data.csv"), content, "text/csv")
            .unwrap();
        assert!(result.is_some());
        let summary = result.unwrap();
        assert!(summary.contains("[CSV File]"));
        assert!(summary.contains("name, age, city"));
        assert!(summary.contains("Total rows"));
    }

    #[test]
    fn test_code_exploration() {
        let mut config = LcmConfig::default();
        config.large_file_token_threshold = 5;
        let handler = FileHandler::new(&config);

        let content = "fn main() {\n    println!(\"Hello\");\n}\n\npub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\nstruct User {\n    name: String,\n}\n";
        let result = handler
            .explore_file(Path::new("main.rs"), content, "text/plain")
            .unwrap();
        assert!(result.is_some());
        let summary = result.unwrap();
        assert!(summary.contains("[Code File]"));
        assert!(summary.contains("pub fn add"));
        assert!(summary.contains("struct User"));
    }

    #[test]
    fn test_register_file_below_threshold() {
        let config = LcmConfig::default();
        let handler = FileHandler::new(&config);

        let content = "small";
        let result = handler
            .register_file(Path::new("test.txt"), content, "text/plain", "conv-1", 1000)
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_register_file_above_threshold() {
        let mut config = LcmConfig::default();
        config.large_file_token_threshold = 1;
        let handler = FileHandler::new(&config);

        let content = "some content that is above the threshold";
        let result = handler
            .register_file(Path::new("test.txt"), content, "text/plain", "conv-1", 1000)
            .unwrap();
        assert!(result.is_some());
        let file_ref = result.unwrap();
        assert_eq!(file_ref.conversation_id, "conv-1");
        assert_eq!(file_ref.path, "test.txt");
    }
}
