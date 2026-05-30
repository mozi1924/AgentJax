import { useMemo, useState } from 'react';
import { AlertCircle, LoaderCircle, Wrench } from 'lucide-react';
import { useI18n } from '../../../features/i18n';
import type { SettingsSchemaNode, SettingsSnapshot } from '../../../features/settings/types';
import {
  TOOL_MANAGER_CATEGORIES,
  mcpExposurePolicyPath,
  sourceIdentityKey,
  sourcePolicyEnabledPath,
  toolPolicyEnabledPath,
  type McpExposureMode,
  type ToolCategory,
  type ToolManagerSourceSnapshot,
  type ToolManagerToolSnapshot,
} from '../../../features/settings/toolManagerView';
import { SchemaRenderer, type SchemaRendererDataContext } from '../schemaRenderer';
import { useToolManagerSelection } from './useToolManagerSelection';
import { useToolManagerSnapshot } from './useToolManagerSnapshot';

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

export function ToolManagerSchemaAdapter({
  title,
  description,
  nodes,
  snapshot: settingsSnapshot,
  savingPath,
  fieldErrors,
  onSaveField,
}: {
  title?: string;
  description?: string;
  nodes: SettingsSchemaNode[];
  snapshot: SettingsSnapshot;
  savingPath: string | null;
  fieldErrors: Record<string, string>;
  onSaveField: (path: string, value: unknown) => Promise<void>;
}) {
  const { t } = useI18n();
  const snapshotState = useToolManagerSnapshot();
  const selection = useToolManagerSelection(snapshotState.snapshot);
  const [savingKeys, setSavingKeys] = useState<Set<string>>(new Set());

  const refreshAfterSave = async (source: ToolManagerSourceSnapshot | null) => {
    const shouldRediscover =
      source?.sourceType === 'mcp' && snapshotState.discoveredSourceIds.has(source.sourceId);
    await snapshotState.refreshSnapshot({
      discoverSourceId: shouldRediscover ? source.sourceId : undefined,
    });
  };

  const savePolicy = async (
    path: string | null,
    value: unknown,
    savingKey: string,
    source: ToolManagerSourceSnapshot | null
  ) => {
    if (!path) return;
    setSavingKeys((current) => new Set(current).add(savingKey));
    snapshotState.setActionError('');
    try {
      await onSaveField(path, value);
      await refreshAfterSave(source);
    } catch (err) {
      snapshotState.setActionError(typeof err === 'string' ? err : String(err));
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
      selection.categorySources.map((source) => ({
        ...source,
        identityKey: sourceIdentityKey(source),
        activeSourceKey: selection.activeSource ? sourceIdentityKey(selection.activeSource) : '',
        toolCount: String(source.tools.length),
        normalizedExposureMode: normalizedExposure(source),
        sourceSavingKey: `source:${source.sourceType}:${source.sourceId}:enabled`,
        exposureSavingKey: `source:${source.sourceType}:${source.sourceId}:exposure`,
        hint:
          source.error ||
          (source.sourceType === 'mcp' && source.tools.length === 0
            ? t('settings.tools.mcp_lazy_hint')
            : ''),
        actionError: snapshotState.actionError,
      })),
    [selection.activeSource, selection.categorySources, snapshotState.actionError, t]
  );

  const activeSource = useMemo(() => {
    if (!selection.activeSource) return null;
    return (
      sources.find((source) => sourceIdentityKey(source) === sourceIdentityKey(selection.activeSource!)) ||
      null
    );
  }, [selection.activeSource, sources]);

  const tools = useMemo(
    () =>
      selection.filteredTools.map((tool) => ({
        ...tool,
        activeToolId: selection.selectedTool?.id || '',
        schemaProperties: schemaProperties(tool),
        toolSavingKey: selection.activeSource
          ? `tool:${selection.activeSource.sourceType}:${selection.activeSource.sourceId}:${tool.id}:enabled`
          : '',
      })),
    [selection.activeSource, selection.filteredTools, selection.selectedTool]
  );

  const selectedTool = useMemo(() => {
    if (!selection.selectedTool) return null;
    return (
      tools.find((tool) => tool.id === selection.selectedTool?.id) || null
    );
  }, [selection.selectedTool, tools]);

  const dataContext: SchemaRendererDataContext = {
    getDataSource: (dataSource) => {
      if (dataSource === 'toolManager' || dataSource === 'toolManager.root') {
        return snapshotState.snapshot;
      }
      if (dataSource === 'toolManager.categories') {
        return TOOL_MANAGER_CATEGORIES;
      }
      if (dataSource === 'toolManager.sources') {
        return sources;
      }
      if (dataSource === 'toolManager.activeSource') {
        return activeSource || {};
      }
      if (dataSource === 'toolManager.tools') {
        return tools;
      }
      if (dataSource === 'toolManager.selectedTool') {
        return selectedTool || {};
      }
      if (dataSource === 'toolManager.query') {
        return {
          activeTab: selection.activeCategory,
          search: selection.search,
          selectedItem: selection.selectedTool?.id || '',
        };
      }
      return undefined;
    },
    dispatch: async (action, payload) => {
      const record = asRecord(payload);
      const item = asRecord(record.item);
      if (action === 'selectCategory') {
        selection.selectCategory(String(record.value || item.id || 'native') as ToolCategory);
        return;
      }
      if (action === 'selectSource') {
        const source = item as unknown as ToolManagerSourceSnapshot;
        selection.selectSource(source);
        snapshotState.setActionError('');
        if (source.sourceType === 'mcp' && !snapshotState.discoveredSourceIds.has(source.sourceId)) {
          await snapshotState.discoverSource(source.sourceId);
        }
        return;
      }
      if (action === 'selectTool') {
        selection.setSelectedToolId(String(record.value || item.id || ''));
        return;
      }
      if (action === 'setSearch') {
        selection.setSearch(String(record.value || ''));
        return;
      }
      if (action === 'discoverSource') {
        if (selection.activeSource?.sourceType === 'mcp') {
          await snapshotState.discoverSource(selection.activeSource.sourceId);
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
      if (action === 'toggleToolEnabled' && selection.activeSource) {
        await savePolicy(
          typeof record.path === 'string'
            ? record.path
            : toolPolicyEnabledPath(
                selection.activeSource,
                item as unknown as ToolManagerToolSnapshot
              ),
          !!record.value,
          String(item.toolSavingKey || ''),
          selection.activeSource
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

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex min-h-0 flex-1 flex-col">
        {snapshotState.loading ? (
          <div className="flex h-48 items-center justify-center gap-2 text-sm text-neutral-400">
            <LoaderCircle className="h-4 w-4 animate-spin" />
            {t('settings.tools.loading')}
          </div>
        ) : snapshotState.loadError ? (
          <div className="flex h-48 items-center justify-center gap-2 px-4 text-sm text-rose-300">
            <AlertCircle className="h-4 w-4 shrink-0" />
            <span>{t('settings.tools.error', { message: snapshotState.loadError })}</span>
          </div>
        ) : (
          <SchemaRenderer
            nodes={nodes}
            snapshot={settingsSnapshot}
            savingPath={savingPath}
            fieldErrors={fieldErrors}
            valueSource="toolManager"
            queryState={{
              activeTab: selection.activeCategory,
              search: selection.search,
              selectedItem: selection.selectedTool?.id,
            }}
            actions={{
              saveField: onSaveField,
              discover: (sourceId) =>
                sourceId ? snapshotState.discoverSource(sourceId) : Promise.resolve(),
              refresh: async () => {
                await snapshotState.refreshSnapshot();
              },
              togglePolicy: (path, enabled) => onSaveField(path, enabled),
              setExposure: (path, exposure) => onSaveField(path, exposure),
            }}
            dataContext={dataContext}
          />
        )}
      </div>
    </div>
  );
}
