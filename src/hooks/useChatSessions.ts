import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';
import {
  applyConversationTitle,
  buildDraftConversationTitle,
  createLocalConversation,
  getConversationDisplayTitle,
  hydrateConversationLines,
  isConversationEmpty,
  shouldShowConversationInSidebar,
} from '../features/conversations/conversationUtils';
import {
  countVisibleMessages,
  ensureAtLeastOneConversation,
  mergeWithLocalDrafts,
  restoreConversationPreview,
} from '../features/conversations/conversationState';
import type {
  AssistantLine,
  ChatRequestOptions,
  ChatStreamEventPayload,
  ChatStreamResponse,
  Conversation,
  ConversationDetail,
  ConversationSummary,
  ModelOption,
  ToolLine,
  UserLine,
} from '../features/conversations/types';
import { DEFAULT_REASONING_MODE } from '../features/models/modelCatalog';

interface ComposerAttachment {
  name: string;
  type: string;
}

interface StreamRequestMapping {
  conversationId: string;
  lastEventIndex: number;
}

interface SendMessageOptions {
  appendUserMessage?: boolean;
  conversationIdOverride?: string | null;
  requestOptions?: ChatRequestOptions;
}

interface UseChatSessionsOptions {
  selectedModel: string;
  selectedModelOption: ModelOption | null;
  selectedReasoningMode: string;
  showAdvancedRequestOptionsButton: boolean;
}

const normalizeAssistantPhase = (
  phase: ChatStreamEventPayload['phase'] | undefined
): AssistantLine['phase'] => {
  if (phase === 'commentary' || phase === 'final_answer') {
    return phase;
  }
  return null;
};

const parseAdvancedRequestOptions = (raw: string): ChatRequestOptions => {
  const trimmed = raw.trim();
  if (!trimmed) {
    return {};
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch {
    throw new Error('高级请求参数不是合法 JSON。');
  }

  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error('高级请求参数必须是 JSON 对象。');
  }

  const source = parsed as Record<string, unknown>;
  const out: ChatRequestOptions = {};

  if (source.text !== undefined) {
    out.text = source.text;
  }

  if (source.include !== undefined) {
    if (!Array.isArray(source.include)) {
      throw new Error('`include` 必须是字符串数组。');
    }
    out.include = source.include
      .map((item) => `${item ?? ''}`.trim())
      .filter((item) => item.length > 0);
  }

  if (source.serviceTier !== undefined) {
    const value = `${source.serviceTier ?? ''}`.trim();
    if (value) {
      out.serviceTier = value;
    }
  }

  if (source.promptCacheKey !== undefined) {
    const value = `${source.promptCacheKey ?? ''}`.trim();
    if (value) {
      out.promptCacheKey = value;
    }
  }

  if (source.clientMetadata !== undefined) {
    if (
      !source.clientMetadata ||
      typeof source.clientMetadata !== 'object' ||
      Array.isArray(source.clientMetadata)
    ) {
      throw new Error('`clientMetadata` 必须是 JSON 对象。');
    }
    out.clientMetadata = source.clientMetadata as Record<string, unknown>;
  }

  if (source.generate !== undefined) {
    if (typeof source.generate !== 'boolean') {
      throw new Error('`generate` 必须是布尔值。');
    }
    out.generate = source.generate;
  }

  return out;
};

const parsePossiblyJson = (value: string | undefined): unknown => {
  if (!value) {
    return undefined;
  }

  try {
    return JSON.parse(value);
  } catch {
    return value;
  }
};

