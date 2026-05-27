use crate::tools::{
    ApplyPatchTool, CalculatorTool, FileReaderTool, FileWriterTool, InsertAfterTool,
    InsertBeforeTool, ListFilesTool, MkdirTool, ReplaceBlockTool, ReplaceTextTool, StatFileTool,
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
                Box::new(StatFileTool),
                Box::new(MkdirTool),
                Box::new(ReplaceTextTool),
                Box::new(ReplaceBlockTool),
                Box::new(ApplyPatchTool),
                Box::new(InsertAfterTool),
                Box::new(InsertBeforeTool),
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

    pub fn execute(
        &self,
        name: &str,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> Result<Value, String> {
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.name() == name)
            .ok_or_else(|| format!("Tool '{}' not found in registry", name))?;

        tool.execute(arguments, context)
    }
}
