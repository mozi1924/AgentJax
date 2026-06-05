use crate::error::{AgentJaxError, AgentJaxResult};
use crate::tools::{Tool, ToolExecutionContext};
use serde::Deserialize;
use serde_json::{Value, json};

use super::common::{
    DEFAULT_READ_MAX_BYTES, MAX_READ_MAX_BYTES, attach_file_type_metadata, count_lines,
    read_text_file, relative_path_display, resolve_workspace_path,
};

#[derive(Debug, Deserialize)]
pub struct ReadFileArgs {
    #[serde(alias = "filename")]
    pub path: String,
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

pub struct FileReaderTool;

#[async_trait::async_trait]
impl Tool for FileReaderTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn display_name(&self) -> &'static str {
        "Read File"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("FileSearch")
    }

    fn description(&self) -> &'static str {
        "Reads a UTF-8 text file preview from the current conversation workspace. Large files are truncated, and content-based type sniffing rejects binary files even when the extension looks text-like."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Workspace-relative file path to read, e.g. 'src/components/Sidebar.tsx'."
                },
                "filename": {
                    "type": "string",
                    "description": "Legacy alias for 'path'. New callers should send 'path'."
                },
                "max_bytes": {
                    "type": "integer",
                    "description": "Optional maximum number of UTF-8 bytes to return. Defaults to 32768 and is capped at 262144."
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(
        &self,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> AgentJaxResult<Value> {
        let args = super::common::parse_tool_args::<ReadFileArgs>(arguments, self.name())?;
        let resolved = resolve_workspace_path(&args.path, context, false)?;
        if !resolved.absolute_path.exists() {
            return Err(AgentJaxError::not_found(format!(
                "File '{}' not found in current conversation workspace",
                relative_path_display(&resolved.relative_path)
            )));
        }

        let max_bytes = args
            .max_bytes
            .unwrap_or(DEFAULT_READ_MAX_BYTES)
            .clamp(1, MAX_READ_MAX_BYTES);
        let text = read_text_file(&resolved.absolute_path, max_bytes, "read")?;
        let line_count = count_lines(&text.content);
        let mut response = json!({
            "path": relative_path_display(&resolved.relative_path),
            "content": text.content,
            "bytesRead": text.returned_bytes,
            "totalBytes": text.total_bytes,
            "lineCount": line_count,
            "truncated": text.truncated,
            "maxBytes": max_bytes,
        });
        if let Some(object) = response.as_object_mut() {
            attach_file_type_metadata(object, &text.file_type);
        }

        Ok(response)
    }
}
