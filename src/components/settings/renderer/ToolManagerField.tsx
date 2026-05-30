import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  AlertCircle,
  Boxes,
  LoaderCircle,
  Plug,
  RefreshCcw,
  Search,
  Server,
  Wrench,
} from 'lucide-react';
import { useI18n } from '../../../features/i18n';
import type { SettingsSnapshot } from '../../../features/settings/types';
import {
  TOOL_MANAGER_CATEGORIES,
  filterToolsForQuery,
  isMcpExposureEditable,
  isSourcePolicyEditable,
  isToolPolicyEditable,
  mcpExposurePolicyPath,
  selectToolManagerSource,
  sourcePolicyEnabledPath,
  sourcesForCategory,
  toolPolicyEnabledPath,
  type McpExposureMode,
  type ToolManagerSnapshot,
  type ToolManagerSourceSnapshot,
  type ToolManagerToolSnapshot,
  type ToolCategory,
  type ToolSourceType,
} from '../../../features/settings/toolManagerView';
import { OverlayScrollArea } from '../../OverlayScrollArea';
import type { FieldRendererProps } from './types';

const sourceIcon = (sourceType: ToolSourceType) => {
  if (sourceType === 'mcp' || sourceType === 'control') return Server;
  if (sourceType === 'plugin') return Plug;
  if (sourceType === 'background') return RefreshCcw;
  return Boxes;
};

const SwitchControl = ({
  checked,
  disabled,
  loading,
  title,
  onChange,
}: {
  checked: boolean;
  disabled?: boolean;
  loading?: boolean;
  title: string;
  onChange: (checked: boolean) => void;
}) => (
  <label
    className={`relative inline-flex h-5 w-9 items-center ${
      disabled ? 'cursor-not-allowed opacity-60' : 'cursor-pointer'
    }`}
    title={title}
  >
    <input
      type="checkbox"
      checked={checked}
      disabled={disabled}
      onChange={(event) => onChange(event.target.checked)}
      className="peer sr-only"
    />
    <span className="absolute inset-0 rounded-full bg-[#3e3e42] transition peer-checked:bg-cyan-500" />
    <span className="absolute left-0.5 top-0.5 flex h-4 w-4 items-center justify-center rounded-full bg-white transition-transform duration-200 peer-checked:translate-x-4">
      {loading && <LoaderCircle className="h-2.5 w-2.5 animate-spin text-neutral-700" />}
    </span>
  </label>
);

const normalizedExposure = (source: ToolManagerSourceSnapshot): McpExposureMode =>
  source.exposureMode === 'unfolded' ? 'unfolded' : 'collapsed';

