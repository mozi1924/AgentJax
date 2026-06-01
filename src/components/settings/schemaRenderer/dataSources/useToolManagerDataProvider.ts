import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  TOOL_MANAGER_CATEGORIES,
  filterToolsForQuery,
  mcpExposurePolicyPath,
  selectToolManagerSource,
  sourceIdentityKey,
  sourcePolicyEnabledPath,
  sourcesForCategory,
  toolPolicyEnabledPath,
  type McpExposureMode,
  type ToolCategory,
  type ToolManagerSnapshot,
  type ToolManagerSourceSnapshot,
  type ToolManagerToolSnapshot,
} from './toolManagerData';
import type { SchemaDataProvider, SchemaDataProviderArgs } from './types';

const asRecord = (value: unknown): Record<string, unknown> =>
  value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};

const asStringArray = (value: unknown): string[] =>
  Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];

const schemaTypeLabel = (schema: Record<string, unknown>) => {
  const enumValues = asStringArray(schema.enum);
  const rawType = schema.type;
  const type = Array.isArray(rawType)
    ? rawType.filter((item): item is string => typeof item === 'string').join(' | ')
    : typeof rawType === 'string'
      ? rawType
      : 'value';
  return enumValues.length > 0 ? `${type} enum(${enumValues.join(', ')})` : type;
};

const schemaProperties = (tool: ToolManagerToolSnapshot | null) => {
  const schema = asRecord(tool?.inputSchema);
  const properties = asRecord(schema.properties);
  const required = new Set(asStringArray(schema.required));
  return Object.entries(properties).map(([name, property]) => {
    const propertyRecord = asRecord(property);
    return {
      name,
      type: schemaTypeLabel(propertyRecord),
      required: required.has(name),
      description:
        typeof propertyRecord.description === 'string' ? propertyRecord.description : '',
    };
  });
};

const normalizedExposure = (source: ToolManagerSourceSnapshot): McpExposureMode =>
  source.exposureMode === 'unfolded' ? 'unfolded' : 'collapsed';

