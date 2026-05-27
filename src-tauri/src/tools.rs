mod catalog;
mod math;
mod native;
mod registry;

use serde_json::{json, Value};

pub use catalog::{ToolCatalog, ToolCatalogSnapshot};
pub use native::{CalculatorTool, FileReaderTool, FileWriterTool, SystemTimeTool};
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

pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters_schema(&self) -> Value;
    fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> Result<Value, String>;

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
