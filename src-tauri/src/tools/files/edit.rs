use crate::tools::{Tool, ToolExecutionContext};
use serde::Deserialize;
use serde_json::{json, Value};

use super::common::{
    count_lines, read_text_file, relative_path_display, resolve_workspace_path, write_text_file,
    MAX_READ_MAX_BYTES,
};

#[derive(Debug, Deserialize)]
pub struct EditFileArgs {
    #[serde(alias = "filename")]
    pub path: String,
    pub edits: Vec<TextPatchEdit>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum TextPatchEdit {
    Replace {
        find: String,
        replace: String,
        #[serde(default)]
        replace_all: bool,
    },
    InsertAfter {
        anchor: String,
        content: String,
        #[serde(default)]
        insert_all: bool,
    },
    InsertBefore {
        anchor: String,
        content: String,
        #[serde(default)]
        insert_all: bool,
    },
}

#[derive(Debug, Clone)]
struct TextEditOutcome {
    content: String,
    occurrences_changed: usize,
}

fn count_occurrences(content: &str, needle: &str, label: &str) -> Result<usize, String> {
    if needle.is_empty() {
        return Err(format!("{label} cannot be empty"));
    }

    Ok(content.match_indices(needle).count())
}

fn replace_exact_text(
    content: &str,
    needle: &str,
    replacement: &str,
    replace_all: bool,
    label: &str,
) -> Result<TextEditOutcome, String> {
    let occurrences = count_occurrences(content, needle, label)?;
    if occurrences == 0 {
        return Err(format!("Could not find the requested {label} in the file"));
    }

    if !replace_all && occurrences > 1 {
        return Err(format!(
            "Found {occurrences} matches for the requested {label}; rerun with replace_all=true to update every occurrence"
        ));
    }

    let updated = if replace_all {
        content.replace(needle, replacement)
    } else {
        content.replacen(needle, replacement, 1)
    };

    Ok(TextEditOutcome {
        content: updated,
        occurrences_changed: if replace_all { occurrences } else { 1 },
    })
}

fn insert_relative_to_anchor(
    content: &str,
    anchor: &str,
    insertion: &str,
    insert_after: bool,
    insert_all: bool,
    label: &str,
) -> Result<TextEditOutcome, String> {
    let occurrences = count_occurrences(content, anchor, "anchor text")?;
    if occurrences == 0 {
        return Err(format!(
            "Could not find the requested anchor text for {label}"
        ));
    }

    if !insert_all && occurrences > 1 {
        return Err(format!(
            "Found {occurrences} anchor matches for {label}; rerun with insert_all=true to apply the insertion at every match"
        ));
    }

    let updated = if insert_all {
        if insert_after {
            content.replace(anchor, &format!("{anchor}{insertion}"))
        } else {
            content.replace(anchor, &format!("{insertion}{anchor}"))
        }
    } else {
        let index = content
            .find(anchor)
            .ok_or_else(|| format!("Could not find the requested anchor text for {label}"))?;
        let split_index = if insert_after {
            index + anchor.len()
        } else {
            index
        };
        let mut next = String::with_capacity(content.len() + insertion.len());
        next.push_str(&content[..split_index]);
        next.push_str(insertion);
        next.push_str(&content[split_index..]);
        next
    };

    Ok(TextEditOutcome {
        content: updated,
        occurrences_changed: if insert_all { occurrences } else { 1 },
    })
}

fn apply_single_text_patch(content: &str, edit: &TextPatchEdit) -> Result<TextEditOutcome, String> {
    match edit {
        TextPatchEdit::Replace {
            find,
            replace,
            replace_all,
        } => replace_exact_text(content, find, replace, *replace_all, "target text"),
        TextPatchEdit::InsertAfter {
            anchor,
            content: insertion,
            insert_all,
        } => insert_relative_to_anchor(
            content,
            anchor,
            insertion,
            true,
            *insert_all,
            "insert_after",
        ),
        TextPatchEdit::InsertBefore {
            anchor,
            content: insertion,
            insert_all,
        } => insert_relative_to_anchor(
            content,
            anchor,
            insertion,
            false,
            *insert_all,
            "insert_before",
        ),
    }
}

fn apply_text_patch_plan(
    content: &str,
    edits: &[TextPatchEdit],
) -> Result<(String, Vec<Value>), String> {
    if edits.is_empty() {
        return Err("Patch must contain at least one edit".to_string());
    }

    let mut next_content = content.to_string();
    let mut details = Vec::with_capacity(edits.len());

    for (index, edit) in edits.iter().enumerate() {
        let outcome = apply_single_text_patch(&next_content, edit)
            .map_err(|err| format!("Patch edit {} failed: {err}", index + 1))?;
        next_content = outcome.content;
        details.push(json!({
            "index": index + 1,
            "op": patch_operation_name(edit),
            "occurrencesChanged": outcome.occurrences_changed
        }));
    }

    Ok((next_content, details))
}

fn patch_operation_name(edit: &TextPatchEdit) -> &'static str {
    match edit {
        TextPatchEdit::Replace { .. } => "replace",
        TextPatchEdit::InsertAfter { .. } => "insert_after",
        TextPatchEdit::InsertBefore { .. } => "insert_before",
    }
}

pub struct EditFileTool;

impl Tool for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn display_name(&self) -> &'static str {
        "Edit File"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("FilePenLine")
    }

    fn description(&self) -> &'static str {
        "Applies a deterministic sequence of structured text edits to an existing workspace text file. Use this for atomic multi-match modifications."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Workspace-relative file path to edit." },
                "filename": { "type": "string", "description": "Legacy alias for 'path'. New callers should send 'path'." },
                "edits": {
                    "type": "array",
                    "description": "Ordered patch edits. Supported op values: replace, insert_after, insert_before.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "op": {
                                "type": "string",
                                "enum": ["replace", "insert_after", "insert_before"]
                            },
                            "find": { "type": "string", "description": "Exact text block/string to replace. Required for op=replace." },
                            "replace": { "type": "string", "description": "Replacement text. Required for op=replace." },
                            "anchor": { "type": "string", "description": "Exact anchor text to insert relative to. Required for insert operations." },
                            "content": { "type": "string", "description": "Text to insert. Required for insert operations." },
                            "replace_all": { "type": "boolean" },
                            "insert_all": { "type": "boolean" }
                        },
                        "required": ["op"]
                    }
                }
            },
            "required": ["path", "edits"]
        })
    }

    fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> Result<Value, String> {
        let args = super::common::parse_tool_args::<EditFileArgs>(arguments, self.name())?;
        let resolved = resolve_workspace_path(&args.path, context, false)?;
        let original = read_text_file(&resolved.absolute_path, MAX_READ_MAX_BYTES, "edit")?;
        let (patched, details) = apply_text_patch_plan(&original.content, &args.edits)?;
        write_text_file(&resolved.absolute_path, &patched)?;

        Ok(json!({
            "path": relative_path_display(&resolved.relative_path),
            "operationsApplied": details.len(),
            "bytesWritten": patched.as_bytes().len(),
            "lineCount": count_lines(&patched),
            "details": details,
            "status": "success"
        }))
    }
}