// Loads and mutates only Tool Manager data; the visual structure stays in tools.json
// and is rendered by the shared SchemaRenderer data-source components.
export function useToolManagerDataProvider({
  requestedDataSourceNamespaces,
  search,
  onSearchChange,
  onSaveField,
}: SchemaDataProviderArgs & {
  search?: string;
  onSearchChange?: (search: string) => void;
}): SchemaDataProvider {
  const enabled = requestedDataSourceNamespaces.includes('toolManager');
  const [snapshot, setSnapshot] = useState<ToolManagerSnapshot | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState('');
  const [actionError, setActionError] = useState('');
  const [mcpDiscovering, setMcpDiscovering] = useState(false);
  const [mcpDiscoverError, setMcpDiscoverError] = useState('');
  const [discoveredSourceIds, setDiscoveredSourceIds] = useState<Set<string>>(new Set());
  const [savingKeys, setSavingKeys] = useState<Set<string>>(new Set());
  const [activeCategory, setActiveCategory] = useState<ToolCategory>('native');
  const [selectedSourceKey, setSelectedSourceKey] = useState('');
  const [selectedToolId, setSelectedToolId] = useState('');
  const [localSearch, setLocalSearch] = useState('');

  const effectiveSearch = search ?? localSearch;

  const refreshSnapshot = async (options?: { discoverSourceId?: string }) => {
    const request = options?.discoverSourceId
      ? { sourceId: options.discoverSourceId, discover: true }
      : null;
    const nextSnapshot = await invoke<ToolManagerSnapshot>('get_tool_manager_snapshot', {
      request,
    });
    setSnapshot(nextSnapshot);
    if (options?.discoverSourceId) {
      setDiscoveredSourceIds((current) => new Set(current).add(options.discoverSourceId || ''));
    }
    return nextSnapshot;
  };

  const discoverSource = async (sourceId: string) => {
    setActionError('');
    setMcpDiscovering(true);
    setMcpDiscoverError('');
    try {
      await refreshSnapshot({ discoverSourceId: sourceId });
    } catch (err) {
      const errMsg = typeof err === 'string' ? err : String(err);
      setActionError(errMsg);
      setMcpDiscoverError(errMsg);
    } finally {
      setMcpDiscovering(false);
    }
  };

  useEffect(() => {
    if (!enabled) return undefined;
    let disposed = false;
    setLoading(true);
    setLoadError('');
    invoke<ToolManagerSnapshot>('get_tool_manager_snapshot', { request: null })
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
    if (search !== undefined) {
      setLocalSearch(search);
    }
  }, [search]);

  const categorySources = useMemo(() => {
    const sources = snapshot?.sources || [];
    return sourcesForCategory(sources, activeCategory);
  }, [activeCategory, snapshot]);

  const activeSource = useMemo(
    () => selectToolManagerSource(categorySources, selectedSourceKey),
    [categorySources, selectedSourceKey]
  );

  const filteredTools = useMemo(
    () => filterToolsForQuery(activeSource?.tools || [], effectiveSearch),
    [activeSource, effectiveSearch]
  );

  const selectedTool = useMemo(
    () => filteredTools.find((tool) => tool.id === selectedToolId) || filteredTools[0] || null,
    [filteredTools, selectedToolId]
  );

  useEffect(() => {
    if (!selectedTool && filteredTools[0]) {
      setSelectedToolId(filteredTools[0].id);
      return;
    }
    if (selectedTool) {
      setSelectedToolId(selectedTool.id);
    }
  }, [filteredTools, selectedTool]);

  const savePolicy = async (
    path: string | null,
    value: unknown,
    savingKey: string,
    source: ToolManagerSourceSnapshot | null
  ) => {
    if (!path) return;
    setSavingKeys((current) => new Set(current).add(savingKey));
    setActionError('');
    try {
      await onSaveField(path, value);
      const shouldRediscover =
        source?.sourceType === 'mcp' && discoveredSourceIds.has(source.sourceId);
      await refreshSnapshot({ discoverSourceId: shouldRediscover ? source.sourceId : undefined });
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

  const sources = useMemo(
    () =>
      categorySources.map((source) => ({
        ...source,
        identityKey: sourceIdentityKey(source),
        activeSourceKey: activeSource ? sourceIdentityKey(activeSource) : '',
        toolCount: String(source.tools.length),
        normalizedExposureMode: normalizedExposure(source),
        sourceSavingKey: `source:${source.sourceType}:${source.sourceId}:enabled`,
        exposureSavingKey: `source:${source.sourceType}:${source.sourceId}:exposure`,
        hint:
          source.error ||
          (source.sourceType === 'mcp' && source.tools.length === 0
            ? 'settings.tools.mcp_lazy_hint'
            : ''),
        actionError,
      })),
    [actionError, activeSource, categorySources]
  );

  const hydratedActiveSource = useMemo(() => {
    if (!activeSource) return null;
    return sources.find((source) => sourceIdentityKey(source) === sourceIdentityKey(activeSource)) || null;
  }, [activeSource, sources]);

  const tools = useMemo(
    () =>
      filteredTools.map((tool) => ({
        ...tool,
        activeToolId: selectedTool?.id || '',
        schemaProperties: schemaProperties(tool),
        toolSavingKey: activeSource
          ? `tool:${activeSource.sourceType}:${activeSource.sourceId}:${tool.id}:enabled`
          : '',
      })),
    [activeSource, filteredTools, selectedTool]
  );

  const hydratedSelectedTool = useMemo(() => {
    if (!selectedTool) return null;
    return tools.find((tool) => tool.id === selectedTool.id) || null;
  }, [selectedTool, tools]);

  return {
    namespace: 'toolManager',
    enabled,
    getDataSource: (dataSource) => {
      if (dataSource === 'toolManager' || dataSource === 'toolManager.root') {
        return snapshot;
      }
      if (dataSource === 'toolManager.categories') {
        return snapshot?.categories || TOOL_MANAGER_CATEGORIES;
      }
      if (dataSource === 'toolManager.sources') {
        return sources;
      }
      if (dataSource === 'toolManager.activeSource') {
        return hydratedActiveSource || {};
      }
      if (dataSource === 'toolManager.tools') {
        return tools;
      }
      if (dataSource === 'toolManager.selectedTool') {
        return hydratedSelectedTool || {};
      }
      if (dataSource === 'toolManager.query') {
        return {
          activeTab: activeCategory,
          search: effectiveSearch,
          selectedItem: selectedTool?.id || '',
        };
      }
      return undefined;
    },
    getStatus: (dataSource) => {
      if (dataSource === 'toolManager' || dataSource === 'toolManager.root') {
        return {
          loading,
          error: loadError,
          loadingText: 'settings.tools.loading',
          errorText: 'settings.tools.error',
        };
      }
      if (dataSource === 'toolManager.tools') {
        return {
          loading: mcpDiscovering,
          error: mcpDiscoverError,
          loadingText: 'settings.tools.mcp_loading',
          errorText: 'settings.tools.mcp_error',
        };
      }
      return undefined;
    },
    dispatch: async (action, payload) => {
      const record = asRecord(payload);
      const item = asRecord(record.item);
      if (action === 'selectCategory') {
        const defaultCategory = snapshot?.categories?.[0]?.id || 'native';
        setActiveCategory(String(record.value || item.id || defaultCategory) as ToolCategory);
        setSelectedSourceKey('');
        setSelectedToolId('');
        return;
      }
      if (action === 'selectSource') {
        const source = item as unknown as ToolManagerSourceSnapshot;
        setSelectedSourceKey(sourceIdentityKey(source));
        setSelectedToolId('');
        setActionError('');
        if (source.sourceType === 'mcp') {
          await discoverSource(source.sourceId);
        }
        return;
      }
      if (action === 'selectTool') {
        setSelectedToolId(String(record.value || item.id || ''));
        return;
      }
      if (action === 'setSearch') {
        const nextSearch = String(record.value || '');
        setLocalSearch(nextSearch);
        onSearchChange?.(nextSearch);
        return;
      }
      if (action === 'discoverSource') {
        if (activeSource?.sourceType === 'mcp') {
          await discoverSource(activeSource.sourceId);
        }
        return;
      }
      if (action === 'toggleSourceEnabled') {
        await savePolicy(
          typeof record.path === 'string'
            ? record.path
            : sourcePolicyEnabledPath(item as unknown as ToolManagerSourceSnapshot),
          !!record.value,
          String(item.sourceSavingKey || ''),
          item as unknown as ToolManagerSourceSnapshot
        );
        return;
      }
      if (action === 'toggleToolEnabled' && activeSource) {
        await savePolicy(
          typeof record.path === 'string'
            ? record.path
            : toolPolicyEnabledPath(activeSource, item as unknown as ToolManagerToolSnapshot),
          !!record.value,
          String(item.toolSavingKey || ''),
          activeSource
        );
        return;
      }
      if (action === 'setMcpExposure') {
        await savePolicy(
          typeof record.path === 'string'
            ? record.path
            : mcpExposurePolicyPath(item as unknown as ToolManagerSourceSnapshot),
          String(record.value || 'collapsed'),
          String(item.exposureSavingKey || ''),
          item as unknown as ToolManagerSourceSnapshot
        );
      }
    },
    isSaving: (savingKey) => !!savingKey && savingKeys.has(savingKey),
  };
}
