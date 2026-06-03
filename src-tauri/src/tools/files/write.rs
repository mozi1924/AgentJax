use crate::error::AgentJaxResult;
use crate::tools::{Tool, ToolExecutionContext};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs;

use super::common::{count_lines, relative_path_display, resolve_workspace_path, write_text_file};

#[derive(Debug, Deserialize)]
pub struct WriteFileArgs {
    #[serde(alias = "filename")]
    pub path: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct MkdirArgs {
    pub path: String,
    #[serde(default = "default_true")]
    pub recursive: bool,
}

fn default_true() -> bool {
    true
}

pub struct FileWriterTool;

#[async_trait::async_trait]
impl Tool for FileWriterTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn display_name(&self) -> &'static str {
        "Write File"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("FilePenLine")
    }

    fn description(&self) -> &'static str {
        "Writes UTF-8 text to a workspace-relative file. Creates missing parent directories, overwrites existing text files, and rejects binary targets."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Workspace-relative file path to write, e.g. 'notes/output.txt'."
                },
                "filename": {
                    "type": "string",
                    "description": "Legacy alias for 'path'. New callers should send 'path'."
                },
                "content": {
                    "type": "string",
                    "description": "Complete UTF-8 file contents to write."
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> AgentJaxResult<Value> {
        let args = super::common::parse_tool_args::<WriteFileArgs>(arguments, self.name())?;
        let resolved = resolve_workspace_path(&args.path, context, false)?;
        write_text_file(&resolved.absolute_path, &args.content)?;

        Ok(json!({
            "path": relative_path_display(&resolved.relative_path),
            "bytesWritten": args.content.len(),
            "lineCount": count_lines(&args.content),
            "status": "success"
        }))
    }
}

pub struct MkdirTool;

#[async_trait::async_trait]
impl Tool for MkdirTool {
    fn name(&self) -> &'static str {
        "mkdir"
    }

    fn display_name(&self) -> &'static str {
        "Make Directory"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("FolderPlus")
    }

    fn description(&self) -> &'static str {
        "Creates a directory inside the conversation workspace. Supports nested paths and recursive creation."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Workspace-relative directory path to create."
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Whether to create all missing parent directories. Defaults to true."
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> AgentJaxResult<Value> {
        let args = super::common::parse_tool_args::<MkdirArgs>(arguments, self.name())?;
        let resolved = resolve_workspace_path(&args.path, context, false)?;
        let existed_before = resolved.absolute_path.exists();

        if args.recursive {
            fs::create_dir_all(&resolved.absolute_path).map_err(|err| {
                format!(
                    "Failed to create directory {}: {err}",
                    resolved.absolute_path.display()
                )
            })?;
        } else {
            fs::create_dir(&resolved.absolute_path).map_err(|err| {
                format!(
                    "Failed to create directory {}: {err}",
                    resolved.absolute_path.display()
                )
            })?;
        }

        Ok(json!({
            "path": relative_path_display(&resolved.relative_path),
            "created": !existed_before,
            "alreadyExisted": existed_before,
            "recursive": args.recursive,
        }))
    }
}
