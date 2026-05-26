import { useEffect, useRef, useState } from 'react';
import {
  CheckCircle2,
  Copy,
  Loader2,
  MessageSquare,
  Sparkles,
} from 'lucide-react';
import { renderMarkdown } from './chat/markdownRenderer';
import type {
  ConversationLine,
  AssistantLine,
  ToolLine,
  UserLine,
} from '../features/conversations/types';

interface ChatAreaProps {
  lines: ConversationLine[];
  isGenerating: boolean;
  isThinking: boolean;
  activeChatTitle: string;
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
  return { displayName: name, origin: '\u5185\u7F6E' };
};

export default function ChatArea({
  lines,
  isGenerating,
  isThinking,
  activeChatTitle,
}: ChatAreaProps) {
  const messagesEndRef = useRef<HTMLDivElement | null>(null);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [expandedTools, setExpandedTools] = useState<Set<string>>(new Set());

  const handleCopy = async (id: string, text: string) => {
    await navigator.clipboard.writeText(text);
    setCopiedId(id);
    window.setTimeout(() => setCopiedId(null), 2000);
  };

  const toggleToolExpanded = (callId: string) => {
    setExpandedTools((prev) => {
      const next = new Set(prev);
      if (next.has(callId)) next.delete(callId);
      else next.add(callId);
      return next;
    });
  };

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [lines, isGenerating, isThinking]);

  if (lines.length === 0) return null;

  return (
    <div className="scrollbar-thin flex flex-1 flex-col overflow-y-auto px-4 py-6 md:px-8 lg:px-12">
      <div className="mx-auto flex w-full max-w-3xl flex-col gap-3 py-4">
        <div className="mb-2 flex items-center gap-2 border-b border-[#2d2f31]/60 pb-3 text-xs font-semibold tracking-widest text-slate-500 uppercase">
          <MessageSquare className="h-4 w-4 text-cyan-400" />
          <span>{activeChatTitle}</span>
        </div>

        {lines.map((line) => {
          // ── user message ──────────────────────────────────────────
          if (line.kind === 'user') {
            return (
              <div key={line.id} className="flex justify-end">
                <div
                  data-native-context-menu="true"
                  className="max-w-[80%] break-words rounded-3xl border border-[#2d2f31]/30 bg-[#1e1f20] px-5 py-3.5 text-sm leading-relaxed text-slate-200 transition hover:border-slate-500/30 select-text"
                >
                  {(line as UserLine).text && String((line as UserLine).text)}
                </div>
              </div>
            );
          }

          // ── tool card ─────────────────────────────────────────────
          if (line.kind === 'tool') {
            const t = line as ToolLine;
            const { displayName, origin } = resolveToolDisplayName(t.name);
            const expanded = expandedTools.has(t.callId);
            const statusColor =
              t.status === 'done'
                ? 'border-emerald-500/20 bg-emerald-500/5'
                : t.status === 'failed'
                  ? 'border-rose-500/20 bg-rose-500/5'
                  : 'border-amber-500/20 bg-amber-500/5';

            return (
              <div key={line.id} className="flex justify-start">
                <div className={`max-w-xl rounded-xl border px-4 py-2.5 text-xs ${statusColor}`}>
                  <button
                    className="flex w-full items-center gap-2 text-left"
                    onClick={() => toggleToolExpanded(t.callId)}
                  >
                    {t.status === 'done' ? (
                      <CheckCircle2 className="h-3.5 w-3.5 text-emerald-400" />
                    ) : t.status === 'failed' ? (
                      <span className="h-3.5 w-3.5 text-rose-400">{'\u2717'}</span>
                    ) : (
                      <Loader2 className="h-3.5 w-3.5 animate-spin text-amber-400" />
                    )}
                    <span className="font-medium text-slate-300">{displayName}</span>
                    <span className="text-slate-500">{'\u00B7'}</span>
                    <span className="text-slate-500">{origin}</span>
                    <span className="ml-auto text-[10px] text-slate-500">
                      {t.status === 'done' ? '\u5B8C\u6210' : t.status === 'failed' ? '\u5931\u8D25' : '\u6267\u884C\u4E2D'}
                    </span>
                  </button>
                  {expanded && (
                    <div className="mt-2 space-y-2 border-t border-white/5 pt-2">
                      {t.args != null && (
                        <div>
                          <span className="text-[10px] font-semibold text-slate-500 uppercase tracking-wider">{'\u53C2\u6570'}</span>
                          <pre className="mt-0.5 overflow-x-auto rounded bg-black/20 p-2 text-[11px] text-slate-300 max-h-24">
                            {typeof t.args === 'string' ? t.args : JSON.stringify(t.args, null, 2)}
                          </pre>
                        </div>
                      )}
                      {t.output != null && (
                        <div>
                          <span className="text-[10px] font-semibold text-slate-500 uppercase tracking-wider">{'\u8F93\u51FA'}</span>
                          <pre className="mt-0.5 overflow-x-auto rounded bg-black/20 p-2 text-[11px] text-slate-300 max-h-32">
                            {formatToolOutput(t.output)}
                          </pre>
                        </div>
                      )}
                    </div>
                  )}
                </div>
              </div>
            );
          }

          // ── assistant message ─────────────────────────────────────
          if (line.kind === 'assistant') {
            const a = line as AssistantLine;
            const isDraft = a.status === 'draft';
            const isEmpty = !a.text || (typeof a.text === 'string' && !a.text.trim());
            const isCommentary = a.phase === 'commentary';
            if (isEmpty && !isDraft) return null;

            return (
              <div key={line.id} className={`flex gap-4 justify-start ${isCommentary ? 'pl-6' : ''}`}>
                {!isCommentary && (
                  <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-gradient-to-tr from-cyan-400 via-purple-500 to-pink-500 text-white shadow-md shadow-purple-500/20">
                    <Sparkles className={`h-4.5 w-4.5 ${isDraft ? 'animate-pulse' : ''}`} />
                  </div>
                )}
                {isCommentary && (
                  <div className="mt-1 flex h-6 w-6 shrink-0 items-center justify-center rounded-full border border-amber-500/20 bg-amber-500/10 text-amber-300">
                    <Loader2 className={`h-3.5 w-3.5 ${isDraft ? 'animate-spin' : ''}`} />
                  </div>
                )}
                <div className="flex-1 space-y-1.5 overflow-hidden">
                  <div className="group flex items-start gap-2">
                    <div className={`flex-1 leading-relaxed select-text prose prose-invert max-w-none [&_pre]:!bg-[#0d0d0e] [&_pre]:!border [&_pre]:!border-[#2d2f31]/30 [&_pre]:!rounded-xl [&_code]:!bg-[#0d0d0e] [&_code]:!text-pink-300 [&_code]:!px-1.5 [&_code]:!py-0.5 [&_code]:!rounded-md [&_code]:!text-xs ${isCommentary ? 'text-xs prose-sm text-amber-100/90' : 'text-sm text-slate-200'}`}>
                      {isEmpty ? (
                        <span className="inline-flex items-center gap-1.5 text-slate-500 text-xs">
                          <Loader2 className="h-3 w-3 animate-spin" />
                          {'\u601D\u8003\u4E2D...'}
                        </span>
                      ) : (
                        <>{renderMarkdown(String(a.text))}</>
                      )}
                      {isDraft && !isEmpty && (
                        <span className="inline-block ml-1 w-1.5 h-4 bg-cyan-400 animate-pulse rounded-sm align-middle" />
                      )}
                    </div>
                    {!isEmpty && !isCommentary && (
                      <button
                        className="mt-0.5 shrink-0 opacity-0 group-hover:opacity-100 transition p-1 rounded hover:bg-white/5"
                        onClick={() => handleCopy(a.id, a.text)}
                        title="\u590D\u5236"
                      >
                        {copiedId === a.id ? (
                          <CheckCircle2 className="h-3.5 w-3.5 text-emerald-400" />
                        ) : (
                          <Copy className="h-3.5 w-3.5 text-slate-500" />
                        )}
                      </button>
                    )}
                  </div>
                </div>
              </div>
            );
          }

          return null;
        })}

        {isThinking && (
          <div className="flex items-center gap-3 py-1">
            <div className="h-px flex-1 bg-gradient-to-r from-transparent via-purple-500/30 to-transparent" />
            <span className="flex items-center gap-1.5 text-xs font-medium text-purple-400 whitespace-nowrap">
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
              {'\u6A21\u578B\u601D\u8003\u4E2D...'}
            </span>
            <div className="h-px flex-1 bg-gradient-to-r from-transparent via-purple-500/30 to-transparent" />
          </div>
        )}

        <div ref={messagesEndRef} />
      </div>
    </div>
  );
}
