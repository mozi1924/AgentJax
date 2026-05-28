import { useEffect, useMemo, useRef, useState } from 'react';
import { CheckCircle2, Copy, Loader2, MessageSquare } from 'lucide-react';
import WorkLogPanel from './chat/WorkLogPanel';
import { renderMarkdown } from './chat/markdownRenderer';
import {
  buildConversationTurns,
  hasTurnWorkLog,
  joinAssistantTexts,
  shouldCollapseTurnWorkLog,
} from './chat/transcriptGrouping';
import type {
  AssistantLine,
  ConversationLine,
  UserLine,
} from '../features/conversations/types';
import { useI18n } from '../features/i18n';

interface ChatAreaProps {
  lines: ConversationLine[];
  isGenerating: boolean;
  isThinking: boolean;
  activeChatTitle: string;
  contextTokenCount: number;
}

const createAssistantTextClassName = (isCommentary: boolean) =>
  [
    'prose prose-invert max-w-none',
    '[&_code]:!rounded-md [&_code]:!bg-[#101112] [&_code]:!px-1.5 [&_code]:!py-0.5',
    '[&_code]:!text-[11px] [&_code]:!text-slate-200 [&_pre]:!rounded-xl',
    '[&_pre]:!border [&_pre]:!border-white/8 [&_pre]:!bg-[#101112]',
    isCommentary
      ? 'prose-sm text-slate-300 [&_p]:!my-1.5'
      : '[&_p]:!my-2.5 text-[15px] leading-7 text-slate-100',
  ].join(' ');

/**
 * Renders the conversation as request-scoped turns so final answers can stay
 * prominent while intermediate work remains available on demand.
 */
export default function ChatArea({
  lines,
  isGenerating,
  isThinking,
  activeChatTitle,
  contextTokenCount,
}: ChatAreaProps) {
  const { t } = useI18n();
  const containerRef = useRef<HTMLDivElement | null>(null);
  const wasAtBottomRef = useRef(true);
  const prevLinesLengthRef = useRef(lines.length);
  const isThinkingRef = useRef(isThinking);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [manualWorkLogState, setManualWorkLogState] = useState<Record<string, boolean>>({});
  const formattedContextTokenCount = new Intl.NumberFormat().format(contextTokenCount || 0);

  const handleScroll = () => {
    const container = containerRef.current;
    if (!container) return;

    const threshold = 15;
    const isAtBottom =
      container.scrollHeight - container.scrollTop - container.clientHeight <= threshold;
    wasAtBottomRef.current = isAtBottom;
  };

  const turns = useMemo(() => buildConversationTurns(lines), [lines]);

  useEffect(() => {
    const requestIds = new Set(turns.map((turn) => turn.requestId));
    setManualWorkLogState((prev) => {
      const entries = Object.entries(prev).filter(([requestId]) => requestIds.has(requestId));
      if (entries.length === Object.keys(prev).length) {
        return prev;
      }
      return Object.fromEntries(entries);
    });
  }, [turns]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const currentLinesLength = lines.length;
    const prevLinesLength = prevLinesLengthRef.current;
    prevLinesLengthRef.current = currentLinesLength;

    const isNewMessage = currentLinesLength > prevLinesLength;
    const becameThinking = isThinking && !isThinkingRef.current;
    isThinkingRef.current = isThinking;

    if (isNewMessage || becameThinking) {
      container.scrollTo({
        top: container.scrollHeight,
        behavior: 'smooth',
      });
      wasAtBottomRef.current = true;
      return;
    }

    if (isGenerating && wasAtBottomRef.current) {
      container.scrollTop = container.scrollHeight;
    }
  }, [lines, isGenerating, isThinking]);

  const handleCopy = async (id: string, text: string) => {
    await navigator.clipboard.writeText(text);
    setCopiedId(id);
    window.setTimeout(() => setCopiedId(null), 2000);
  };

  const toggleWorkLog = (requestId: string, nextOpen: boolean) => {
    setManualWorkLogState((prev) => ({
      ...prev,
      [requestId]: nextOpen,
    }));
  };

  if (lines.length === 0) return null;

  return (
    <div
      ref={containerRef}
      onScroll={handleScroll}
      className="scrollbar-thin flex flex-1 flex-col overflow-y-auto px-4 py-6 md:px-8 lg:px-12"
    >
      <div className="mx-auto flex w-full max-w-3xl flex-col gap-5 py-4">
        <div className="mb-1 flex flex-wrap items-center gap-2 border-b border-white/6 pb-3 text-xs font-semibold tracking-[0.18em] text-slate-500 uppercase">
          <MessageSquare className="h-4 w-4 text-slate-400" />
          <span className="min-w-0 max-w-full truncate">{activeChatTitle}</span>
          {(contextTokenCount > 0 || lines.length > 0) && (
            <span className="inline-flex items-center gap-1 rounded-full border border-emerald-400/20 bg-emerald-500/10 px-2 py-0.5 text-[11px] font-medium tracking-[0.06em] text-emerald-200 shadow-[0_0_0_1px_rgba(16,185,129,0.08)]">
              <span className="font-mono tabular-nums text-emerald-100">
                {formattedContextTokenCount}
              </span>
              <span>{t('chat.context_tokens')}</span>
            </span>
          )}
        </div>

        {turns.map((turn) => {
          const joinedFinalText = joinAssistantTexts(turn.finalLines);
          const defaultWorkLogOpen = !shouldCollapseTurnWorkLog(turn);
          const isWorkLogOpen =
            manualWorkLogState[turn.requestId] ?? defaultWorkLogOpen;

          return (
            <section key={turn.requestId} className="space-y-3">
              {turn.userLines.map((line) => (
                <UserMessageBubble key={line.id} line={line} />
              ))}

              {hasTurnWorkLog(turn) && (
                <WorkLogPanel
                  isOpen={isWorkLogOpen}
                  onToggle={() => toggleWorkLog(turn.requestId, !isWorkLogOpen)}
                  turn={turn}
                />
              )}

              {turn.finalLines.length > 0 && (
                <AssistantFinalCard
                  copiedId={copiedId}
                  lines={turn.finalLines}
                  onCopy={handleCopy}
                  turnText={joinedFinalText}
                />
              )}
            </section>
          );
        })}

        {isThinking && (
          <div className="flex items-center gap-3 py-1">
            <div className="h-px flex-1 bg-gradient-to-r from-transparent via-white/10 to-transparent" />
            <span className="flex items-center gap-1.5 whitespace-nowrap text-xs font-medium text-slate-400">
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
              {t('chat.thinking')}
            </span>
            <div className="h-px flex-1 bg-gradient-to-r from-transparent via-white/10 to-transparent" />
          </div>
        )}
        {/* No more dummy div needed for scrollIntoView since container scroll handles it */}
      </div>
    </div>
  );
}

