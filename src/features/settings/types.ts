export type SettingsValueType =
  | 'boolean'
  | 'integer'
  | 'float'
  | 'string'
  | 'enum'
  | 'secret'
  | 'string_list'
  | 'string_map'
  | 'json_map'
  | 'object'
  | 'object_collection';

export type SettingsControlType =
  | 'switch'
  | 'select'
  | 'text'
  | 'textarea'
  | 'number'
  | 'secret'
  | 'tags'
  | 'key_value'
  | 'json'
  | 'prompt_assembler';

export interface SettingsOption {
  label: string;
  value: string;
}

export interface SecretStatus {
  configured: boolean;
  source: string;
}

export interface SettingsSnapshot {
  configPath: string;
  revision: string;
  values: Record<string, unknown>;
  dynamicOptions: Record<string, SettingsOption[]>;
  secretStatuses: Record<string, SecretStatus>;
}

export interface SettingsUiSnapshot {
  snapshot: SettingsSnapshot;
  sections: SettingsSectionSchema[];
}

export interface SettingsSnapshotEvent extends SettingsSnapshot {
  origin: 'internal' | 'external' | string;
}

export interface SettingsPatchRequest {
  path: string;
  value?: unknown;
  expectedRevision: string;
  operation?: 'set' | 'delete';
}

export interface SettingsCondition {
  path: string;
  equals?: string | number | boolean | null;
  notEquals?: string | number | boolean | null;
  includes?: string;
  truthy?: boolean;
}

interface SettingsNodeBase {
  id: string;
  title: string;
  description?: string;
  helpText?: string;
  warningText?: string;
  advanced?: boolean;
  visibleWhen?: SettingsCondition[];
  enabledWhen?: SettingsCondition[];
}

export interface SettingsFieldSchema extends SettingsNodeBase {
  kind: 'field';
  path: string;
  valueType: SettingsValueType;
  control: SettingsControlType;
  placeholder?: string;
  min?: number;
  max?: number;
  step?: number;
  minLength?: number;
  maxLength?: number;
  pattern?: string;
  options?: SettingsOption[];
  optionSourceKey?: string;
  rows?: number;
}

export interface SettingsGroupSchema extends SettingsNodeBase {
  kind: 'group';
  children: SettingsSchemaNode[];
}

export interface SettingsCollectionSchema extends SettingsNodeBase {
  kind: 'collection';
  path: string;
  valueType: 'object_collection';
  addLabel: string;
  keyLabel: string;
  itemLabel: string;
  keyPattern?: string;
  defaultItem: Record<string, unknown>;
  children: SettingsSchemaNode[];
}

export type SettingsSchemaNode =
  | SettingsFieldSchema
  | SettingsGroupSchema
  | SettingsCollectionSchema;

export interface SettingsSectionSchema {
  id: string;
  title: string;
  icon: string;
  order: number;
  description?: string;
  children: SettingsSchemaNode[];
}

export interface SettingsModuleSchema {
  namespace: string;
  sections: SettingsSectionSchema[];
}

export interface SettingsRegistry {
  sections: SettingsSectionSchema[];
}