export function ToolManagerField({ field, snapshot: settingsSnapshot }: FieldRendererProps) {
  const { t } = useI18n();
  const [snapshot, setSnapshot] = useState<ToolManagerSnapshot | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState('');
  const [actionError, setActionError] = useState('');
  const [activeCategory, setActiveCategory] = useState<ToolCategory>('native');
  const [selectedSourceId, setSelectedSourceId] = useState('');
  const [search, setSearch] = useState('');
  const [discoveringSourceId, setDiscoveringSourceId] = useState('');
  const [savingKeys, setSavingKeys] = useState<Set<string>>(new Set());
  const [discoveredSourceIds, setDiscoveredSourceIds] = useState<Set<string>>(new Set());
  const [configRevision, setConfigRevision] = useState(settingsSnapshot.revision);

  useEffect(() => {
    setConfigRevision(settingsSnapshot.revision);
  }, [settingsSnapshot.revision]);

  useEffect(() => {
    let disposed = false;
    setLoading(true);
    setLoadError('');
    invoke<ToolManagerSnapshot>('get_tool_manager_snapshot', { request: null })
      .then((nextSnapshot) => {
        if (disposed) return;
        setSnapshot(nextSnapshot);
      })
      .catch((err) => {
        if (disposed) return;
        setLoadError(typeof err === 'string' ? err : String(err));
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, []);

  const categorySources = useMemo(() => {
    const sources = snapshot?.sources || [];
    return sourcesForCategory(sources, activeCategory);
  }, [activeCategory, snapshot]);

  const activeSource = useMemo(() => {
    return selectToolManagerSource(categorySources, selectedSourceId);
  }, [categorySources, selectedSourceId]);

  const filteredTools = useMemo(() => {
    return filterToolsForQuery(activeSource?.tools || [], search);
  }, [activeSource, search]);

  const selectSource = (source: ToolManagerSourceSnapshot) => {
    setSelectedSourceId(source.sourceId);
    setSearch('');
    setActionError('');
    if (source.sourceType === 'mcp' && !discoveredSourceIds.has(source.sourceId)) {
      void discoverSource(source.sourceId);
    }
  };

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
    setDiscoveringSourceId(sourceId);
    setActionError('');
    try {
      await refreshSnapshot({ discoverSourceId: sourceId });
    } catch (err) {
      setActionError(typeof err === 'string' ? err : String(err));
    } finally {
      setDiscoveringSourceId('');
    }
  };

  const refreshAfterSave = async () => {
    const shouldRediscover =
      activeSource?.sourceType === 'mcp' && discoveredSourceIds.has(activeSource.sourceId);
    await refreshSnapshot({
      discoverSourceId: shouldRediscover ? activeSource.sourceId : undefined,
    });
  };

  const savePolicy = async (path: string | null, value: unknown, savingKey: string) => {
    if (!path) return;
    setSavingKeys((current) => new Set(current).add(savingKey));
    setActionError('');
    try {
      const nextSettingsSnapshot = await invoke<SettingsSnapshot>('apply_settings_patch', {
        patch: {
          path,
          value,
          expectedRevision: configRevision,
          operation: 'set',
        },
      });
      setConfigRevision(nextSettingsSnapshot.revision);
      await refreshAfterSave();
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

  const saveSourceEnabled = (source: ToolManagerSourceSnapshot, enabled: boolean) =>
    savePolicy(
      sourcePolicyEnabledPath(source),
      enabled,
      `source:${source.sourceType}:${source.sourceId}:enabled`
    );

  const saveToolEnabled = (
    source: ToolManagerSourceSnapshot,
    tool: ToolManagerToolSnapshot,
    enabled: boolean
  ) =>
    savePolicy(
      toolPolicyEnabledPath(source, tool),
      enabled,
      `tool:${source.sourceType}:${source.sourceId}:${tool.id}:enabled`
    );

  const saveMcpExposure = (source: ToolManagerSourceSnapshot, exposure: McpExposureMode) =>
    savePolicy(
      mcpExposurePolicyPath(source),
      exposure,
      `source:${source.sourceType}:${source.sourceId}:exposure`
    );

  return (
    <div className="border-b border-[#242426]/30 py-3 first:pt-0 last:border-b-0">
      <div className="mb-3">
        <div className="flex items-center gap-2">
          <Wrench className="h-4 w-4 text-cyan-300" />
          <h4 className="text-[13.5px] font-medium text-neutral-200">{t(field.title)}</h4>
        </div>
        {field.description && (
          <p className="mt-0.5 text-[11.5px] leading-relaxed text-neutral-400/80">
            {t(field.description)}
          </p>
        )}
      </div>

      <div className="overflow-hidden rounded-lg border border-[#27282b] bg-[#101112]">
        <div className="flex items-center gap-1 border-b border-[#242426] px-3 py-2">
          {TOOL_MANAGER_CATEGORIES.map((category) => (
            <button
              key={category.id}
              type="button"
              onClick={() => {
                setActiveCategory(category.id);
                setSelectedSourceId('');
                setSearch('');
              }}
              className={`rounded-md px-2.5 py-1 text-[12px] transition ${
                activeCategory === category.id
                  ? 'bg-cyan-500/15 text-cyan-100'
                  : 'text-neutral-400 hover:bg-[#202124] hover:text-neutral-200'
              }`}
            >
              {t(category.labelKey)}
            </button>
          ))}
        </div>

        {loading ? (
          <div className="flex h-48 items-center justify-center gap-2 text-sm text-neutral-400">
            <LoaderCircle className="h-4 w-4 animate-spin" />
            {t('settings.tools.loading')}
          </div>
        ) : loadError ? (
          <div className="flex h-48 items-center justify-center gap-2 px-4 text-sm text-rose-300">
            <AlertCircle className="h-4 w-4 shrink-0" />
            <span>{t('settings.tools.error', { message: loadError })}</span>
          </div>
        ) : (
          <div className="grid min-h-[360px] grid-cols-[240px_1fr]">
            <aside className="border-r border-[#242426] bg-[#141516]">
              <OverlayScrollArea containerClassName="h-[360px]" className="h-full p-2">
                {categorySources.length === 0 ? (
                  <p className="px-2 py-6 text-center text-xs text-neutral-500">
                    {t('settings.tools.empty_sources')}
                  </p>
                ) : (
                  <div className="space-y-1">
                    {categorySources.map((source) => {
                      const Icon = sourceIcon(source.sourceType);
                      const active = activeSource?.sourceId === source.sourceId;
                      return (
                        <div
                          key={`${source.sourceType}-${source.sourceId}`}
                          className={`flex w-full items-center gap-2 rounded-md px-2.5 py-2 text-left transition ${
                            active
                              ? 'bg-[#25272a] text-white'
                              : 'text-neutral-400 hover:bg-[#202124] hover:text-neutral-200'
                          }`}
                        >
                          <button
                            type="button"
                            onClick={() => selectSource(source)}
                            className="flex min-w-0 flex-1 items-center gap-2 text-left"
                          >
                            <Icon className="h-3.5 w-3.5 shrink-0" />
                            <span className="min-w-0 flex-1">
                              <span className="block truncate text-[12.5px]">
                                {source.sourceName}
                              </span>
                              <span className="mt-0.5 block truncate text-[10px] text-neutral-500">
                                {source.status} · {source.exposureMode}
                              </span>
                            </span>
                          </button>
                          {isSourcePolicyEditable(source) ? (
                            <SwitchControl
                              checked={source.enabled}
                              loading={savingKeys.has(
                                `source:${source.sourceType}:${source.sourceId}:enabled`
                              )}
                              disabled={savingKeys.has(
                                `source:${source.sourceType}:${source.sourceId}:enabled`
                              )}
                              title={
                                source.enabled
                                  ? t('settings.tools.disable_source')
                                  : t('settings.tools.enable_source')
                              }
                              onChange={(nextEnabled) => saveSourceEnabled(source, nextEnabled)}
                            />
                          ) : !source.enabled ? (
                            <span className="rounded bg-[#3a2328] px-1.5 py-0.5 text-[9px] text-rose-200">
                              off
                            </span>
                          ) : null}
                        </div>
                      );
                    })}
                  </div>
                )}
              </OverlayScrollArea>
            </aside>

            <section className="min-w-0">
              {activeSource ? (
                <div className="flex h-full flex-col">
                  <div className="flex items-center justify-between gap-3 border-b border-[#242426] px-3 py-2">
                    <div className="min-w-0">
                      <div className="flex items-center gap-2">
                        <h5 className="truncate text-[13px] font-medium text-neutral-100">
                          {activeSource.sourceName}
                        </h5>
                        {isSourcePolicyEditable(activeSource) ? (
                          <SwitchControl
                            checked={activeSource.enabled}
                            loading={savingKeys.has(
                              `source:${activeSource.sourceType}:${activeSource.sourceId}:enabled`
                            )}
                            disabled={savingKeys.has(
                              `source:${activeSource.sourceType}:${activeSource.sourceId}:enabled`
                            )}
                            title={
                              activeSource.enabled
                                ? t('settings.tools.disable_source')
                                : t('settings.tools.enable_source')
                            }
                            onChange={(nextEnabled) =>
                              saveSourceEnabled(activeSource, nextEnabled)
                            }
                          />
                        ) : (
                          <span
                            className={`rounded px-1.5 py-0.5 text-[10px] ${
                              activeSource.enabled
                                ? 'bg-emerald-500/10 text-emerald-200'
                                : 'bg-rose-500/10 text-rose-200'
                            }`}
                          >
                            {activeSource.enabled
                              ? t('settings.tools.status.enabled')
                              : t('settings.tools.status.disabled')}
                          </span>
                        )}
                      </div>
                      {activeSource.error ? (
                        <p className="mt-0.5 truncate text-[11px] text-rose-300">
                          {activeSource.error}
                        </p>
                      ) : activeSource.sourceType === 'mcp' && activeSource.tools.length === 0 ? (
                        <p className="mt-0.5 text-[11px] text-neutral-500">
                          {t('settings.tools.mcp_lazy_hint')}
                        </p>
                      ) : null}
                      {actionError && (
                        <p className="mt-0.5 truncate text-[11px] text-rose-300">
                          {t('settings.tools.error', { message: actionError })}
                        </p>
                      )}
                    </div>

                    <div className="flex shrink-0 items-center gap-2">
                      {isMcpExposureEditable(activeSource) && (
                        <div className="inline-flex h-7 overflow-hidden rounded-md border border-[#2b2c30] bg-[#111214]">
                          {(['collapsed', 'unfolded'] as McpExposureMode[]).map((mode) => {
                            const saving = savingKeys.has(
                              `source:${activeSource.sourceType}:${activeSource.sourceId}:exposure`
                            );
                            const selected = normalizedExposure(activeSource) === mode;
                            return (
                              <button
                                key={mode}
                                type="button"
                                disabled={saving || selected}
                                onClick={() => saveMcpExposure(activeSource, mode)}
                                className={`px-2.5 text-[11px] capitalize transition ${
                                  selected
                                    ? 'bg-cyan-500/15 text-cyan-100'
                                    : 'text-neutral-400 hover:bg-[#202124] hover:text-neutral-200'
                                } disabled:cursor-default disabled:opacity-70`}
                              >
                                {saving && !selected ? (
                                  <LoaderCircle className="mx-2 h-3 w-3 animate-spin" />
                                ) : (
                                  mode
                                )}
                              </button>
                            );
                          })}
                        </div>
                      )}
                      <div className="relative">
                        <Search className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-neutral-500" />
                        <input
                          value={search}
                          onChange={(event) => setSearch(event.target.value)}
                          placeholder={t('settings.tools.search')}
                          className="h-7 w-48 rounded-md border border-[#2b2c30] bg-[#111214] pl-7 pr-2 text-[12px] text-neutral-200 outline-none transition placeholder:text-neutral-600 focus:border-cyan-500/50"
                        />
                      </div>
                      {activeSource.sourceType === 'mcp' && (
                        <button
                          type="button"
                          onClick={() => discoverSource(activeSource.sourceId)}
                          disabled={discoveringSourceId === activeSource.sourceId}
                          className="inline-flex h-7 items-center gap-1.5 rounded-md border border-[#2b2c30] px-2 text-[12px] text-neutral-300 transition hover:bg-[#202124] disabled:opacity-60"
                        >
                          {discoveringSourceId === activeSource.sourceId ? (
                            <LoaderCircle className="h-3.5 w-3.5 animate-spin" />
                          ) : (
                            <RefreshCcw className="h-3.5 w-3.5" />
                          )}
                          {discoveringSourceId === activeSource.sourceId
                            ? t('settings.tools.discovering')
                            : t('settings.tools.discover')}
                        </button>
                      )}
                    </div>
                  </div>

                  <OverlayScrollArea containerClassName="h-[305px]" className="h-full p-3">
                    {filteredTools.length === 0 ? (
                      <div className="flex h-48 items-center justify-center text-xs text-neutral-500">
                        {t('settings.tools.empty_tools')}
                      </div>
                    ) : (
                      <div className="space-y-2">
                        {filteredTools.map((tool) => (
                          <div
                            key={`${activeSource.sourceId}-${tool.modelName}`}
                            className="rounded-lg border border-[#26272b] bg-[#151618] px-3 py-2"
                          >
                            <div className="flex items-start justify-between gap-3">
                              <div className="min-w-0">
                                <div className="flex items-center gap-2">
                                  <h6 className="truncate text-[13px] font-medium text-neutral-100">
                                    {tool.friendlyName}
                                  </h6>
                                  <span className="rounded bg-[#24262a] px-1.5 py-0.5 text-[10px] text-neutral-400">
                                    {tool.availability}
                                  </span>
                                </div>
                                <p className="mt-1 line-clamp-2 text-[11.5px] leading-relaxed text-neutral-400">
                                  {tool.description || tool.id}
                                </p>
                              </div>
                              {isToolPolicyEditable(activeSource) ? (
                                <SwitchControl
                                  checked={tool.enabled}
                                  loading={savingKeys.has(
                                    `tool:${activeSource.sourceType}:${activeSource.sourceId}:${tool.id}:enabled`
                                  )}
                                  disabled={savingKeys.has(
                                    `tool:${activeSource.sourceType}:${activeSource.sourceId}:${tool.id}:enabled`
                                  )}
                                  title={
                                    tool.enabled
                                      ? t('settings.tools.disable_tool')
                                      : t('settings.tools.enable_tool')
                                  }
                                  onChange={(nextEnabled) =>
                                    saveToolEnabled(activeSource, tool, nextEnabled)
                                  }
                                />
                              ) : (
                                <span
                                  className={`shrink-0 rounded px-1.5 py-0.5 text-[10px] ${
                                    tool.enabled
                                      ? 'bg-emerald-500/10 text-emerald-200'
                                      : 'bg-rose-500/10 text-rose-200'
                                  }`}
                                >
                                  {tool.enabled
                                    ? t('settings.tools.status.enabled')
                                    : t('settings.tools.status.disabled')}
                                </span>
                              )}
                            </div>
                            <div className="mt-2 flex flex-wrap items-center gap-2 text-[10.5px] text-neutral-500">
                              <span className="font-mono text-neutral-400">{tool.id}</span>
                              <span className="font-mono">{tool.modelName}</span>
                              <span>
                                {t('settings.tools.schema_summary', {
                                  count: String(tool.schemaSummary.parameterCount),
                                })}
                              </span>
                            </div>
                          </div>
                        ))}
                      </div>
                    )}
                  </OverlayScrollArea>
                </div>
              ) : (
                <div className="flex h-full items-center justify-center text-xs text-neutral-500">
                  {t('settings.tools.empty_sources')}
                </div>
              )}
            </section>
          </div>
        )}
      </div>
    </div>
  );
}
