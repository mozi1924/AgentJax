import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  PERMISSION_LABELS,
  type PluginEntrySnapshot,
  type PluginManagerSnapshot,
} from './pluginManagerData';
import type { SchemaDataProvider, SchemaDataProviderArgs } from './types';

const asRecord = (value: unknown): Record<string, unknown> =>
  value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};

const labelForOrigin = (plugin: PluginEntrySnapshot): string =>
  plugin.isBuiltin ? 'Built-in' : 'External';

const labelForHasTools = (plugin: PluginEntrySnapshot): string =>
  plugin.hasTools ? 'Has Tools' : 'Provider Only';

const declaredPermissionList = (plugin: PluginEntrySnapshot | null) => {
  if (!plugin) return [];
  const perms = plugin.declaredPermissions;
  const list: Array<{
    name: string;
    type: string;
    description: string;
    required: boolean;
    permissionKey?: string;
    actionId?: string;
  }> = [];

  if (perms.allowNetwork) {
    list.push({
      name: 'Network Access',
      type: perms.allowedHosts.length > 0 ? `Hosts: ${perms.allowedHosts.join(', ')}` : 'Any',
      description: 'Allows the plugin to make network requests.',
      required: false,
      permissionKey: 'allowNetwork',
      actionId: 'togglePermission_allowNetwork',
    });
  }
  if (perms.allowFileRead) {
    list.push({
      name: 'File Read',
      type: 'Permission',
      description: 'Allows the plugin to read files from the filesystem.',
      required: false,
      permissionKey: 'allowFileRead',
      actionId: 'togglePermission_allowFileRead',
    });
  }
  if (perms.allowFileWrite) {
    list.push({
      name: 'File Write',
      type: 'Permission',
      description: 'Allows the plugin to write files to the filesystem.',
      required: false,
      permissionKey: 'allowFileWrite',
      actionId: 'togglePermission_allowFileWrite',
    });
  }
  if (perms.allowProcessSpawn) {
    list.push({
      name: 'Process Spawn',
      type: 'Permission',
      description: 'Allows the plugin to spawn child processes.',
      required: false,
      permissionKey: 'allowProcessSpawn',
      actionId: 'togglePermission_allowProcessSpawn',
    });
  }
  if (perms.allowEnvRead) {
    list.push({
      name: 'Environment Variables',
      type: 'Permission',
      description: 'Allows the plugin to read environment variables.',
      required: false,
      permissionKey: 'allowEnvRead',
      actionId: 'togglePermission_allowEnvRead',
    });
  }

  return list;
};

