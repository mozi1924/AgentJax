import { ToolManagerSchemaAdapter } from '../toolManager/ToolManagerSchemaAdapter';
import type { FieldRendererProps } from './types';

// Compatibility adapter for legacy v1 schemas that still use control="tool_manager".
export function ToolManagerField({
  field,
  snapshot,
  savingPath,
  fieldErrors,
  onSaveField,
}: FieldRendererProps) {
  return (
    <ToolManagerSchemaAdapter
      title={field.title}
      description={field.description}
      snapshot={snapshot}
      savingPath={savingPath}
      fieldErrors={fieldErrors}
      onSaveField={onSaveField}
    />
  );
}
