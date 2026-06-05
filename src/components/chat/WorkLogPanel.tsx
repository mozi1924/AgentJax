import { useEffect, useState } from 'react';
import {
  CheckCircle2,
  ChevronDown,
  Copy,
  Loader2,
  Sparkles,
} from 'lucide-react';
import type { ToolLine } from '../../features/conversations/types';
import { resolveToolLucideIcon } from '../../features/icons/lucide';
import { renderMarkdown } from './markdownRenderer';
import {
  formatTurnDuration,
  getTurnActivitySummary,
  getTurnDurationMs,
  type ConversationTurn,
} from './transcriptGrouping';
import { useI18n } from '../../features/i18n';
import { OverlayScrollArea } from '../OverlayScrollArea';

interface WorkLogPanelProps {
  isOpen: boolean;
  onToggle: () => void;
  turn: ConversationTurn;
  isWorking?: boolean;
}

const formatToolOutput = (val: unknown): string => {
  if (!val) return '';
  try {
    const parsed = typeof val === 'string' ? JSON.parse(val) : val;
    return JSON.stringify(parsed, null, 2);
  } catch {
    return String(val);
  }
};

const humanizeToolName = (name: string) =>
  name
    .split(/[_.-]+/g)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(' ');

const resolveToolDisplayName = (toolLine: ToolLine) => {
  const explicitDisplayName = (toolLine.displayName || '').trim();
  if (explicitDisplayName) {
    return explicitDisplayName;
  }

  if (toolLine.name.startsWith('mcp__')) {
    const parts = toolLine.name.split('__');
    if (parts.length >= 3) {
      return humanizeToolName(parts.slice(2).join('_'));
    }
  }

  return humanizeToolName(toolLine.name || 'tool');
};

const resolveToolOriginLabel = (toolLine: ToolLine, t: (key: string) => string) => {
  if (toolLine.name.startsWith('mcp__')) {
    const parts = toolLine.name.split('__');
    return parts.length >= 2 ? `MCP: ${parts[1]}` : 'MCP';
  }
  if (toolLine.name.startsWith('mcp_server__')) {
    return 'MCP';
  }
  return t('settings.tools.category.native');
};

const resolveToolAccent = (toolLine: ToolLine) => {
  const isMcpTool =
    toolLine.name.startsWith('mcp__') || toolLine.name.startsWith('mcp_server__');
  return {
    icon: resolveToolLucideIcon(toolLine.name, toolLine.icon),
    badge: isMcpTool ? 'MCP' : 'Tool',
    className: 'border-[#26292e] bg-[#17181c] text-slate-200',
    iconClassName: isMcpTool ? 'text-cyan-300/90' : 'text-slate-400',
  };
};

const getToolDurationLabel = (toolLine: ToolLine) => {
  if (!toolLine.startedTs || !toolLine.completedTs) {
    return '';
  }
  const durationMs = Math.max(0, toolLine.completedTs - toolLine.startedTs);
  if (durationMs < 1000) {
    return `${durationMs}ms`;
  }
  return formatTurnDuration(durationMs);
};

const getLocalizedSummaryLabel = (
  summary: { kind: 'search' | 'edit' | 'tool' | 'update'; count: number; label: string },
  t: (key: string, replacements?: Record<string, string>) => string
) => {
  const isOne = summary.count === 1;
  const key = `chat.activity.${summary.kind}_${isOne ? 'one' : 'many'}`;
  return t(key, { count: String(summary.count) });
};

/**
 * Keeps intermediate commentary and tool activity inside one collapsible
 * transcript panel so the final answer can stay visually dominant.
 */
