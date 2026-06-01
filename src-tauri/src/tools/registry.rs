use crate::tools::{
    CalculatorTool, EditFileTool, FileReaderTool, FileWriterTool, ListFilesTool, MkdirTool,
    SystemTimeTool, Tool, ToolExecutionContext, ToolSchemaFormat,
};
use serde_json::Value;

pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new_with_defaults() -> Self {
        Self {
            tools: vec![
                Box::new(CalculatorTool),
                Box::new(SystemTimeTool),
                Box::new(FileReaderTool),
                Box::new(FileWriterTool),
                Box::new(ListFilesTool),
                Box::new(MkdirTool),
                Box::new(EditFileTool),
            ],
        }
    }

    pub fn list_schemas(&self) -> Vec<Value> {
        self.list_schemas_with_format(ToolSchemaFormat::Responses)
    }

    pub fn list_schemas_with_format(&self, format: ToolSchemaFormat) -> Vec<Value> {
        self.tools
            .iter()
            .map(|tool| tool.to_schema_with_format(format))
            .collect()
    }

    pub async fn execute(
        &self,
        name: &str,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> crate::error::AgentJaxResult<Value> {
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.name() == name)
            .ok_or_else(|| crate::error::AgentJaxError::not_found(format!("Tool '{}' not found in registry", name)))?;

        tool.execute(arguments, context).await
    }
}
