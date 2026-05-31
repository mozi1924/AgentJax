import type { SchemaRendererDataContext } from '../types';
import type { SchemaDataProvider } from './types';

export const dataSourceMatchesNamespace = (dataSource: string | undefined, namespace: string) =>
  !!dataSource && (dataSource === namespace || dataSource.startsWith(`${namespace}.`));

export const providerForDataSource = (
  providers: SchemaDataProvider[],
  dataSource?: string
): SchemaDataProvider | undefined =>
  providers.find(
    (provider) =>
      provider.enabled && dataSourceMatchesNamespace(dataSource, provider.namespace)
  );

const asRecord = (value: unknown): Record<string, unknown> =>
  value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};

// Merges enabled providers into the generic SchemaRenderer data context. Dispatch
// is routed by payload.dataSource when present, otherwise broadcast for global actions.
export const mergeSchemaDataProviders = (
  providers: SchemaDataProvider[]
): SchemaRendererDataContext | undefined => {
  const activeProviders = providers.filter((provider) => provider.enabled);
  if (activeProviders.length === 0) return undefined;

  return {
    getDataSource: (dataSource) => providerForDataSource(activeProviders, dataSource)?.getDataSource(dataSource),
    getStatus: (dataSource) => providerForDataSource(activeProviders, dataSource)?.getStatus?.(dataSource),
    isSaving: (savingKey) =>
      activeProviders.some((provider) => !!provider.isSaving?.(savingKey)),
    dispatch: async (action, payload) => {
      const dataSource = asRecord(payload).dataSource;
      const provider =
        typeof dataSource === 'string'
          ? providerForDataSource(activeProviders, dataSource)
          : undefined;
      if (provider) {
        await provider.dispatch(action, payload);
        return;
      }
      await Promise.all(activeProviders.map((activeProvider) => activeProvider.dispatch(action, payload)));
    },
  };
};
