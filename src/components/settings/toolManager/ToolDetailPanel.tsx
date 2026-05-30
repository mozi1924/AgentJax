import { useI18n } from '../../../features/i18n';
import type {
  ToolManagerSourceSnapshot,
  ToolManagerToolSnapshot,
} from '../../../features/settings/toolManagerView';
import { OverlayScrollArea } from '../../OverlayScrollArea';
import { ToolPolicyControls } from './ToolPolicyControls';
import { ToolSchemaPreview } from './ToolSchemaPreview';

export function ToolDetailPanel({
  source,
  tool,
  savingKeys,
  onSaveToolEnabled,
}: {
  source: ToolManagerSourceSnapshot | null;
  tool: ToolManagerToolSnapshot | null;
  savingKeys: Set<string>;
  onSaveToolEnabled: (
    source: ToolManagerSourceSnapshot,
    tool: ToolManagerToolSnapshot,
    enabled: boolean
  ) => void;
}) {
  const { t } = useI18n();

  if (!source || !tool) {
    return (
      <section className="min-w-0 border-t border-[#242426]/50 bg-[#171719]/20 xl:border-l xl:border-t-0">
        <div className="flex h-40 items-center justify-center text-xs text-neutral-500 xl:h-full">
          {t('settings.tools.empty_tools')}
        </div>
      </section>
    );
  }

  const savingKey = `tool:${source.sourceType}:${source.sourceId}:${tool.id}:enabled`;

  return (
    <section className="min-w-0 border-t border-[#242426]/50 bg-[#171719]/20 xl:border-l xl:border-t-0">
      <OverlayScrollArea containerClassName="h-[min(52vh,520px)] xl:h-full" className="h-full p-3">
        <div className="space-y-3">
          <div>
            <div className="flex flex-wrap items-center gap-2">
              <h5 className="min-w-0 flex-1 truncate text-[13px] font-medium text-neutral-100">
                {tool.friendlyName}
              </h5>
              <span className="rounded bg-[#2a2a2c] px-1.5 py-0.5 text-[10px] text-neutral-400">
                {tool.schemaFormat || 'json_schema'}
              </span>
            </div>
            <p className="mt-1 text-[11.5px] leading-relaxed text-neutral-400">
              {tool.description || tool.id}
            </p>
            <div className="mt-2 flex flex-wrap items-center gap-2 text-[10.5px] text-neutral-500">
              <span className="font-mono text-neutral-300">{tool.id}</span>
              <span className="font-mono">{tool.modelName}</span>
              <span>{tool.availability}</span>
            </div>
          </div>

          <div className="rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 px-3 py-2">
            <div className="flex items-center justify-between gap-3">
              <span className="text-[12px] font-medium text-neutral-200">
                {t('settings.tools.policy')}
              </span>
              <ToolPolicyControls
                source={source}
                tool={tool}
                saving={savingKeys.has(savingKey)}
                onSaveToolEnabled={onSaveToolEnabled}
              />
            </div>
          </div>

          <div>
            <h6 className="mb-2 text-[11px] font-semibold uppercase tracking-wider text-neutral-500">
              {t('settings.tools.parameters')}
            </h6>
            <ToolSchemaPreview tool={tool} />
          </div>
        </div>
      </OverlayScrollArea>
    </section>
  );
}
