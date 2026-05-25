import { useEffect, useRef, useState } from 'react';
import {
  AlertTriangle,
  CheckCircle2,
  Copy,
  Loader2,
  MessageSquare,
  RotateCcw,
  Sparkles,
} from 'lucide-react';
import ToolCallWidget from './chat/ToolCallWidget';
import { renderMarkdown } from './chat/markdownRenderer';
import type { ConversationMessage } from '../features/conversations/types';

interface ChatAreaProps {
  messages: ConversationMessage[];
  isGenerating: boolean;
  isThinking: boolean;
  onRetryMessage?: (assistantMessageId: string) => void;
  activeChatTitle: string;
}

export default function ChatArea({
  messages,
  isGenerating,
  isThinking,
  onRetryMessage,
  activeChatTitle,
}: ChatAreaProps) {
  const messagesEndRef = useRef<HTMLDivElement | null>(null);
  const [copiedMessageId, setCopiedMessageId] = useState<string | null>(null);
  const [copiedErrorId, setCopiedErrorId] = useState<string | null>(null);

  const handleCopyMessage = async (msgId: string, text: string) => {
    await navigator.clipboard.writeText(text);
    setCopiedMessageId(msgId);
    window.setTimeout(() => setCopiedMessageId(null), 2000);
  };

  const handleCopyError = async (msgId: string, errorText: string) => {
    await navigator.clipboard.writeText(errorText);
    setCopiedErrorId(msgId);
    window.setTimeout(() => setCopiedErrorId(null), 2000);
  };

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages, isGenerating, isThinking]);

  if (messages.length === 0) {
    return null;
  }

  return (
    <div className="scrollbar-thin flex flex-1 flex-col overflow-y-auto px-4 py-6 md:px-8 lg:px-12">
      <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 py-4">
        <div className="mb-2 flex items-center gap-2 border-b border-[#2d2f31]/60 pb-3 text-xs font-semibold tracking-widest text-slate-500 uppercase">
          <MessageSquare className="h-4 w-4 text-cyan-400" />
          <span>{activeChatTitle}</span>
        </div>

        {messages.map((message) => (
          <div
            key={message.id}
            className={`group flex gap-4 ${
              message.role === 'user' ? 'justify-end' : 'justify-start'
            }`}
          >
            {message.role === 'assistant' && (
              <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-gradient-to-tr from-cyan-400 via-purple-500 to-pink-500 text-white shadow-md shadow-purple-500/20">
                <Sparkles className="h-4.5 w-4.5 animate-pulse" />
              </div>
            )}

            {message.role === 'user' ? (
              <div
                data-native-context-menu="true"
                className="max-w-[80%] break-words rounded-3xl border border-[#2d2f31]/30 bg-[#1e1f20] px-5 py-3.5 text-sm leading-relaxed text-slate-200 transition hover:border-slate-500/30 select-text"
              >
                {message.text}
              </div>
            ) : (
              <div className="flex-1 space-y-1.5 overflow-hidden">
                {Array.isArray(message.toolCalls) && message.toolCalls.length > 0 && (
                  <div className="mb-2 max-w-xl space-y-1">
                    {message.toolCalls.map((toolCall) => (
                      <ToolCallWidget key={toolCall.id} toolCall={toolCall} />
                    ))}
                  </div>
                )}

                {message.status === 'failed' || message.status === 'interrupted' ? (
                  <div className="rounded-2xl border border-rose-500/30 bg-rose-950/20 px-4 py-3 select-text">
                    <div className="flex items-start gap-2">
                      <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-rose-300" />
                      <div className="min-w-0 flex-1">
                        <p className="text-sm text-rose-200">
                          {message.status === 'interrupted'
                            ? '上次请求在回复完成前中断了。'
                            : '请求失败，未完成这轮回复。'}
                        </p>
                        <p className="mt-1 text-xs break-words text-rose-300/90">
                          {message.errorText || '请检查网络或配置后重试。'}
                        </p>
                        <div className="mt-3 flex gap-2 select-none">
                          <button
                            onClick={() => onRetryMessage?.(message.id)}
                            disabled={isGenerating}
                            className={`inline-flex items-center gap-1.5 rounded-full px-3 py-1.5 text-xs font-medium transition ${
                              isGenerating
                                ? 'cursor-not-allowed border border-[#2d2f31] text-slate-500'
                                : 'border border-rose-400/40 text-rose-200 hover:bg-rose-900/40'
                            }`}
                          >
                            <RotateCcw className="h-3.5 w-3.5" />
                            重试这条消息
                          </button>
                          <button
                            onClick={() =>
                              handleCopyError(
                                message.id,
                                message.errorText || '请检查网络或配置后重试。'
                              )
                            }
                            className="inline-flex cursor-pointer items-center gap-1.5 rounded-full border border-rose-500/20 bg-rose-500/5 px-3 py-1.5 text-xs font-medium text-rose-200 transition hover:bg-rose-500/10"
                          >
                            {copiedErrorId === message.id ? (
                              <>
                                <CheckCircle2 className="h-3.5 w-3.5 text-emerald-400" />
                                <span className="text-emerald-400">已复制错误</span>
                              </>
                            ) : (
                              <>
                                <Copy className="h-3.5 w-3.5" />
                                <span>复制错误信息</span>
                              </>
                            )}
                          </button>
                        </div>
                      </div>
                    </div>
                  </div>
                ) : message.status === 'streaming' && !message.text ? (
                  <div className="flex min-h-8 items-center">
                    <div className="inline-flex items-center gap-2 rounded-full border border-[#2d2f31] bg-[#1b1c1d] px-3 py-1.5 text-xs text-slate-400">
                      <Loader2 className="h-3.5 w-3.5 animate-spin text-cyan-300" />
                      {isThinking && <span>思考中...</span>}
                    </div>
                  </div>
                ) : (
                  <>
                    <div
                      data-native-context-menu="true"
                      className="prose prose-invert max-w-none text-slate-300 select-text"
                    >
                      {renderMarkdown(message.text)}
                    </div>
                    {message.status !== 'streaming' && message.text && (
                      <div className="mt-2.5 flex items-center gap-4 text-xs text-slate-500 opacity-0 transition-opacity duration-200 group-hover:opacity-70 hover:!opacity-100 select-none">
                        <button
                          onClick={() => handleCopyMessage(message.id, message.text)}
                          className="flex cursor-pointer items-center gap-1.5 py-1 transition duration-150 hover:text-slate-300"
                          title="复制全文"
                        >
                          {copiedMessageId === message.id ? (
                            <>
                              <CheckCircle2 className="h-3.5 w-3.5 text-emerald-400" />
                              <span className="font-medium text-emerald-400">已复制全文</span>
                            </>
                          ) : (
                            <>
                              <Copy className="h-3.5 w-3.5" />
                              <span>复制全文</span>
                            </>
                          )}
                        </button>
                      </div>
                    )}
                  </>
                )}
              </div>
            )}
          </div>
        ))}

        {isGenerating && messages[messages.length - 1]?.role === 'user' && (
          <div className="flex items-start justify-start gap-4">
            <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-gradient-to-tr from-cyan-400 via-purple-500 to-pink-500 text-white shadow-md shadow-purple-500/20">
              <Loader2 className="h-4.5 w-4.5 animate-spin" />
            </div>
            <div className="flex w-full flex-col gap-2 pt-1.5">
              <div className="h-4 w-3/4 animate-pulse rounded bg-[#2d2f31]" />
              <div className="h-4 w-1/2 animate-pulse rounded bg-[#2d2f31]" />
              <div className="h-4 w-5/6 animate-pulse rounded bg-[#2d2f31]" />
            </div>
          </div>
        )}
      </div>

      <div ref={messagesEndRef} />
    </div>
  );
}
