import type { SettingsRendererProps } from './types';
import { ToolManagerSchemaAdapter } from '../toolManager/ToolManagerSchemaAdapter';
import { SchemaRenderer } from '../schemaRenderer';

export default function SettingsRenderer(props: SettingsRendererProps) {
  return (
    <SchemaRenderer
      nodes={props.section.children}
      snapshot={props.snapshot}
      savingPath={props.savingPath}
      fieldErrors={props.fieldErrors}
      valueSource="settings"
      actions={{
        saveField: props.onSaveField,
        deletePath: props.onDeletePath,
        addCollectionItem: props.onAddCollectionItem,
      }}
      renderUiNode={({ node, defaultRender }) => {
        if (node.dataSource === 'toolManager') {
          return (
            <ToolManagerSchemaAdapter
              title={node.title}
              description={node.description}
              nodes={[node]}
              snapshot={props.snapshot}
              savingPath={props.savingPath}
              fieldErrors={props.fieldErrors}
              onSaveField={props.onSaveField}
            />
          );
        }
        return defaultRender();
      }}
    />
  );
}
