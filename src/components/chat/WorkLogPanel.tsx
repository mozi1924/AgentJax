import { useState } from 'react';
import {
  CheckCircle2,
  ChevronDown,
  Copy,
  FilePenLine,
  Loader2,
  Search,
  Sparkles,
  Wrench,
} from 'lucide-react';
import type { ToolLine } from '../../features/conversations/types';
import { renderMarkdown } from './markdownRenderer';
import {
  formatTurnDuration,
  getTurnActivitySummary,
  getTurnDurationMs,
  type ConversationTurn,
} from './transcriptGrouping';

interface WorkLogPanelProps {
  isOpen: boolean;
  onToggle: () => void;
  turn: ConversationTurn;
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

const resolveToolDisplayName = (name: string) => {
  if (name.startsWith('mcp__')) {
    const parts = name.split('__');
    if (parts.length >= 3) {
      return { displayName: parts.slice(2).join('__'), origin: `MCP: ${parts[1]}` };
    }
  }

  return { displayName: name, origin: 'Built-in' };
};

const resolveToolAccent = (toolName: string) => {
  if (/search|find|glob|\brg\b|\bgrep\b/i.test(toolName)) {
    return {
      icon: Search,
      badge: 'Explored',
      className: 'border-cyan-400/15 bg-cyan-400/[0.04] text-cyan-100',
      iconClassName: 'text-cyan-300',
    };
  }

  if (/apply_patch|edit|write|push_files|create_or_update_file|delete_file/i.test(toolName)) {
    return {
      icon: FilePenLine,
      badge: 'Edited',
      className: 'border-emerald-400/15 bg-emerald-400/[0.04] text-emerald-100',
      iconClassName: 'text-emerald-300',
    };
  }

  return {
    icon: Wrench,
    badge: 'Tool',
    className: 'border-white/8 bg-white/[0.03] text-slate-100',
    iconClassName: 'text-slate-300',
  };
};

/**
 * Keeps intermediate commentary and tool activity inside one collapsible
 * transcript panel so the final answer can stay visually dominant.
 */
export default function WorkLogPanel({
  isOpen,
  onToggle,
  turn,
}: WorkLogPanelProps) {
  const [copiedToolId, setCopiedToolId] = useState<string | null>(null);
  const [expandedTools, setExpandedTools] = useState<Set<string>>(new Set());
  const summaries = getTurnActivitySummary(turn);
  const durationLabel = formatTurnDuration(getTurnDurationMs(turn));

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
    <section className="rounded-2xl border border-white/6 bg-white/[0.02] shadow-[0_18px_40px_-30px_rgba(0,0,0,0.9)]">
      <button
        type="button"
        className="flex w-full items-center gap-3 px-3 py-2.5 text-left transition hover:bg-white/[0.02]"
        onClick={onToggle}
      >
        <div className="flex items-center gap-2">
          {turn.hasDraft ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin text-slate-300" />
          ) : (
            <Sparkles className="h-3.5 w-3.5 text-slate-400" />
          )}
          <span className="rounded-full border border-sky-400/50 px-2.5 py-0.5 text-sm text-slate-200">
            Worked for {durationLabel}
          </span>
          <ChevronDown
            className={`h-3.5 w-3.5 text-slate-500 transition-transform ${
              isOpen ? 'rotate-180' : ''
            }`}
          />
        </div>

        <div className="ml-auto flex flex-wrap justify-end gap-1.5">
          {summaries.map((summary) => {
            const accentClassName =
              summary.kind === 'search'
                ? 'border-cyan-400/15 bg-cyan-400/[0.04] text-cyan-100'
                : summary.kind === 'edit'
                  ? 'border-emerald-400/15 bg-emerald-400/[0.04] text-emerald-100'
                  : 'border-white/8 bg-white/[0.03] text-slate-300';

            return (
              <span
                key={`${summary.kind}-${summary.count}`}
                className={`rounded-full border px-2 py-0.5 text-xs ${accentClassName}`}
              >
                {summary.label}
              </span>
            );
          })}
        </div>
      </button>

