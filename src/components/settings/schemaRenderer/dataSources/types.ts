import type { SettingsSchemaNode } from '../../../../features/settings/types';
import type { SchemaRendererDataContext, SchemaRendererQueryState } from '../types';

export interface SchemaDataProviderArgs {
  nodes: SettingsSchemaNode[];
  requestedDataSourceNamespaces: string[];
  queryState?: SchemaRendererQueryState;
  agentId?: string;
  onSaveField: (path: string, value: unknown) => Promise<void>;
}

export interface SchemaDataProvider extends SchemaRendererDataContext {
  namespace: string;
  enabled: boolean;
}
