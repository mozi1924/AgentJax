mod calculator;
mod catalog;
mod files;
mod native;
mod registry;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub(crate) use catalog::ToolCatalogExecution;
pub use catalog::{
    MountedToolDefinition, MountedToolSourceSession, MountedToolSourceSessions, ToolCatalog,
    ToolCatalogSnapshot, ToolCatalogStateChange,
};
pub use files::{EditFileTool, FileReaderTool, FileWriterTool, ListFilesTool, MkdirTool};
pub use native::{CalculatorTool, SystemTimeTool};
pub use registry::ToolRegistry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSchemaFormat {
    Responses,
    ChatCompletions,
}

pub fn format_tool_schema(
    format: ToolSchemaFormat,
    name: &str,
    description: &str,
    parameters: Value,
) -> Value {
    match format {
        ToolSchemaFormat::Responses => json!({
            "type": "function",
            "name": name,
            "description": description,
            "parameters": parameters,
        }),
        ToolSchemaFormat::ChatCompletions => json!({
            "type": "function",
            "function": {
                "name": name,
                "description": description,
                "parameters": parameters,
            }
        }),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolPresentation {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

impl ToolPresentation {
    pub fn new(
        display_name: impl Into<String>,
        description: impl Into<String>,
        icon: Option<impl Into<String>>,
    ) -> Self {
        Self {
            display_name: display_name.into(),
            description: description.into(),
            icon: icon.map(Into::into),
        }
    }
}

pub fn humanize_tool_name(name: &str) -> String {
    name.split(['_', '-', '.'])
        .filter(|part| !part.trim().is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters_schema(&self) -> Value;
    fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> Result<Value, String>;
    fn display_name(&self) -> &'static str {
        self.name()
    }
    fn icon(&self) -> Option<&'static str> {
        None
    }

    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::new(
            self.display_name(),
            self.description(),
            self.icon().map(str::to_string),
        )
    }

    fn to_schema_with_format(&self, format: ToolSchemaFormat) -> Value {
        format_tool_schema(
            format,
            self.name(),
            self.description(),
            self.parameters_schema(),
        )
    }

    fn to_schema(&self) -> Value {
        self.to_schema_with_format(ToolSchemaFormat::Responses)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ToolExecutionContext {
    pub conversation_id: Option<String>,
}
