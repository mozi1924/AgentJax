use crate::error::{AgentJaxError, AgentJaxResult};
use crate::tools::{Tool, ToolExecutionContext};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

use super::common::{is_hidden_name, relative_path_display, resolve_workspace_path, stat_value};

const DEFAULT_LIST_MAX_ENTRIES: usize = 200;
const MAX_LIST_MAX_ENTRIES: usize = 1_000;
const LIST_OUTPUT_CHAR_BUDGET: usize = 48 * 1024;

#[derive(Debug, Deserialize)]
pub struct ListFilesArgs {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub recursive: bool,
    #[serde(default)]
    pub include_hidden: bool,
    #[serde(default)]
    pub max_entries: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct ListCollectionState {
    pub output_chars: usize,
    pub hit_entry_limit: bool,
    pub hit_output_limit: bool,
}

impl ListCollectionState {
    pub fn is_truncated(&self) -> bool {
        self.hit_entry_limit || self.hit_output_limit
    }

    pub fn truncation_reasons(&self) -> Vec<&'static str> {
        let mut reasons = Vec::new();
        if self.hit_entry_limit {
            reasons.push("max_entries");
        }
        if self.hit_output_limit {
            reasons.push("max_output_chars");
        }
        reasons
    }
}

pub struct ListFilesTool;

#[async_trait::async_trait]
impl Tool for ListFilesTool {
    fn name(&self) -> &'static str {
        "list_files"
    }

    fn display_name(&self) -> &'static str {
        "List Files"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("FolderOpen")
    }

    fn description(&self) -> &'static str {
        "Lists files and directories inside the conversation workspace. Supports nested paths, optional recursion, and truncates oversized directory results."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional workspace-relative directory path to list. Defaults to the workspace root."
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Whether to recursively include nested directory contents."
                },
                "include_hidden": {
                    "type": "boolean",
                    "description": "Whether to include hidden entries whose names start with '.'."
                },
                "max_entries": {
                    "type": "integer",
                    "description": "Maximum number of entries to return. Defaults to 200 and is capped at 1000."
                }
            }
        })
    }

    async fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> AgentJaxResult<Value> {
        let args = super::common::parse_tool_args::<ListFilesArgs>(arguments, self.name())?;
        let target = args.path.unwrap_or_else(|| ".".to_string());
        let resolved = resolve_workspace_path(&target, context, true)?;
        if !resolved.absolute_path.exists() {
            return Err(AgentJaxError::not_found(format!(
                "Directory '{}' not found in current conversation workspace",
                relative_path_display(&resolved.relative_path)
            )));
        }

        let metadata = fs::metadata(&resolved.absolute_path)
            .map_err(|err| format!("Failed to stat {}: {err}", resolved.absolute_path.display()))?;
        if !metadata.is_dir() {
            return Err(AgentJaxError::tool(format!(
                "Path '{}' is not a directory",
                relative_path_display(&resolved.relative_path)
            )));
        }

        let max_entries = args
            .max_entries
            .unwrap_or(DEFAULT_LIST_MAX_ENTRIES)
            .clamp(1, MAX_LIST_MAX_ENTRIES);
        let mut entries = Vec::new();
        let mut state = ListCollectionState::default();
        collect_directory_entries(
            &resolved.workspace_dir,
            &resolved.absolute_path,
            args.recursive,
            args.include_hidden,
            max_entries,
            &mut entries,
            &mut state,
        )?;

        Ok(json!({
            "path": relative_path_display(&resolved.relative_path),
            "recursive": args.recursive,
            "includeHidden": args.include_hidden,
            "maxEntries": max_entries,
            "truncated": state.is_truncated(),
            "truncationReasons": state.truncation_reasons(),
            "entryCount": entries.len(),
            "approxOutputChars": state.output_chars,
            "entries": entries,
        }))
    }
}

pub fn collect_directory_entries(
    workspace_dir: &Path,
    current_dir: &Path,
    recursive: bool,
    include_hidden: bool,
    max_entries: usize,
    entries: &mut Vec<Value>,
    state: &mut ListCollectionState,
) -> AgentJaxResult<()> {
    let mut children = Vec::new();
    for entry in fs::read_dir(current_dir)
        .map_err(|err| AgentJaxError::tool(format!("Failed to list directory {}: {err}", current_dir.display())).with_error_source(&err))?
    {
        let entry = entry.map_err(|err| {
            AgentJaxError::tool(format!(
                "Failed to inspect directory entry {}: {err}",
                current_dir.display()
            ))
            .with_error_source(&err)
        })?;
        children.push(entry);
    }

    children.sort_by_key(|entry| entry.path());

    for entry in children {
        if entries.len() >= max_entries {
            state.hit_entry_limit = true;
            break;
        }

        let path = entry.path();
        let relative = path
            .strip_prefix(workspace_dir)
            .map_err(|err| AgentJaxError::tool(format!("Failed to derive workspace-relative path: {err}")).with_error_source(&err))?
            .to_path_buf();

        let name = entry.file_name();
        let name = name.to_string_lossy().to_string();
        if !include_hidden && is_hidden_name(&name) {
            continue;
        }

        let metadata = entry
            .metadata()
            .map_err(|err| AgentJaxError::tool(format!("Failed to read metadata for {}: {err}", path.display())).with_error_source(&err))?;
        let value = stat_value(&relative, &metadata);
        let estimated_chars = value.to_string().chars().count();
        if !entries.is_empty() && state.output_chars + estimated_chars > LIST_OUTPUT_CHAR_BUDGET {
            state.hit_output_limit = true;
            break;
        }

        state.output_chars += estimated_chars;
        entries.push(value);

        if recursive && metadata.is_dir() {
            collect_directory_entries(
                workspace_dir,
                &path,
                true,
                include_hidden,
                max_entries,
                entries,
                state,
            )?;
            if state.is_truncated() {
                break;
            }
        }
    }

    Ok(())
}
