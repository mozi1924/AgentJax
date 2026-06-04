import type { AppConfig } from './__generated__/config-types';

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

export type SettingsUiNodeKind =
  | 'layout'
  | 'panel'
  | 'tabs'
  | 'split'
  | 'toolbar'
  | 'list'
  | 'detail'
  | 'collapsible'
  | 'badge'
  | 'metric'
  | 'empty_state'
  | 'action';

export interface SettingsOption {
  label: string;
  value: string;
}

export interface SettingsUiProperty {
  id: string;
  label?: string;
  value: string;
  variant?: 'text' | 'code' | 'badge' | 'status' | string;
  visibleWhen?: SettingsCondition[];
}

export interface SecretStatus {
  configured: boolean;
  source: string;
}

export interface SettingsSnapshot {
  configPath: string;
  revision: string;
  /** Config values typed via codegen from Rust structs (snake_case fields).
   *  The intersection with `Record<string, unknown>` preserves index access
   *  for dynamic path-based lookups (`getValueAtPath`). */
  values: AppConfig & Record<string, unknown>;
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

export interface SettingsCondition {
  path: string;
  equals?: string | number | boolean | null;
  notEquals?: string | number | boolean | null;
  includes?: string;
  truthy?: boolean;
}

interface SettingsNodeBase {
  id: string;
  title?: string;
  description?: string;
  helpText?: string;
  warningText?: string;
  advanced?: boolean;
  visibleWhen?: SettingsCondition[];
  enabledWhen?: SettingsCondition[];
}

export interface SettingsCommonUiProps {
  layout?: string;
  variant?: string;
  density?: 'compact' | 'comfortable' | 'spacious' | string;
  width?: string | number;
  height?: string | number;
  scroll?: boolean | 'x' | 'y' | 'both';
  responsive?: string | Record<string, unknown>;
  icon?: string;
  badge?: string | number | boolean | Record<string, unknown>;
  status?: string;
  defaultExpanded?: boolean;
  actions?: SettingsUiAction[];
  properties?: SettingsUiProperty[];
  dataSource?: string;
  itemTemplate?: SettingsSchemaNode;
  bindings?: Record<string, string>;
  emptyText?: string;
  action?: string;
}

export interface SettingsUiAction {
  id: string;
  label?: string;
  action?: string;
  icon?: string;
  variant?: string;
  visibleWhen?: SettingsCondition[];
  disabledWhen?: SettingsCondition[];
  dataSource?: string;
  path?: string;
  value?: string;
  savingKey?: string;
  options?: SettingsOption[];
}

export interface SettingsFieldSchema extends SettingsNodeBase, SettingsCommonUiProps {
  kind: 'field';
  title: string;
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

export interface SettingsGroupSchema extends SettingsNodeBase, SettingsCommonUiProps {
  kind: 'group';
  title: string;
  children: SettingsSchemaNode[];
}

export interface SettingsCollectionSchema extends SettingsNodeBase, SettingsCommonUiProps {
  kind: 'collection';
  title: string;
  path: string;
  valueType: 'object_collection';
  addLabel: string;
  keyLabel: string;
  itemLabel: string;
  keyPattern?: string;
  defaultItem: Record<string, unknown>;
  children: SettingsSchemaNode[];
}

export interface SettingsUiTab {
  id: string;
  title: string;
  icon?: string;
  children: SettingsSchemaNode[];
}

export interface SettingsUiSchemaNode extends SettingsNodeBase, SettingsCommonUiProps {
  kind: SettingsUiNodeKind;
  title?: string;
  label?: string;
  value?: unknown;
  children?: SettingsSchemaNode[];
  tabs?: SettingsUiTab[];
}

export type SettingsSchemaNode =
  | SettingsFieldSchema
  | SettingsGroupSchema
  | SettingsCollectionSchema
  | SettingsUiSchemaNode;

export interface SettingsSectionSchema {
  id: string;
  title: string;
  icon: string;
  order: number;
  description?: string;
  children: SettingsSchemaNode[];
}