export function usePluginManagerDataProvider({
  requestedDataSourceNamespaces,
  onSaveField,
}: SchemaDataProviderArgs): SchemaDataProvider {
  const enabled = requestedDataSourceNamespaces.includes('pluginManager');
  const [snapshot, setSnapshot] = useState<PluginManagerSnapshot | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState('');
  const [actionError, setActionError] = useState('');
  const [savingKeys, setSavingKeys] = useState<Set<string>>(new Set());
  const [selectedPluginId, setSelectedPluginId] = useState('');
  const [query, setQuery] = useState('');

  const refreshSnapshot = async () => {
    const nextSnapshot = await invoke<PluginManagerSnapshot>(
      'get_plugin_manager_snapshot'
    );
    setSnapshot(nextSnapshot);
    return nextSnapshot;
  };

  useEffect(() => {
    if (!enabled) return undefined;
    let disposed = false;
    setLoading(true);
    setLoadError('');
    invoke<PluginManagerSnapshot>('get_plugin_manager_snapshot')
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

  const plugins = useMemo(() => {
    const allPlugins = snapshot?.plugins || [];
    if (!query.trim()) {
      return allPlugins.map((plugin) => ({
        ...plugin,
        activePluginId: selectedPluginId,
        originLabel: labelForOrigin(plugin),
        hasToolsLabel: labelForHasTools(plugin),
        pluginSavingKey: `plugin:${plugin.id}:enabled`,
        declaredPermissionList: declaredPermissionList(plugin),
      }));
    }
    const normalizedQuery = query.trim().toLowerCase();
    return allPlugins
      .filter(
        (plugin) =>
          plugin.id.toLowerCase().includes(normalizedQuery) ||
          plugin.name.toLowerCase().includes(normalizedQuery) ||
          plugin.description.toLowerCase().includes(normalizedQuery)
      )
      .map((plugin) => ({
        ...plugin,
        activePluginId: selectedPluginId,
        originLabel: labelForOrigin(plugin),
        hasToolsLabel: labelForHasTools(plugin),
        pluginSavingKey: `plugin:${plugin.id}:enabled`,
        declaredPermissionList: declaredPermissionList(plugin),
      }));
  }, [snapshot, query, selectedPluginId]);

  const selectedPlugin = useMemo(
    () => plugins.find((p) => p.id === selectedPluginId) || plugins[0] || null,
    [plugins, selectedPluginId]
  );

  useEffect(() => {
    if (!selectedPlugin && plugins[0]) {
      setSelectedPluginId(plugins[0].id);
      return;
    }
    if (selectedPlugin) {
      setSelectedPluginId(selectedPlugin.id);
    }
  }, [plugins, selectedPlugin]);

  const savePolicy = async (
    path: string | null,
    value: unknown,
    savingKey: string
  ) => {
    if (!path) return;
    setSavingKeys((current) => new Set(current).add(savingKey));
    setActionError('');
    try {
      await onSaveField(path, value);
      await refreshSnapshot();
    } catch (err) {
      setActionError(typeof err === 'string' ? err : String(err));
    } finally {
      setSavingKeys((current) => {
        const next = new Set(current);
        next.delete(savingKey);
        return next;
      });
    }
  };

  const hydratedPlugins = useMemo(
    () =>
      plugins.map((plugin) => ({
        ...plugin,
        actionError,
        permissionSavingKey_allowNetwork: `perm:${plugin.id}:allowNetwork`,
        permissionSavingKey_allowFileRead: `perm:${plugin.id}:allowFileRead`,
        permissionSavingKey_allowFileWrite: `perm:${plugin.id}:allowFileWrite`,
        permissionSavingKey_allowProcessSpawn: `perm:${plugin.id}:allowProcessSpawn`,
        permissionSavingKey_allowEnvRead: `perm:${plugin.id}:allowEnvRead`,
      })),
    [actionError, plugins]
  );

  const hydratedSelectedPlugin = useMemo(() => {
    if (!selectedPlugin) return null;
    return (
      hydratedPlugins.find((p) => p.id === selectedPlugin.id) || null
    );
  }, [hydratedPlugins, selectedPlugin]);

  return {
    namespace: 'pluginManager',
    enabled,
    getDataSource: (dataSource) => {
      if (dataSource === 'pluginManager' || dataSource === 'pluginManager.root') {
        return snapshot;
      }
      if (dataSource === 'pluginManager.plugins') {
        return hydratedPlugins;
      }
      if (dataSource === 'pluginManager.selectedPlugin') {
        return hydratedSelectedPlugin || {};
      }
      if (dataSource === 'pluginManager.query') {
        return {
          search: query,
          activePluginId: selectedPluginId,
        };
      }
      return undefined;
    },
    getStatus: (dataSource) =>
      dataSource === 'pluginManager' || dataSource === 'pluginManager.root'
        ? {
            loading,
            error: loadError,
            loadingText: 'settings.plugins.loading',
            errorText: 'settings.plugins.error',
          }
        : undefined,
    dispatch: async (action, payload) => {
      const record = asRecord(payload);
      const item = asRecord(record.item);
      const dispatchPath = typeof record.path === 'string' ? record.path : null;
      if (action === 'selectPlugin') {
        setSelectedPluginId(String(record.value || item.id || ''));
        return;
      }
      if (action === 'setSearch') {
        setQuery(String(record.value || ''));
        return;
      }
      if (action === 'togglePluginEnabled') {
        await savePolicy(
          dispatchPath,
          record.value,
          `plugin:${String(item.id || '')}:enabled`
        );
        return;
      }
      if (action.startsWith('togglePermission_')) {
        const permKey = action.replace('togglePermission_', '') as keyof typeof PERMISSION_LABELS;
        await savePolicy(
          dispatchPath,
          record.value,
          `perm:${String(item.id || '')}:${permKey}`
        );
        return;
      }
    },
  };
}
