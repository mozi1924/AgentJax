import { useI18n } from '../../../features/i18n';
import type {
  ToolManagerSourceSnapshot,
  ToolManagerToolSnapshot,
} from '../../../features/settings/toolManagerView';
import { OverlayScrollArea } from '../../OverlayScrollArea';
import { ToolPolicyControls } from './ToolPolicyControls';

export function ToolList({
  source,
  tools,
  selectedTool,
  savingKeys,
  onSelectTool,
  onSaveToolEnabled,
}: {
  source: ToolManagerSourceSnapshot;
  tools: ToolManagerToolSnapshot[];
  selectedTool: ToolManagerToolSnapshot | null;
  savingKeys: Set<string>;
  onSelectTool: (toolId: string) => void;
  onSaveToolEnabled: (
    source: ToolManagerSourceSnapshot,
    tool: ToolManagerToolSnapshot,
    enabled: boolean
  ) => void;
}) {
  const { t } = useI18n();

  return (
    <OverlayScrollArea containerClassName="h-[min(46vh,480px)] xl:h-full" className="h-full p-3">
      {tools.length === 0 ? (
        <div className="flex h-48 items-center justify-center text-xs text-neutral-500">
          {t('settings.tools.empty_tools')}
        </div>
      ) : (
        <div className="space-y-2">
          {tools.map((tool) => {
            const activeTool = selectedTool?.id === tool.id;
            const savingKey = `tool:${source.sourceType}:${source.sourceId}:${tool.id}:enabled`;
            return (
              <div
                key={`${source.sourceId}-${tool.modelName}`}
                onClick={() => onSelectTool(tool.id)}
                className={`w-full rounded-lg border px-3 py-2 text-left transition cursor-pointer select-none ${
                  activeTool
                    ? 'border-neutral-500 bg-[#2a2a2c]/60'
                    : 'border-[#2b2b2d] bg-[#1a1b1d]/40 hover:border-neutral-500'
                }`}
              >
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <h6 className="truncate text-[13px] font-medium text-neutral-100">
                        {tool.friendlyName}
                      </h6>
                      <span className="rounded bg-[#2a2a2c] px-1.5 py-0.5 text-[10px] text-neutral-400">
                        {tool.availability}
                      </span>
                    </div>
                    <p className="mt-1 line-clamp-2 text-[11.5px] leading-relaxed text-neutral-400">
                      {tool.description || tool.id}
                    </p>
                  </div>
                  <ToolPolicyControls
                    source={source}
                    tool={tool}
                    saving={savingKeys.has(savingKey)}
                    onSaveToolEnabled={onSaveToolEnabled}
                  />
                </div>
                <div className="mt-2 flex flex-wrap items-center gap-2 text-[10.5px] text-neutral-500">
                  <span className="font-mono text-neutral-300">{tool.id}</span>
                  <span className="font-mono">{tool.modelName}</span>
                  <span>
                    {t('settings.tools.schema_summary', {
                      count: String(tool.schemaSummary.parameterCount),
                    })}
                  </span>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </OverlayScrollArea>
  );
}