      {isOpen && (
        <div className="border-t border-white/6 px-3 py-3">
          <div className="space-y-2.5 border-l border-white/7 pl-4">
            {turn.workItems.map((item, index) => {
              if (item.kind === 'assistant') {
                const text = (item.line.text || '').trim();
                const isDraft = item.line.status === 'draft';

                return (
                  <div key={item.line.id} className="relative pl-2">
                    <span className="absolute -left-[23px] top-2.5 h-2 w-2 rounded-full bg-slate-500/80" />
                    <div className="rounded-2xl bg-transparent py-0.5 text-sm text-slate-300">
                      {text ? (
                        <div className="prose prose-invert prose-sm max-w-none [&_code]:!rounded-md [&_code]:!bg-[#1b1c1d] [&_code]:!px-1.5 [&_code]:!py-0.5 [&_code]:!text-[11px] [&_code]:!text-slate-200 [&_p]:!my-1.5 [&_pre]:!rounded-xl [&_pre]:!border [&_pre]:!border-white/8 [&_pre]:!bg-[#101112]">
                          {renderMarkdown(text)}
                        </div>
                      ) : (
                        <span className="inline-flex items-center gap-1.5 text-xs text-slate-500">
                          <Loader2 className="h-3 w-3 animate-spin" />
                          Thinking...
                        </span>
                      )}
                      {isDraft && text && (
                        <span className="ml-1 inline-block h-4 w-1.5 animate-pulse rounded-sm bg-slate-400 align-middle" />
                      )}
                    </div>
                  </div>
                );
              }

              const toolLine = item.line;
              const toolMeta = resolveToolDisplayName(toolLine.name || '');
              const accent = resolveToolAccent(toolLine.name || '');
              const expanded = expandedTools.has(toolLine.callId);
              const AccentIcon = accent.icon;
              const statusLabel =
                toolLine.status === 'done'
                  ? 'Completed'
                  : toolLine.status === 'failed'
                    ? 'Failed'
                    : 'Running';

              return (
                <div key={toolLine.id || `${toolLine.callId}-${index}`} className="relative pl-2">
                  <span className="absolute -left-[23px] top-4 h-2 w-2 rounded-full bg-slate-500/80" />
                  <div className={`overflow-hidden rounded-2xl border ${accent.className}`}>
                    <button
                      type="button"
                      className="flex w-full items-center gap-2 px-3 py-2 text-left"
                      onClick={() => toggleToolExpanded(toolLine.callId)}
                    >
                      <AccentIcon className={`h-3.5 w-3.5 ${accent.iconClassName}`} />
                      <span className="rounded-full border border-current/10 bg-black/10 px-2 py-0.5 text-[11px] opacity-80">
                        {accent.badge}
                      </span>
                      <span className="truncate text-sm font-medium">{toolMeta.displayName}</span>
                      <span className="truncate text-xs text-slate-400">{toolMeta.origin}</span>
                      <span className="ml-auto text-xs text-slate-400">{statusLabel}</span>
                      <ChevronDown
                        className={`h-3.5 w-3.5 text-slate-500 transition-transform ${
                          expanded ? 'rotate-180' : ''
                        }`}
                      />
                    </button>

                    {expanded && (
                      <ToolPayload
                        copiedToolId={copiedToolId}
                        onCopy={handleCopyToolPayload}
                        toolLine={toolLine}
                      />
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}
    </section>
  );
}

interface ToolPayloadProps {
  copiedToolId: string | null;
  onCopy: (toolId: string, value: unknown) => Promise<void>;
  toolLine: ToolLine;
}

function ToolPayload({
  copiedToolId,
  onCopy,
  toolLine,
}: ToolPayloadProps) {
  const payloadId = toolLine.callId || toolLine.id;

  return (
    <div className="space-y-2 border-t border-black/10 bg-black/10 px-3 py-3">
      {toolLine.args != null && (
        <ToolPayloadBlock
          copied={copiedToolId === `${payloadId}:args`}
          label="Arguments"
          onCopy={() => onCopy(`${payloadId}:args`, toolLine.args)}
          value={toolLine.args}
        />
      )}
      {toolLine.output != null && (
        <ToolPayloadBlock
          copied={copiedToolId === `${payloadId}:output`}
          label="Output"
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
              Copied
            </>
          ) : (
            <>
              <Copy className="h-3.5 w-3.5" />
              Copy
            </>
          )}
        </button>
      </div>
      <pre className="scrollbar-thin max-h-52 overflow-x-auto rounded-xl border border-white/6 bg-[#101112] p-3 text-[11px] leading-relaxed text-slate-200">
        {formatToolOutput(value)}
      </pre>
    </div>
  );
}