export function useChatSessions({
  selectedModel,
  selectedModelOption,
  selectedReasoningMode,
  showAdvancedRequestOptionsButton,
}: UseChatSessionsOptions) {
  const initialConversation = useMemo(() => createLocalConversation(), []);
  const [input, setInput] = useState('');
  const [advancedRequestOptionsInput, setAdvancedRequestOptionsInput] = useState('');
  const [advancedRequestOptionsError, setAdvancedRequestOptionsError] = useState<string | null>(
    null
  );
  const [attachment, setAttachment] = useState<ComposerAttachment | null>(null);
  const [conversations, setConversations] = useState<Conversation[]>(() => [initialConversation]);
  const [conversationToDelete, setConversationToDelete] = useState<Conversation | null>(null);
  const [activeConversationId, setActiveConversationId] = useState(
    initialConversation.conversationId
  );
  const [generatingConversationIds, setGeneratingConversationIds] = useState<Set<string>>(
    () => new Set()
  );
  const [stoppingConversationIds, setStoppingConversationIds] = useState<Set<string>>(
    () => new Set()
  );
  const [thinkingConversationIds, setThinkingConversationIds] = useState<Set<string>>(
    () => new Set()
  );

  const streamRequestMapRef = useRef<Record<string, StreamRequestMapping>>({});
  const streamListenerRef = useRef<(() => void) | null>(null);
  const activeConversationRequestRef = useRef<Record<string, string>>({});
  const stoppedRequestIdsRef = useRef<Set<string>>(new Set());

  const activeConversation = useMemo(
    () =>
      conversations.find(
        (conversation) => conversation.conversationId === activeConversationId
      ) || conversations[0],
    [activeConversationId, conversations]
  );

  const sidebarConversations = useMemo(
    () => conversations.filter(shouldShowConversationInSidebar),
    [conversations]
  );

  const activeChatTitle = useMemo(
    () => getConversationDisplayTitle(activeConversation),
    [activeConversation]
  );

  const activeConversationIsGenerating =
    !!activeConversation?.conversationId &&
    generatingConversationIds.has(activeConversation.conversationId);
  const activeConversationIsStopping =
    !!activeConversation?.conversationId &&
    stoppingConversationIds.has(activeConversation.conversationId);
  const activeConversationIsThinking =
    !!activeConversation?.conversationId &&
    thinkingConversationIds.has(activeConversation.conversationId);
  const hasAnyGenerating = generatingConversationIds.size > 0;
  const isEmptyConversation = (activeConversation?.lines?.length ?? 0) === 0;

  const markConversationGenerating = useCallback((conversationId: string, isGenerating: boolean) => {
    if (!conversationId) return;
    setGeneratingConversationIds((prev) => {
      const next = new Set(prev);
      if (isGenerating) {
        next.add(conversationId);
      } else {
        next.delete(conversationId);
      }
      return next;
    });
  }, []);

  const markConversationStopping = useCallback((conversationId: string, isStopping: boolean) => {
    if (!conversationId) return;
    setStoppingConversationIds((prev) => {
      const next = new Set(prev);
      if (isStopping) {
        next.add(conversationId);
      } else {
        next.delete(conversationId);
      }
      return next;
    });
  }, []);

  const markConversationThinking = useCallback((conversationId: string, isThinking: boolean) => {
    if (!conversationId) return;
    setThinkingConversationIds((prev) => {
      const next = new Set(prev);
      if (isThinking) {
        next.add(conversationId);
      } else {
        next.delete(conversationId);
      }
      return next;
    });
  }, []);

  const clearConversationRequestState = useCallback(
    (conversationId: string) => {
      if (!conversationId) return;

      const requestId = activeConversationRequestRef.current[conversationId];
      if (requestId) {
        delete activeConversationRequestRef.current[conversationId];
        stoppedRequestIdsRef.current.delete(requestId);
        delete streamRequestMapRef.current[requestId];
      }

      Object.entries(streamRequestMapRef.current).forEach(([candidateRequestId, mapping]) => {
        if (mapping?.conversationId === conversationId) {
          delete streamRequestMapRef.current[candidateRequestId];
          stoppedRequestIdsRef.current.delete(candidateRequestId);
        }
      });

      markConversationGenerating(conversationId, false);
      markConversationStopping(conversationId, false);
      markConversationThinking(conversationId, false);
    },
    [markConversationGenerating, markConversationStopping, markConversationThinking]
  );

  useEffect(() => {
    let mounted = true;

    invoke<ConversationSummary[]>('list_conversations')
      .then((storedConversations) => {
        if (!mounted || !Array.isArray(storedConversations) || storedConversations.length === 0) {
          return;
        }

        setConversations((prevConversations) =>
          mergeWithLocalDrafts(prevConversations, storedConversations)
        );
      })
      .catch(() => {
        // Keep local fallback conversation list when backend history is unavailable.
      });

    return () => {
      mounted = false;
    };
  }, []);

  useEffect(() => {
    const selectedConversation = conversations.find(
      (conversation) => conversation.conversationId === activeConversationId
    );
    if (!selectedConversation || selectedConversation.isLoaded) {
      return undefined;
    }

    let disposed = false;
    invoke<ConversationDetail>('load_conversation', {
      req: { conversationId: selectedConversation.conversationId },
    })
      .then((detail) => {
        if (disposed || !detail) return;

        setConversations((prevConversations) =>
          prevConversations.map((conversation) => {
            if (conversation.conversationId !== selectedConversation.conversationId) {
              return conversation;
            }

            const hydratedLines = hydrateConversationLines(detail.lines || []);

            return {
              ...conversation,
              title: detail.title || conversation.title,
              titleSource: detail.titleSource || conversation.titleSource,
              lastMessagePreview:
                hydratedLines.length > 0
                  ? (() => {
                      const last = hydratedLines[hydratedLines.length - 1];
                      return last.kind === 'assistant'
                        ? (last as AssistantLine).text
                        : last.kind === 'user'
                          ? (last as UserLine).text
                          : conversation.lastMessagePreview;
                    })()
                  : conversation.lastMessagePreview,
              messageCount: countVisibleMessages(hydratedLines),
              isLoaded: true,
              lines: hydratedLines,
            };
          })
        );
      })
      .catch(() => {});

    return () => {
      disposed = true;
    };
  }, [activeConversationId, conversations]);

  useEffect(() => {
    let disposed = false;

    const setup = async () => {
      const currentWindow = getCurrentWindow();
      const unlisten = await currentWindow.listen<ChatStreamEventPayload>(
        'chat_stream_event',
        (event) => {
          const payload = event?.payload || {};
          const conversationId = payload.conversationId;

          if (payload.kind === 'title' && conversationId && payload.conversationTitle) {
            setConversations((prevConversations) =>
              prevConversations.map((conversation) =>
                conversation.conversationId === conversationId
                  ? applyConversationTitle(conversation, payload.conversationTitle)
                  : conversation
              )
            );
            return;
          }

          const requestId = payload.requestId;
          const mapping = requestId ? streamRequestMapRef.current[requestId] : null;
          if (!mapping) return;
          if (conversationId && conversationId !== mapping.conversationId) return;

          const eventIndex = Number(payload.eventIndex || 0);
          if (eventIndex > 0) {
            const lastEventIndex = Number(mapping.lastEventIndex || 0);
            if (eventIndex <= lastEventIndex) {
              return;
            }
            mapping.lastEventIndex = eventIndex;
          }

          if (payload.kind === 'thinking') {
            markConversationThinking(mapping.conversationId, true);
            return;
          }

          if (payload.kind === 'output_started') {
            markConversationThinking(mapping.conversationId, false);
            return;
          }

          if (payload.kind === 'delta' && payload.delta) {
            markConversationThinking(mapping.conversationId, false);
            const deltaText = String(payload.delta);
            const phase = normalizeAssistantPhase(payload.phase);
            setConversations((prev) =>
              prev.map((c) => {
                if (c.conversationId !== mapping.conversationId) return c;
                const lines = [...c.lines];
                const last = lines[lines.length - 1];
                if (
                  last &&
                  last.kind === 'assistant' &&
                  (last as AssistantLine).status === 'draft' &&
                  (last as AssistantLine).requestId === requestId
                ) {
                  lines[lines.length - 1] = {
                    ...last,
                    phase: phase ?? (last as AssistantLine).phase,
                    text: String((last as AssistantLine).text) + deltaText,
                  } as AssistantLine;
                } else {
                  lines.push({
                    kind: 'assistant' as const,
                    id: `asst-${requestId}-${payload.eventIndex || Date.now()}`,
                    ts: Date.now(),
                    requestId: requestId || '',
                    responseId: '',
                    phase,
                    text: deltaText,
                    status: 'draft' as const,
                  });
                }
                return { ...c, lines };
              })
            );
            return;
          }

          if (payload.kind === 'assistant_message') {
            markConversationThinking(mapping.conversationId, false);
            const messageText = String(payload.delta || '');
            const phase = normalizeAssistantPhase(payload.phase);
            setConversations((prev) =>
              prev.map((c) => {
                if (c.conversationId !== mapping.conversationId) return c;
                const lines = [...c.lines];
                let updated = false;

                for (let i = lines.length - 1; i >= 0; i -= 1) {
                  const line = lines[i];
                  if (line.kind !== 'assistant') continue;
                  const assistant = line as AssistantLine;
                  if (assistant.requestId !== requestId || assistant.status !== 'draft') continue;
                  if (phase && assistant.phase && assistant.phase !== phase) continue;
                  lines[i] = {
                    ...assistant,
                    text: messageText || assistant.text,
                    responseId: payload.responseId || assistant.responseId,
                    phase: phase ?? assistant.phase,
                    status: 'done' as const,
                  };
                  updated = true;
                  break;
                }

                if (!updated) {
                  lines.push({
                    kind: 'assistant' as const,
                    id: `asst-${requestId}-${payload.eventIndex || Date.now()}`,
                    ts: Date.now(),
                    requestId: requestId || '',
                    responseId: payload.responseId || '',
                    phase,
                    text: messageText,
                    status: 'done' as const,
                  });
                }

                return {
                  ...c,
                  lines,
                  lastMessagePreview: messageText || c.lastMessagePreview,
                  messageCount: countVisibleMessages(lines),
                  isLoaded: true,
                };
              })
            );
            return;
          }

          if (payload.kind === 'tool_call_done') {
            markConversationThinking(mapping.conversationId, false);
            setConversations((prev) =>
              prev.map((c) => {
                if (c.conversationId !== mapping.conversationId) return c;
                return {
                  ...c,
                  lines: [
                    ...c.lines,
                    {
                      kind: 'tool' as const,
                      id: `tool-${requestId}-${payload.toolCallId || ''}`,
                      ts: Date.now(),
                      requestId: requestId || '',
                      callId: payload.toolCallId || '',
                      name: payload.toolName || '',
                      args: parsePossiblyJson(payload.toolArguments),
                      status: 'pending' as const,
                    },
                  ],
                };
              })
            );
            return;
          }

          if (payload.kind === 'tool_call_exec') {
            setConversations((prev) =>
              prev.map((c) => {
                if (c.conversationId !== mapping.conversationId) return c;
                const lines = c.lines.map((l) => {
                  if (l.kind === 'tool' && (l as ToolLine).callId === payload.toolCallId) {
                    const toolLine = l as ToolLine;
                    return {
                      ...toolLine,
                      output: parsePossiblyJson(payload.toolOutput),
                      status: 'done' as const,
                    } satisfies ToolLine;
                  }
                  return l;
                });
                return { ...c, lines };
              })
            );
            return;
          }

          if (payload.kind === 'tool_call_delta') {
            setConversations((prev) =>
              prev.map((c) => {
                if (c.conversationId !== mapping.conversationId) return c;
                const lines = c.lines.map((l) => {
                  if (l.kind === 'tool' && (l as ToolLine).callId === payload.toolCallId) {
                    const toolLine = l as ToolLine;
                    return {
                      ...toolLine,
                      args:
                        typeof toolLine.args === 'string'
                          ? toolLine.args + (payload.delta || '')
                          : toolLine.args,
                    } satisfies ToolLine;
                  }
                  return l;
                });
                return { ...c, lines };
              })
            );
            return;
          }

          if (payload.kind === 'done') {
            markConversationGenerating(mapping.conversationId, false);
            markConversationStopping(mapping.conversationId, false);
            markConversationThinking(mapping.conversationId, false);

            setConversations((prev) =>
              prev.map((c) => {
                if (c.conversationId !== mapping.conversationId) return c;
                const lines = c.lines.map((l) => {
                  if (
                    l.kind === 'assistant' &&
                    l.requestId === requestId &&
                    (l as AssistantLine).phase === 'final_answer'
                  ) {
                    return {
                      ...l,
                      responseId: payload.responseId || (l as AssistantLine).responseId,
                      status: 'done' as const,
                    } satisfies AssistantLine;
                  }
                  return l;
                });
                const finalText =
                  payload.delta && String(payload.delta).trim() ? String(payload.delta) : '';
                const hasAssistant = lines.some(
                  (l) =>
                    l.kind === 'assistant' &&
                    l.requestId === requestId &&
                    (l as AssistantLine).phase === 'final_answer'
                );
                const finalLines = hasAssistant
                  ? lines
                  : !finalText
                    ? lines
                    : [
                        ...lines,
                        {
                          kind: 'assistant' as const,
                          id: `asst-${requestId}-final`,
                          ts: Date.now(),
                          requestId: requestId || '',
                          responseId: payload.responseId || '',
                          phase: 'final_answer' as const,
                          text: finalText,
                          status: 'done' as const,
                        } satisfies AssistantLine,
                      ];
                return applyConversationTitle(
                  {
                    ...c,
                    lines: finalLines,
                    lastMessagePreview: finalText || c.lastMessagePreview,
                    messageCount: countVisibleMessages(finalLines),
                    isLoaded: true,
                  },
                  payload.conversationTitle
                );
              })
            );
          }
        }
      );

      if (disposed) {
        unlisten();
        return;
      }

      if (streamListenerRef.current) {
        streamListenerRef.current();
      }
      streamListenerRef.current = unlisten;
    };

    void setup();

    return () => {
      disposed = true;
      if (streamListenerRef.current) {
        streamListenerRef.current();
        streamListenerRef.current = null;
      }
    };
  }, [markConversationGenerating, markConversationStopping, markConversationThinking]);

  const sendMessage = useCallback(
    async (
      textToSend?: string,
      options: SendMessageOptions = {}
    ) => {
      const { appendUserMessage = true, conversationIdOverride = null, requestOptions } = options;
      const text = (textToSend ?? input).trim();
      if (!text || !activeConversation) return;

      let advancedRequestOptions: ChatRequestOptions = requestOptions || {};
      if (!requestOptions && showAdvancedRequestOptionsButton) {
        try {
          advancedRequestOptions = parseAdvancedRequestOptions(advancedRequestOptionsInput);
          if (advancedRequestOptionsError) {
            setAdvancedRequestOptionsError(null);
          }
        } catch (error) {
          const message =
            error instanceof Error ? error.message : '高级请求参数解析失败，请检查 JSON 格式。';
          setAdvancedRequestOptionsError(message);
          return;
        }
      } else if (!requestOptions && advancedRequestOptionsError) {
        setAdvancedRequestOptionsError(null);
      }

      if (appendUserMessage) {
        setInput('');
        setAttachment(null);
      }

      const currentConversationId =
        conversationIdOverride ?? activeConversation.conversationId;
      if (
        generatingConversationIds.has(currentConversationId) ||
        activeConversationRequestRef.current[currentConversationId]
      ) {
        return;
      }

      markConversationGenerating(currentConversationId, true);
      markConversationStopping(currentConversationId, false);
      markConversationThinking(currentConversationId, true);

      const requestId = `req-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      setConversations((prevConversations) =>
        prevConversations.map((conversation) => {
          if (conversation.conversationId !== currentConversationId) {
            return conversation;
          }

          const wasEmptyConversation = isConversationEmpty(conversation);
          let nextLines = [...conversation.lines];
          let nextTitle = conversation.title;

          if (appendUserMessage) {
            if (wasEmptyConversation) {
              nextTitle = buildDraftConversationTitle(text);
            }
            nextLines.push({
              kind: 'user' as const,
              id: `u-${requestId}`,
              ts: Date.now(),
              requestId,
              text,
            });
          }

          return {
            ...conversation,
            title: nextTitle,
            lastMessagePreview: text,
            messageCount: countVisibleMessages(nextLines),
            lines: nextLines,
            isLoaded: true,
          };
        })
      );

      streamRequestMapRef.current[requestId] = {
        conversationId: currentConversationId,
        lastEventIndex: 0,
      };
      activeConversationRequestRef.current[currentConversationId] = requestId;

      const selectedReasoningEffort =
        selectedModelOption?.profileKey === selectedModel &&
        selectedReasoningMode !== DEFAULT_REASONING_MODE
          ? selectedReasoningMode
          : null;

      try {
        const response = await invoke<ChatStreamResponse>('chat_stream', {
          req: {
            input: text,
            conversationId: currentConversationId,
            model: selectedModel,
            reasoningEffort: selectedReasoningEffort,
            text: advancedRequestOptions.text,
            include: advancedRequestOptions.include,
            serviceTier: advancedRequestOptions.serviceTier,
            promptCacheKey: advancedRequestOptions.promptCacheKey,
            clientMetadata: advancedRequestOptions.clientMetadata,
            generate: advancedRequestOptions.generate,
            requestId,
          },
        });
        const wasStopped = stoppedRequestIdsRef.current.has(requestId);

        setConversations((prevConversations) =>
          prevConversations.map((conversation) => {
            if (conversation.conversationId !== currentConversationId) {
              return conversation;
            }
            const lines = conversation.lines.map((line) => {
              if (
                line.kind === 'assistant' &&
                line.requestId === requestId &&
                (line as AssistantLine).status === 'draft'
              ) {
                return {
                  ...line,
                  text:
                    response.outputText ||
                    (line as AssistantLine).text ||
                    (wasStopped ? '已停止' : ''),
                  responseId: response.responseId || (line as AssistantLine).responseId,
                  phase: (line as AssistantLine).phase ?? 'final_answer',
                  status: 'done' as const,
                } satisfies AssistantLine;
              }
              return line;
            });
            return applyConversationTitle(
              {
                ...conversation,
                lines,
                lastMessagePreview: response.outputText || text,
                messageCount: countVisibleMessages(lines),
                isLoaded: true,
              },
              response.conversationTitle
            );
          })
        );
      } catch (error: unknown) {
        markConversationThinking(currentConversationId, false);
        const errorText =
          typeof error === 'string'
            ? error
            : '请求失败，请检查配置文件中的 credential / api_endpoint 和网络连接。';
        setConversations((prevConversations) =>
          prevConversations.map((conversation) => {
            if (conversation.conversationId !== currentConversationId) {
              return conversation;
            }
            const lines = conversation.lines.map((line) => {
              if (line.kind === 'assistant' && line.requestId === requestId) {
                return {
                  ...line,
                  text: '',
                  phase: (line as AssistantLine).phase,
                  status: 'done' as const,
                } satisfies AssistantLine;
              }
              return line;
            });
            return {
              ...conversation,
              lines,
              lastMessagePreview: errorText || text,
              messageCount: countVisibleMessages(lines),
            };
          })
        );
      } finally {
        delete streamRequestMapRef.current[requestId];
        stoppedRequestIdsRef.current.delete(requestId);
        if (activeConversationRequestRef.current[currentConversationId] === requestId) {
          delete activeConversationRequestRef.current[currentConversationId];
        }
        markConversationGenerating(currentConversationId, false);
        markConversationStopping(currentConversationId, false);
        markConversationThinking(currentConversationId, false);
      }
    },
    [
      activeConversation,
      advancedRequestOptionsError,
      advancedRequestOptionsInput,
      generatingConversationIds,
      input,
      markConversationGenerating,
      markConversationStopping,
      markConversationThinking,
      selectedModel,
      selectedModelOption,
      selectedReasoningMode,
      showAdvancedRequestOptionsButton,
    ]
  );

  const stopActiveStream = useCallback(async () => {
    const currentConversationId = activeConversation?.conversationId;
    if (!currentConversationId) return;

    const requestId = activeConversationRequestRef.current[currentConversationId];
    if (!requestId || stoppingConversationIds.has(currentConversationId)) return;

    stoppedRequestIdsRef.current.add(requestId);
    markConversationStopping(currentConversationId, true);

    try {
      await invoke('cancel_chat_stream', {
        req: { requestId },
      });
    } catch {
      markConversationStopping(currentConversationId, false);
    }
  }, [activeConversation, markConversationStopping, stoppingConversationIds]);

  const createNewChat = useCallback(() => {
    if (activeConversation && isConversationEmpty(activeConversation)) {
      setActiveConversationId(activeConversation.conversationId);
      return;
    }

    const newConversation = createLocalConversation();
    setConversations((prevConversations) => [newConversation, ...prevConversations]);
    setActiveConversationId(newConversation.conversationId);
  }, [activeConversation]);

  const renameConversation = useCallback(
    async (conversationId: string, title: string) => {
      const nextTitle = (title || '').trim();
      if (!nextTitle) return;

      const previousConversation = conversations.find(
        (conversation) => conversation.conversationId === conversationId
      );
      if (!previousConversation) return;

      setConversations((prevConversations) =>
        prevConversations.map((conversation) =>
          conversation.conversationId === conversationId
            ? { ...conversation, title: nextTitle, titleSource: 'manual' }
            : conversation
        )
      );

      try {
        const updatedSummary = await invoke<ConversationSummary>('rename_conversation', {
          req: {
            conversationId,
            title: nextTitle,
          },
        });

        if (updatedSummary?.title) {
          setConversations((prevConversations) =>
            prevConversations.map((conversation) =>
              conversation.conversationId === conversationId
                ? { ...conversation, title: updatedSummary.title || '', titleSource: 'manual' }
                : conversation
            )
          );
        }
      } catch {
        setConversations((prevConversations) =>
          prevConversations.map((conversation) =>
            conversation.conversationId === conversationId ? previousConversation : conversation
          )
        );
      }
    },
    [conversations]
  );

  const requestDeleteConversation = useCallback(
    (conversationId: string) => {
      const targetConversation = conversations.find(
        (conversation) => conversation.conversationId === conversationId
      );
      if (!targetConversation) return;
      setConversationToDelete(targetConversation);
    },
    [conversations]
  );

  const cancelDeleteConversation = useCallback(() => {
    setConversationToDelete(null);
  }, []);

  const confirmDeleteConversation = useCallback(async () => {
    if (!conversationToDelete) return;
    const conversationId = conversationToDelete.conversationId;
    const targetConversation = conversationToDelete;
    setConversationToDelete(null);
    clearConversationRequestState(conversationId);

    let rollbackConversation = targetConversation;
    let optimisticNextActiveId: string | null = null;

    setConversations((prevConversations) => {
      const currentTarget = prevConversations.find(
        (conversation) => conversation.conversationId === conversationId
      );
      if (!currentTarget) {
        return prevConversations;
      }

      rollbackConversation = currentTarget;
      const remainingConversations = prevConversations.filter(
        (conversation) => conversation.conversationId !== conversationId
      );

      if (remainingConversations.length > 0) {
        optimisticNextActiveId = remainingConversations[0].conversationId;
        return remainingConversations;
      }

      const fallbackConversation = createLocalConversation();
      optimisticNextActiveId = fallbackConversation.conversationId;
      return [fallbackConversation];
    });

    setActiveConversationId((prevActiveConversationId) =>
      prevActiveConversationId === conversationId
        ? optimisticNextActiveId || prevActiveConversationId
        : prevActiveConversationId
    );

    try {
      const deleted = await invoke<boolean>('delete_conversation', {
        req: { conversationId },
      });

      if (deleted === false) {
        console.warn('[delete_conversation] conversation file not found for id:', conversationId);
      }

      const storedConversations = await invoke<ConversationSummary[]>('list_conversations');
      if (Array.isArray(storedConversations)) {
        let nextConversationIds = new Set<string>();
        let fallbackActiveId: string | null = null;

        setConversations((prevConversations) => {
          const localDrafts = prevConversations.filter((conversation) =>
            isConversationEmpty(conversation)
          );

          const restoredConversations = storedConversations.map(restoreConversationPreview);
          const existingIds = new Set(
            restoredConversations.map((conversation) => conversation.conversationId)
          );
          const remainingLocalDrafts = localDrafts.filter(
            (conversation) => !existingIds.has(conversation.conversationId)
          );

          const nextConversations = [...remainingLocalDrafts, ...restoredConversations];
          const stableConversations = ensureAtLeastOneConversation(nextConversations);

          nextConversationIds = new Set(
            stableConversations.map((conversation) => conversation.conversationId)
          );
          fallbackActiveId = stableConversations[0].conversationId;
          return stableConversations;
        });

        setActiveConversationId((prevActiveConversationId) => {
          if (nextConversationIds.has(prevActiveConversationId)) {
            return prevActiveConversationId;
          }
          return fallbackActiveId || prevActiveConversationId;
        });
      }
    } catch {
      setConversations((prevConversations) => {
        const exists = prevConversations.some(
          (conversation) => conversation.conversationId === conversationId
        );
        if (exists) return prevConversations;
        return [rollbackConversation, ...prevConversations];
      });
      setActiveConversationId((prevActiveConversationId) =>
        prevActiveConversationId === optimisticNextActiveId
          ? conversationId
          : prevActiveConversationId
      );
      console.error('[delete_conversation] invoke failed for id:', conversationId);
    }
  }, [clearConversationRequestState, conversationToDelete]);

  const attachPlaceholderFile = useCallback(() => {
    setAttachment({
      name: 'screenshot_data.png',
      type: 'image',
    });
  }, []);

  const removeAttachment = useCallback(() => {
    setAttachment(null);
  }, []);

  const updateAdvancedRequestOptionsInput = useCallback(
    (value: string) => {
      setAdvancedRequestOptionsInput(value);
      if (advancedRequestOptionsError) {
        setAdvancedRequestOptionsError(null);
      }
    },
    [advancedRequestOptionsError]
  );

  return {
    activeChatTitle,
    activeConversation,
    activeConversationId,
    activeConversationIsGenerating,
    activeConversationIsStopping,
    activeConversationIsThinking,
    advancedRequestOptionsError,
    advancedRequestOptionsInput,
    attachment,
    attachPlaceholderFile,
    cancelDeleteConversation,
    confirmDeleteConversation,
    conversationToDelete,
    conversations,
    createNewChat,
    generatingConversationIds,
    hasAnyGenerating,
    input,
    isEmptyConversation,
    removeAttachment,
    renameConversation,
    requestDeleteConversation,
    sendMessage,
    setActiveConversationId,
    setInput,
    sidebarConversations,
    stopActiveStream,
    updateAdvancedRequestOptionsInput,
  };
}