function UserMessageBubble({ line }: { line: UserLine }) {
  return (
    <div className="flex justify-end">
      <div
        data-native-context-menu="true"
        className="max-w-[85%] break-words rounded-2xl border border-[#34383e] bg-[#202225] px-4 py-2.5 text-sm leading-6 text-slate-100 shadow-[0_4px_12px_rgba(0,0,0,0.1)] select-text"
      >
        {line.text && String(line.text)}
      </div>
    </div>
  );
}

interface AssistantFinalCardProps {
  copiedId: string | null;
  lines: AssistantLine[];
  onCopy: (id: string, text: string) => Promise<void>;
  turnText: string;
}

function AssistantFinalCard({
  copiedId,
  lines,
  onCopy,
  turnText,
}: AssistantFinalCardProps) {
  const { t } = useI18n();
  return (
    <div
      data-native-context-menu="true"
      className="group rounded-xl border border-zinc-800/80 bg-[#161718]/45 px-4.5 py-3.5 shadow-[0_4px_20px_rgba(0,0,0,0.15)] transition-all duration-300 hover:border-zinc-700/60 select-text"
    >
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1 overflow-hidden">
          {lines.map((line) => {
            const isDraft = line.status === 'draft';
            const isEmpty = !line.text || !line.text.trim();

            if (isEmpty && !isDraft) {
              return null;
            }

            return (
              <div
                key={line.id}
                className="bg-transparent py-0.5 text-slate-100"
              >
                <div className={createAssistantTextClassName(false)}>
                  {isEmpty ? (
                    <span className="inline-flex items-center gap-1.5 text-sm text-slate-500">
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      {t('chat.thinking')}
                    </span>
                  ) : (
                    renderMarkdown(
                      String(line.text),
                      isDraft && !isEmpty ? (
                        <span className="ml-1.5 inline-block h-2 w-2 animate-pulse rounded-full bg-white align-middle" />
                      ) : undefined
                    )
                  )}
                </div>
              </div>
            );
          })}
        </div>

        {turnText && (
          <button
            type="button"
            className="mt-0.5 shrink-0 rounded-lg p-1.5 opacity-0 transition hover:bg-white/5 group-hover:opacity-100"
            onClick={() => void onCopy(lines[0]?.id || 'assistant-final', turnText)}
            title={t('chat.copy_final')}
          >
            {copiedId === (lines[0]?.id || 'assistant-final') ? (
              <CheckCircle2 className="h-3.5 w-3.5 text-emerald-400" />
            ) : (
              <Copy className="h-3.5 w-3.5 text-slate-500" />
            )}
          </button>
        )}
      </div>
    </div>
  );
}