export default function WorkLogPanel({
  isOpen,
  onToggle,
  turn,
  isWorking,
}: WorkLogPanelProps) {
  const { t } = useI18n();
  const [copiedToolId, setCopiedToolId] = useState<string | null>(null);
  const [expandedTools, setExpandedTools] = useState<Set<string>>(new Set());

  // ── Live timer tick for in-progress turns ──────────────────────────────
  const [liveElapsedMs, setLiveElapsedMs] = useState(0);

  const shouldTick = isWorking || (turn.hasDraft && turn.finalLines.length === 0);

  useEffect(() => {
    if (!shouldTick) {
      setLiveElapsedMs(0);
      return;
    }

    const startTs = turn.startedAt;
    const tick = () => setLiveElapsedMs(Date.now() - startTs);
    tick(); // immediate first tick
    const id = window.setInterval(tick, 1000);
    return () => window.clearInterval(id);
  }, [shouldTick, turn.startedAt]);

  const durationLabel = shouldTick
    ? formatTurnDuration(liveElapsedMs)
    : formatTurnDuration(getTurnDurationMs(turn));

  const summaries = getTurnActivitySummary(turn);

  const handleCopyToolPayload = async (toolId: string, value: unknown) => {
    await navigator.clipboard.writeText(formatToolOutput(value));
    setCopiedToolId(toolId);
    window.setTimeout(() => setCopiedToolId(null), 1600);
  };

  const toggleToolExpanded = (callId: string) => {
    setExpandedTools((prev) => {
      const next = new Set(prev);
      if (next.has(callId)) {
        next.delete(callId);
      } else {
        next.add(callId);
      }
      return next;
    });
  };

  return (
    <section className="rounded-xl border border-[#25282d] bg-[#141517]/45 shadow-[0_4px_16px_rgba(0,0,0,0.25)] transition-all duration-200 hover:border-[#2f3238]">
      <button
        type="button"
        className="flex w-full items-center gap-3 px-4 py-2.5 text-left transition hover:bg-white/[0.01]"
        onClick={onToggle}
      >
        <div className="flex items-center gap-2">
          {turn.hasDraft ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin text-slate-300" />
          ) : (
            <Sparkles className="h-3.5 w-3.5 text-slate-400" />
          )}
          <span className="rounded-md border border-zinc-800 bg-[#0d0e0f]/80 px-2 py-0.5 font-mono text-[11px] text-slate-300">
            {t('chat.worked', { duration: durationLabel })}
          </span>
          <ChevronDown
            className={`h-3.5 w-3.5 text-slate-500 transition-transform duration-250 ${
              isOpen ? 'rotate-180' : ''
            }`}
          />
        </div>

        <div className="ml-auto flex flex-wrap justify-end gap-1.5">
          {summaries.map((summary) => {
            const accentClassName =
              summary.kind === 'search'
                ? 'border-indigo-900/40 bg-indigo-950/10 text-indigo-300/90'
                : summary.kind === 'edit'
                  ? 'border-emerald-900/40 bg-emerald-950/10 text-emerald-300/90'
                  : 'border-zinc-800 bg-[#1e2022]/40 text-slate-400';

            return (
              <span
                key={`${summary.kind}-${summary.count}`}
                className={`rounded-md border px-2 py-0.5 text-[11px] font-normal leading-none ${accentClassName}`}
              >
                {getLocalizedSummaryLabel(summary, t)}
              </span>
            );
          })}
        </div>
      </button>

      <div className={`grid-collapse-wrapper ${isOpen ? 'is-open' : ''}`}>
        <div className="grid-collapse-content">
          <div className="border-t border-[#25282d] px-4 py-3.5">
            <div className="flex flex-col">
              {turn.workItems.map((item, index) => {
                if (item.kind === 'assistant') {
                  const text = (item.line.text || '').trim();
                  const isDraft = item.line.status === 'draft';
                  const hasThinking = item.line.thinking && item.line.thinking.trim();

                  return (
                    <div
                      key={item.line.id}
                      data-native-context-menu="true"
                      className="relative pl-8 pb-3.5 select-text"
                    >
                      {/* Timeline Line Segments */}
                      {index > 0 && (
                        <div className="absolute left-[18px] top-0 h-[10px] w-[1px] bg-zinc-800/60" />
                      )}
                      {index < turn.workItems.length - 1 && (
                        <div className="absolute left-[18px] top-[24px] bottom-0 w-[1px] bg-zinc-800/60" />
                      )}

                      {/* Timeline Dot Node */}
                      <span className="absolute left-[11px] top-[10px] flex h-3.5 w-3.5 items-center justify-center rounded-full border border-indigo-500/25 bg-indigo-950/40">
                        <span className="h-1.5 w-1.5 rounded-full bg-indigo-400/80" />
                      </span>

                      <div className="rounded-xl bg-transparent py-0.5 text-sm text-slate-300">
                        {hasThinking && (
                          <details className="mb-2" open={isDraft}>
                            <summary className="cursor-pointer text-xs font-medium text-indigo-300/70 hover:text-indigo-200 transition-colors select-none">
                              {isDraft ? (
                                <span className="inline-flex items-center gap-1.5">
                                  <Loader2 className="h-3 w-3 animate-spin" />
                                  {t('chat.thinking')}
                                </span>
                              ) : (
                                t('chat.thinking')
                              )}
                            </summary>
                            <div className="mt-1.5 border-l-2 border-indigo-500/20 pl-3 text-xs text-slate-400 leading-relaxed whitespace-pre-wrap">
                              {item.line.thinking}
                            </div>
                          </details>
                        )}
                        {text ? (
                          <div className="prose prose-invert prose-sm max-w-none [&_code]:!rounded-md [&_code]:!bg-[#1b1c1d] [&_code]:!px-1.5 [&_code]:!py-0.5 [&_code]:!text-[11px] [&_code]:!text-slate-200 [&_p]:!my-1 [&_pre]:!rounded-xl [&_pre]:!border [&_pre]:!border-zinc-800 [&_pre]:!bg-[#0c0d0e]">
                            {renderMarkdown(
                              text === 'chat.stopped' ? t('chat.stopped') : text,
                              isDraft ? (
                                <span className="ml-1.5 inline-block h-2 w-2 animate-pulse rounded-full bg-white align-middle" />
                              ) : undefined,
                              t
                            )}
                          </div>
                        ) : hasThinking ? null : (
                          <span className="inline-flex items-center gap-1.5 text-xs text-slate-500">
                            <Loader2 className="h-3 w-3 animate-spin" />
                            {t('chat.thinking')}
                          </span>
                        )}
                      </div>
                    </div>
                  );
                }

                const toolLine = item.line;
                const toolDisplayName = resolveToolDisplayName(toolLine);
                const toolOrigin = resolveToolOriginLabel(toolLine, t);
                const accent = resolveToolAccent(toolLine);
                const expanded = expandedTools.has(toolLine.callId);
                const AccentIcon = accent.icon;
                const statusLabel =
                  toolLine.status === 'done'
                    ? t('chat.work_items.completed')
                    : toolLine.status === 'failed'
                      ? t('chat.work_items.failed')
                      : t('chat.work_items.running');
                const toolDurationLabel = getToolDurationLabel(toolLine);

                return (
                  <div key={toolLine.id || `${toolLine.callId}-${index}`} className="relative pl-8 pb-3.5">
                    {/* Timeline Line Segments */}
                    {index > 0 && (
                      <div className="absolute left-[18px] top-0 h-[12px] w-[1px] bg-zinc-800/60" />
                    )}
                    {index < turn.workItems.length - 1 && (
                      <div className="absolute left-[18px] top-[28px] bottom-0 w-[1px] bg-zinc-800/60" />
                    )}

                    {/* Timeline Dot Node */}
                    {toolLine.status === 'done' ? (
                      <span className="absolute left-[10px] top-[12px] flex h-4 w-4 items-center justify-center rounded-full border border-emerald-500/30 bg-emerald-950/30" title={statusLabel}>
                        <span className="h-1.5 w-1.5 rounded-full bg-emerald-400" />
                      </span>
                    ) : toolLine.status === 'failed' ? (
                      <span className="absolute left-[10px] top-[12px] flex h-4 w-4 items-center justify-center rounded-full border border-rose-500/30 bg-rose-950/30 animate-pulse" title={statusLabel}>
                        <span className="h-1.5 w-1.5 rounded-full bg-rose-400" />
                      </span>
                    ) : (
                      <span className="absolute left-[10px] top-[12px] flex h-4 w-4 items-center justify-center rounded-full border border-indigo-500/30 bg-indigo-950/30" title={statusLabel}>
                        <span className="absolute h-1.5 w-1.5 rounded-full bg-indigo-400/80 animate-ping" />
                        <span className="relative h-1.5 w-1.5 rounded-full bg-indigo-400" />
                      </span>
                    )}

                    <div className={`overflow-hidden rounded-xl border ${accent.className} shadow-sm transition-all hover:border-[#343840]`}>
                      <button
                        type="button"
                        className="flex w-full items-center gap-2 px-3 py-2 text-left"
                        onClick={() => toggleToolExpanded(toolLine.callId)}
                      >
                        <AccentIcon className={`h-3.5 w-3.5 ${accent.iconClassName}`} />
                        <span className="rounded bg-[#202226] border border-zinc-800 px-1.5 py-0.5 text-[10px] font-medium text-slate-400">
                          {accent.badge}
                        </span>
                        <span className="truncate text-xs font-semibold text-slate-300">{toolDisplayName}</span>
                        <span className="truncate text-[10px] text-slate-500">{toolOrigin}</span>
                        <span className={`ml-auto text-[10px] ${
                          toolLine.status === 'done'
                            ? 'text-emerald-400/80'
                            : toolLine.status === 'failed'
                              ? 'text-rose-400/80'
                              : 'text-indigo-400/80'
                        }`}>
                          {statusLabel}
                          {toolDurationLabel ? ` · ${toolDurationLabel}` : ''}
                        </span>
                        <ChevronDown
                          className={`h-3.5 w-3.5 text-slate-500 transition-transform duration-250 ${
                            expanded ? 'rotate-180' : ''
                          }`}
                        />
                      </button>

                      <div className={`grid-collapse-wrapper ${expanded ? 'is-open' : ''}`}>
                        <div className="grid-collapse-content">
                          <ToolPayload
                            copiedToolId={copiedToolId}
                            description={toolLine.description || ''}
                            onCopy={handleCopyToolPayload}
                            toolLine={toolLine}
                          />
                        </div>
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}

interface ToolPayloadProps {
  copiedToolId: string | null;
  description: string;
  onCopy: (toolId: string, value: unknown) => Promise<void>;
  toolLine: ToolLine;
}

function ToolPayload({
  copiedToolId,
  description,
  onCopy,
  toolLine,
}: ToolPayloadProps) {
  const { t } = useI18n();
  const payloadId = toolLine.callId || toolLine.id;

  return (
    <div className="space-y-2 border-t border-black/10 bg-black/10 px-3 py-3">
      {description.trim() && (
        <p className="text-xs leading-relaxed text-slate-400">{description}</p>
      )}
      {toolLine.args != null && (
        <ToolPayloadBlock
          copied={copiedToolId === `${payloadId}:args`}
          label={t('chat.arguments')}
          onCopy={() => onCopy(`${payloadId}:args`, toolLine.args)}
          value={toolLine.args}
        />
      )}
      {toolLine.output != null && (
        <ToolPayloadBlock
          copied={copiedToolId === `${payloadId}:output`}
          label={t('chat.output')}
          onCopy={() => onCopy(`${payloadId}:output`, toolLine.output)}
          value={toolLine.output}
        />
      )}
    </div>
  );
}

interface ToolPayloadBlockProps {
  copied: boolean;
  label: string;
  onCopy: () => Promise<void>;
  value: unknown;
}

function ToolPayloadBlock({
  copied,
  label,
  onCopy,
  value,
}: ToolPayloadBlockProps) {
  const { t } = useI18n();
  return (
    <div>
      <div className="mb-1.5 flex items-center justify-between">
        <span className="text-[11px] font-medium uppercase tracking-[0.18em] text-slate-400">
          {label}
        </span>
        <button
          type="button"
          className="inline-flex items-center gap-1 rounded-md px-1.5 py-1 text-[11px] text-slate-400 transition hover:bg-white/5 hover:text-slate-200"
          onClick={() => void onCopy()}
        >
          {copied ? (
            <>
              <CheckCircle2 className="h-3.5 w-3.5 text-emerald-400" />
              {t('chat.copied')}
            </>
          ) : (
            <>
              <Copy className="h-3.5 w-3.5" />
              {t('chat.copy')}
            </>
          )}
        </button>
      </div>
      <OverlayScrollArea
        axis="both"
        className="max-h-52 whitespace-pre-wrap rounded-xl border border-white/6 bg-[#101112] p-3 text-[11px] leading-relaxed text-slate-200"
      >
        {formatToolOutput(value)}
      </OverlayScrollArea>
    </div>
  );
}
