import { Boxes, Plug, RefreshCcw, Server } from 'lucide-react';
import { useI18n } from '../../../features/i18n';
import {
  isSourcePolicyEditable,
  sourceIdentityKey,
  type ToolManagerSourceSnapshot,
  type ToolSourceType,
} from '../../../features/settings/toolManagerView';
import { OverlayScrollArea } from '../../OverlayScrollArea';
import { ToolPolicySwitch } from './ToolPolicyControls';

const sourceIcon = (sourceType: ToolSourceType) => {
  if (sourceType === 'mcp' || sourceType === 'control') return Server;
  if (sourceType === 'plugin') return Plug;
  if (sourceType === 'background') return RefreshCcw;
  return Boxes;
};

export function ToolSourceList({
  sources,
  activeSource,
  savingKeys,
  onSelectSource,
  onSaveSourceEnabled,
}: {
  sources: ToolManagerSourceSnapshot[];
  activeSource: ToolManagerSourceSnapshot | null;
  savingKeys: Set<string>;
  onSelectSource: (source: ToolManagerSourceSnapshot) => void;
  onSaveSourceEnabled: (source: ToolManagerSourceSnapshot, enabled: boolean) => void;
}) {
  const { t } = useI18n();

  return (
    <aside className="border-r border-[#242426] bg-[#141516]">
      <OverlayScrollArea containerClassName="h-[min(52vh,520px)] xl:h-full" className="h-full p-2">
        {sources.length === 0 ? (
          <p className="px-2 py-6 text-center text-xs text-neutral-500">
            {t('settings.tools.empty_sources')}
          </p>
        ) : (
          <div className="space-y-1">
            {sources.map((source) => {
              const Icon = sourceIcon(source.sourceType);
              const active =
                activeSource && sourceIdentityKey(activeSource) === sourceIdentityKey(source);
              const savingKey = `source:${source.sourceType}:${source.sourceId}:enabled`;
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
                    onClick={() => onSelectSource(source)}
                    className="flex min-w-0 flex-1 items-center gap-2 text-left"
                  >
                    <Icon className="h-3.5 w-3.5 shrink-0" />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-[12.5px]">{source.sourceName}</span>
                      <span className="mt-0.5 block truncate text-[10px] text-neutral-500">
                        {source.status} · {source.exposureMode} · {source.tools.length}
                      </span>
                    </span>
                  </button>
                  {isSourcePolicyEditable(source) ? (
                    <ToolPolicySwitch
                      checked={source.enabled}
                      loading={savingKeys.has(savingKey)}
                      disabled={savingKeys.has(savingKey)}
                      title={
                        source.enabled
                          ? t('settings.tools.disable_source')
                          : t('settings.tools.enable_source')
                      }
                      onChange={(nextEnabled) => onSaveSourceEnabled(source, nextEnabled)}
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
  );
}
