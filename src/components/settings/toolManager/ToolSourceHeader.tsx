import { LoaderCircle, RefreshCcw, Search } from 'lucide-react';
import { useI18n } from '../../../features/i18n';
import {
  isMcpExposureEditable,
  isSourcePolicyEditable,
  type McpExposureMode,
  type ToolManagerSourceSnapshot,
} from '../../../features/settings/toolManagerView';
import { ToolPolicySwitch } from './ToolPolicyControls';

const normalizedExposure = (source: ToolManagerSourceSnapshot): McpExposureMode =>
  source.exposureMode === 'unfolded' ? 'unfolded' : 'collapsed';

export function ToolSourceHeader({
  activeSource,
  search,
  actionError,
  discoveringSourceId,
  savingKeys,
  onSearchChange,
  onDiscoverSource,
  onSaveSourceEnabled,
  onSaveMcpExposure,
}: {
  activeSource: ToolManagerSourceSnapshot;
  search: string;
  actionError: string;
  discoveringSourceId: string;
  savingKeys: Set<string>;
  onSearchChange: (search: string) => void;
  onDiscoverSource: (sourceId: string) => void;
  onSaveSourceEnabled: (source: ToolManagerSourceSnapshot, enabled: boolean) => void;
  onSaveMcpExposure: (source: ToolManagerSourceSnapshot, exposure: McpExposureMode) => void;
}) {
  const { t } = useI18n();
  const sourceSavingKey = `source:${activeSource.sourceType}:${activeSource.sourceId}:enabled`;
  const exposureSavingKey = `source:${activeSource.sourceType}:${activeSource.sourceId}:exposure`;

  return (
    <div className="flex flex-wrap items-center justify-between gap-3 border-b border-[#242426]/50 px-3 py-2">
      <div className="min-w-0">
        <div className="flex items-center gap-2">
          <h5 className="truncate text-[13px] font-medium text-neutral-100">
            {activeSource.sourceName}
          </h5>
          {isSourcePolicyEditable(activeSource) ? (
            <ToolPolicySwitch
              checked={activeSource.enabled}
              loading={savingKeys.has(sourceSavingKey)}
              disabled={savingKeys.has(sourceSavingKey)}
              title={
                activeSource.enabled
                  ? t('settings.tools.disable_source')
                  : t('settings.tools.enable_source')
              }
              onChange={(nextEnabled) => onSaveSourceEnabled(activeSource, nextEnabled)}
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
          <p className="mt-0.5 truncate text-[11px] text-rose-300">{activeSource.error}</p>
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

      <div className="flex min-w-0 shrink-0 flex-wrap items-center gap-2">
        {isMcpExposureEditable(activeSource) && (
          <div className="inline-flex h-7 overflow-hidden rounded-md border border-[#2b2b2d] bg-[#1a1b1d]/40">
            {(['collapsed', 'unfolded'] as McpExposureMode[]).map((mode) => {
              const saving = savingKeys.has(exposureSavingKey);
              const selected = normalizedExposure(activeSource) === mode;
              return (
                <button
                  key={mode}
                  type="button"
                  disabled={saving || selected}
                  onClick={() => onSaveMcpExposure(activeSource, mode)}
                  className={`px-2.5 text-[11px] capitalize transition ${
                    selected
                      ? 'bg-[#2a2a2c] text-white'
                      : 'text-neutral-400 hover:bg-[#202022] hover:text-white'
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
            onChange={(event) => onSearchChange(event.target.value)}
            placeholder={t('settings.tools.search')}
            className="h-7 w-48 rounded-md border border-[#2b2b2d] bg-[#1a1b1d]/40 pl-7 pr-2 text-[12px] text-neutral-200 outline-none transition placeholder:text-neutral-600 focus:border-neutral-500"
          />
        </div>
        {activeSource.sourceType === 'mcp' && (
          <button
            type="button"
            onClick={() => onDiscoverSource(activeSource.sourceId)}
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
  );
}
