import { useMemo } from 'react';
import type { SettingsSchemaNode } from '../../../../features/settings/types';
import { collectSchemaDataSourceNamespaces } from '../../../../features/settings/schemaRendererView';
import type { SchemaRendererDataContext, SchemaRendererQueryState } from '../types';
import { useRegisteredSchemaDataProviders } from './providerRegistry';
import { mergeSchemaDataProviders } from './runtime';

// Centralizes optional renderer data sources so individual settings sections do
// not need to wrap SchemaRenderer with bespoke UI adapters.
export function useSchemaDataContext({
  nodes,
  queryState,
  onSaveField,
}: {
  nodes: SettingsSchemaNode[];
  queryState?: SchemaRendererQueryState;
  onSaveField: (path: string, value: unknown) => Promise<void>;
}): SchemaRendererDataContext | undefined {
  const requestedDataSourceNamespaces = useMemo(
    () => collectSchemaDataSourceNamespaces(nodes),
    [nodes]
  );
  const providers = useRegisteredSchemaDataProviders({
    nodes,
    requestedDataSourceNamespaces,
    queryState,
    onSaveField,
  });

  return useMemo(() => mergeSchemaDataProviders(providers), [providers]);
}
