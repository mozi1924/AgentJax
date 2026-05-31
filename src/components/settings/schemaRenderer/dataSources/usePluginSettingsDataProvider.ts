import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { SchemaDataProvider, SchemaDataProviderArgs } from './types';
import {
  asPluginRecord,
  pluginItemIdentity,
  type PluginSettingsSnapshot,
  resolvePluginSelectedItem,
  resolvePluginSettingsList,
} from './pluginSettingsData';

// Generic manifest-backed provider for plugin SchemaRenderer sections. It keeps
// selection/search behavior in the shared renderer path so plugins can avoid
// shipping custom React panels for ordinary settings views.
export function usePluginSettingsDataProvider({
  requestedDataSourceNamespaces,
  queryState,
}: SchemaDataProviderArgs): SchemaDataProvider {
  const enabled = requestedDataSourceNamespaces.includes('plugin');
  const [snapshot, setSnapshot] = useState<PluginSettingsSnapshot | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState('');
  const [localSearch, setLocalSearch] = useState('');
  const [selectedIds, setSelectedIds] = useState<Record<string, string>>({});
  const effectiveSearch = queryState?.search ?? localSearch;

  useEffect(() => {
    if (!enabled) return undefined;
    let disposed = false;
    setLoading(true);
    setLoadError('');
    invoke<PluginSettingsSnapshot>('get_plugin_settings_snapshot')
      .then((nextSnapshot) => {
        if (!disposed) setSnapshot(nextSnapshot);
      })
      .catch((err) => {
        if (!disposed) setLoadError(typeof err === 'string' ? err : String(err));
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, [enabled]);

  useEffect(() => {
    if (queryState?.search !== undefined) {
      setLocalSearch(queryState.search);
    }
  }, [queryState?.search]);

  const resolveList = useMemo(
    () => (dataSource: string) =>
      resolvePluginSettingsList({
        snapshot,
        dataSource,
        search: effectiveSearch,
        selectedIds,
      }),
    [effectiveSearch, selectedIds, snapshot]
  );

  const resolveSelectedItem = (dataSource: string) =>
    resolvePluginSelectedItem({
      snapshot,
      dataSource,
      search: effectiveSearch,
      selectedIds,
    });

  return {
    namespace: 'plugin',
    enabled,
    getDataSource: (dataSource) => {
      if (!dataSource) return undefined;
      if (dataSource === 'plugin.query') {
        return { search: effectiveSearch };
      }
      const selectedItem = resolveSelectedItem(dataSource);
      if (selectedItem !== undefined) {
        return selectedItem;
      }
      const value = snapshot?.dataSources[dataSource];
      if (Array.isArray(value)) {
        return resolveList(dataSource);
      }
      return value;
    },
    getStatus: (dataSource) =>
      !dataSource || dataSource === 'plugin' || dataSource.startsWith('plugin.')
        ? {
            loading,
            error: loadError,
          }
        : undefined,
    dispatch: (action, payload) => {
      const record = asPluginRecord(payload);
      if (action === 'setSearch') {
        const nextSearch = String(record.value || '');
        setLocalSearch(nextSearch);
        queryState?.onSearchChange?.(nextSearch);
        return;
      }

      if (action === 'selectItem' || action.startsWith('select')) {
        const dataSource = typeof record.dataSource === 'string' ? record.dataSource : '';
        if (!dataSource) return;
        const itemId = String(record.value || pluginItemIdentity(record.item));
        setSelectedIds((current) => ({
          ...current,
          [dataSource]: itemId,
        }));
      }
    },
  };
}
