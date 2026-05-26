import type { SettingsRendererProps } from './types';
import { NodeList } from './NodeList';

export default function SettingsRenderer(props: SettingsRendererProps) {
  return (
    <NodeList
      nodes={props.section.children}
      snapshot={props.snapshot}
      savingPath={props.savingPath}
      fieldErrors={props.fieldErrors}
      onSaveField={props.onSaveField}
      onDeletePath={props.onDeletePath}
      onAddCollectionItem={props.onAddCollectionItem}
    />
  );
}

