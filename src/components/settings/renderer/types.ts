import type {
  SettingsSchemaNode,
  SettingsSectionSchema,
  SettingsSnapshot,
} from '../../../features/settings/types';

export interface SettingsRendererProps {
  section: SettingsSectionSchema;
  snapshot: SettingsSnapshot;
  savingPath: string | null;
  fieldErrors: Record<string, string>;
  onSaveField: (path: string, value: unknown) => Promise<void>;
  onDeletePath: (path: string) => Promise<void>;
  onAddCollectionItem: (
    path: string,
    key: string,
    value: Record<string, unknown>
  ) => Promise<void>;
}

export interface NodeListProps extends Omit<SettingsRendererProps, 'section'> {
  nodes: SettingsSchemaNode[];
  contextPath?: string;
}

export interface KeyValueEntry {
  id: string;
  key: string;
  value: string;
}

