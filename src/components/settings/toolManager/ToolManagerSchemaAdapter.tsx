import { useState } from 'react';
import { AlertCircle, LoaderCircle, Wrench } from 'lucide-react';
import type { ReactElement } from 'react';
import { useI18n } from '../../../features/i18n';
import type { SettingsSchemaNode, SettingsSnapshot, SettingsUiSchemaNode } from '../../../features/settings/types';
import {
  mcpExposurePolicyPath,
  sourcePolicyEnabledPath,
  toolPolicyEnabledPath,
  type McpExposureMode,
  type ToolManagerSourceSnapshot,
  type ToolManagerToolSnapshot,
} from '../../../features/settings/toolManagerView';
import { SchemaRenderer } from '../schemaRenderer';
import { ToolDetailPanel } from './ToolDetailPanel';
import { ToolList } from './ToolList';
import { ToolSourceHeader } from './ToolSourceHeader';
import { ToolSourceList } from './ToolSourceList';
import { ToolSourceTabs } from './ToolSourceTabs';
import { TOOL_MANAGER_UI_SCHEMA } from './toolManagerUiSchema';
import { useToolManagerSelection } from './useToolManagerSelection';
import { useToolManagerSnapshot } from './useToolManagerSnapshot';

export function ToolManagerSchemaAdapter({
  title,
  description,
  nodes = TOOL_MANAGER_UI_SCHEMA,
  snapshot: settingsSnapshot,
  savingPath,
  fieldErrors,
  onSaveField,
}: {
  title?: string;
  description?: string;
  nodes?: SettingsSchemaNode[];
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

  const saveSourceEnabled = (source: ToolManagerSourceSnapshot, enabled: boolean) =>
    savePolicy(
      sourcePolicyEnabledPath(source),
      enabled,
      `source:${source.sourceType}:${source.sourceId}:enabled`,
      source
    );

  const saveToolEnabled = (
    source: ToolManagerSourceSnapshot,
    tool: ToolManagerToolSnapshot,
    enabled: boolean
  ) =>
    savePolicy(
      toolPolicyEnabledPath(source, tool),
      enabled,
      `tool:${source.sourceType}:${source.sourceId}:${tool.id}:enabled`,
      source
    );

  const saveMcpExposure = (source: ToolManagerSourceSnapshot, exposure: McpExposureMode) =>
    savePolicy(
      mcpExposurePolicyPath(source),
      exposure,
      `source:${source.sourceType}:${source.sourceId}:exposure`,
      source
    );

  const selectSource = (source: ToolManagerSourceSnapshot) => {
    selection.selectSource(source);
    snapshotState.setActionError('');
    if (source.sourceType === 'mcp' && !snapshotState.discoveredSourceIds.has(source.sourceId)) {
      void snapshotState.discoverSource(source.sourceId);
    }
  };

  const renderToolManagerNode = ({
    node,
    defaultRender,
    renderChildren,
  }: {
    node: SettingsUiSchemaNode;
    defaultRender: () => ReactElement;
    renderChildren: (nodes: SettingsSchemaNode[], contextPath?: string) => ReactElement;
  }) => {
    if (node.dataSource === 'toolManager' || node.id === 'tool-manager-root' || node.id === 'tools-manager') {
      return <>{node.children ? renderChildren(node.children) : null}</>;
    }

    if (node.id === 'tool-manager-layout') {
      return (
        <div className="grid min-h-[420px] h-auto xl:h-[520px] grid-cols-1 xl:grid-cols-[220px_minmax(260px,0.9fr)_minmax(300px,1.1fr)]">
          {node.children ? node.children.map((child) => {
            if (child.id === 'tool-source-list') {
              return (
                <ToolSourceList
                  key={child.id}
                  sources={selection.categorySources}
                  activeSource={selection.activeSource}
                  savingKeys={savingKeys}
                  onSelectSource={selectSource}
                  onSaveSourceEnabled={saveSourceEnabled}
                />
              );
            }
            if (child.id === 'tool-list-pane') {
              return (
                <section key={child.id} className="min-w-0 border-t border-[#242426]/50 xl:border-t-0">
                  {selection.activeSource ? (
                    <div className="flex h-full flex-col">
                      <ToolSourceHeader
                        activeSource={selection.activeSource}
                        search={selection.search}
                        actionError={snapshotState.actionError}
                        discoveringSourceId={snapshotState.discoveringSourceId}
                        savingKeys={savingKeys}
                        onSearchChange={selection.setSearch}
                        onDiscoverSource={(sourceId) => {
                          void snapshotState.discoverSource(sourceId);
                        }}
                        onSaveSourceEnabled={saveSourceEnabled}
                        onSaveMcpExposure={saveMcpExposure}
                      />
                      <ToolList
                        source={selection.activeSource}
                        tools={selection.filteredTools}
                        selectedTool={selection.selectedTool}
                        savingKeys={savingKeys}
                        onSelectTool={selection.setSelectedToolId}
                        onSaveToolEnabled={saveToolEnabled}
                      />
                    </div>
                  ) : (
                    <div className="flex h-full items-center justify-center text-xs text-neutral-500">
                      {t('settings.tools.empty_sources')}
                    </div>
                  )}
                </section>
              );
            }
            if (child.id === 'tool-detail') {
              return (
                <ToolDetailPanel
                  key={child.id}
                  source={selection.activeSource}
                  tool={selection.selectedTool}
                  savingKeys={savingKeys}
                  onSaveToolEnabled={saveToolEnabled}
                />
              );
            }
            return null;
          }) : null}
        </div>
      );
    }

    if (node.dataSource === 'toolManager.categories') {
      return (
        <ToolSourceTabs
          activeCategory={selection.activeCategory}
          onSelectCategory={selection.selectCategory}
        />
      );
    }

    if (node.dataSource === 'toolManager.sources') {
      return (
        <ToolSourceList
          sources={selection.categorySources}
          activeSource={selection.activeSource}
          savingKeys={savingKeys}
          onSelectSource={selectSource}
          onSaveSourceEnabled={saveSourceEnabled}
        />
      );
    }

    if (node.id === 'tool-list-pane') {
      return (
        <section className="min-w-0 border-t border-[#242426] xl:border-t-0">
          {selection.activeSource ? (
            <div className="flex h-full flex-col">
              {node.children ? renderChildren(node.children) : null}
            </div>
          ) : (
            <div className="flex h-full items-center justify-center text-xs text-neutral-500">
              {t('settings.tools.empty_sources')}
            </div>
          )}
        </section>
      );
    }

    if (node.dataSource === 'toolManager.sourceHeader' && selection.activeSource) {
      return (
        <ToolSourceHeader
          activeSource={selection.activeSource}
          search={selection.search}
          actionError={snapshotState.actionError}
          discoveringSourceId={snapshotState.discoveringSourceId}
          savingKeys={savingKeys}
          onSearchChange={selection.setSearch}
          onDiscoverSource={(sourceId) => {
            void snapshotState.discoverSource(sourceId);
          }}
          onSaveSourceEnabled={saveSourceEnabled}
          onSaveMcpExposure={saveMcpExposure}
        />
      );
    }

    if (node.dataSource === 'toolManager.tools' && selection.activeSource) {
      return (
        <ToolList
          source={selection.activeSource}
          tools={selection.filteredTools}
          selectedTool={selection.selectedTool}
          savingKeys={savingKeys}
          onSelectTool={selection.setSelectedToolId}
          onSaveToolEnabled={saveToolEnabled}
        />
      );
    }

    if (node.dataSource === 'toolManager.selectedTool') {
      return (
        <ToolDetailPanel
          source={selection.activeSource}
          tool={selection.selectedTool}
          savingKeys={savingKeys}
          onSaveToolEnabled={saveToolEnabled}
        />
      );
    }

    return defaultRender();
  };

  return (
    <div className="border-b border-[#242426]/30 py-3 first:pt-0 last:border-b-0">
      {(title || description) && (
        <div className="mb-3">
          {title && (
            <div className="flex items-center gap-2">
              <Wrench className="h-4 w-4 text-neutral-300" />
              <h4 className="text-[13.5px] font-medium text-neutral-200">{t(title)}</h4>
            </div>
          )}
          {description && (
            <p className="mt-0.5 text-[11.5px] leading-relaxed text-neutral-400/80">
              {t(description)}
            </p>
          )}
        </div>
      )}

      <div className="overflow-hidden rounded-lg border border-[#2b2b2d] bg-[#171719]/30">
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
            renderUiNode={renderToolManagerNode}
          />
        )}
      </div>
    </div>
  );
}
