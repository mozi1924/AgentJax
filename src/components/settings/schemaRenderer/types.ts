import type { Dispatch, ReactNode, SetStateAction } from 'react';
import type {
  SecretStatus,
  SettingsFieldSchema,
  SettingsSchemaNode,
  SettingsSnapshot,
  SettingsUiAction,
} from '../../../features/settings/types';

export interface SchemaRendererActions {
  saveField: (path: string, value: unknown) => Promise<void>;
  deletePath?: (path: string) => Promise<void>;
  addCollectionItem?: (
    path: string,
    key: string,
    value: Record<string, unknown>
  ) => Promise<void>;
  discover?: (id?: string) => Promise<void> | void;
  refresh?: (id?: string) => Promise<void> | void;
  togglePolicy?: (path: string, enabled: boolean) => Promise<void> | void;
  setExposure?: (path: string, exposure: string) => Promise<void> | void;
  runAction?: (action: SettingsUiAction) => Promise<void> | void;
}

export interface SchemaRendererQueryState {
  search?: string;
  onSearchChange?: (search: string) => void;
  filter?: string;
  activeTab?: string;
  selectedItem?: string;
  [key: string]: unknown;
}

export interface SchemaRendererProps {
  nodes: SettingsSchemaNode[];
  snapshot: SettingsSnapshot;
  savingPath: string | null;
  fieldErrors: Record<string, string>;
  contextPath?: string;
  container?: 'stack' | 'fragment';
  actions: SchemaRendererActions;
  queryState?: SchemaRendererQueryState;
  dataContext?: SchemaRendererDataContext;
}

export interface SchemaRendererDataContext {
  getDataSource: (dataSource?: string) => unknown;
  dispatch: (action: string, payload?: unknown) => void | Promise<void>;
  isSaving?: (savingKey?: string) => boolean;
  getStatus?: (dataSource?: string) =>
    | {
        loading?: boolean;
        error?: string;
        loadingText?: string;
        errorText?: string;
      }
    | undefined;
}

export interface FieldControlProps {
  field: SettingsFieldSchema;
  resolvedPath: string;
  value: unknown;
  draft: string;
  setDraft: Dispatch<SetStateAction<string>>;
  isDirty: boolean;
  setIsDirty: Dispatch<SetStateAction<boolean>>;
  disabled: boolean;
  options: Array<{ label: string; value: string }>;
  secretStatus?: SecretStatus;
  commit: (value: unknown) => Promise<void>;
  onSaveField: (path: string, value: unknown) => Promise<void>;
  setLocalError: Dispatch<SetStateAction<string | null>>;
}

export interface FieldShellProps {
  field: SettingsFieldSchema;
  resolvedPath: string;
  secretStatus?: SecretStatus;
  helperText: string;
  hasError: boolean;
  fullWidth: boolean;
  children: ReactNode;
}
