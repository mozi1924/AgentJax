import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';
import AppHeader from './components/AppHeader';
import Sidebar from './components/Sidebar';
import ChatArea from './components/ChatArea';
import ChatComposer from './components/ChatComposer';
import ConfirmModal from './components/ConfirmModal';
import SettingsModal from './components/SettingsModal';
import {
  applyConversationTitle,
  buildDraftConversationTitle,
  canUseNativeContextMenu,
  createLocalConversation,
  getConversationDisplayTitle,
  hydrateConversationLines,
  isConversationEmpty,
  shouldShowConversationInSidebar,
} from './features/conversations/conversationUtils';
import {
  countVisibleMessages,
  ensureAtLeastOneConversation,
  mergeWithLocalDrafts,
  restoreConversationPreview,
} from './features/conversations/conversationState';
import type {
  AssistantLine,
  ChatRequestOptions,
  ChatStreamEventPayload,
  ChatStreamResponse,
  Conversation,
  ConversationDetail,
  ConversationLine,
  ConversationSummary,
  ModelCatalogResponse,
  ModelOption,
  ToolLine,
  UserLine,
} from './features/conversations/types';
import {
  buildFallbackModelOption,
  DEFAULT_MODEL_PROFILE,
  DEFAULT_REASONING_MODE,
  normalizeModelOption,
  resolveConfiguredDefaultOptionProfileKey,
} from './features/models/modelCatalog';
import type { SettingsSnapshot, SettingsSnapshotEvent } from './features/settings/types';
import { useComposerMeasurements } from './hooks/useComposerMeasurements';
import { useContextMenuGuard } from './hooks/useContextMenuGuard';
import { useTitlebarDragging } from './hooks/useTitlebarDragging';

interface ComposerAttachment {
  name: string;
  type: string;
}

interface StreamRequestMapping {
  conversationId: string;
  lastEventIndex: number;
}

const isModelOption = (option: ModelOption | null): option is ModelOption =>
  option !== null;

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

const resolveShowAdvancedRequestOptionsButton = (
  values: Record<string, unknown> | undefined
): boolean => values?.show_advanced_request_options === true;

export default function App() {
  const initialConversation = useMemo(() => createLocalConversation(), []);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [selectedModel, setSelectedModel] = useState(DEFAULT_MODEL_PROFILE);
  const [modelOptions, setModelOptions] = useState<ModelOption[]>([]);
  const [selectedReasoningMode, setSelectedReasoningMode] = useState(DEFAULT_REASONING_MODE);
  const [configPath, setConfigPath] = useState('');
  const [cachePath, setCachePath] = useState('');
  const [input, setInput] = useState('');
  const [showAdvancedRequestOptionsButton, setShowAdvancedRequestOptionsButton] = useState(false);
  const [advancedRequestOptionsInput, setAdvancedRequestOptionsInput] = useState('');
  const [advancedRequestOptionsError, setAdvancedRequestOptionsError] = useState<string | null>(
    null
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
  const [attachment, setAttachment] = useState<ComposerAttachment | null>(null);
  const [conversations, setConversations] = useState<Conversation[]>(() => [initialConversation]);
  const [conversationToDelete, setConversationToDelete] = useState<Conversation | null>(null);
  const [activeConversationId, setActiveConversationId] = useState(
    initialConversation.conversationId
  );

  const titlebarRef = useRef<HTMLDivElement | null>(null);
  const mainRef = useRef<HTMLDivElement | null>(null);
  const composerStageRef = useRef<HTMLDivElement | null>(null);
  const composerShellRef = useRef<HTMLDivElement | null>(null);
  const streamRequestMapRef = useRef<Record<string, StreamRequestMapping>>({});
  const streamListenerRef = useRef<(() => void) | null>(null);
  const activeConversationRequestRef = useRef<Record<string, string>>({});
  const stoppedRequestIdsRef = useRef<Set<string>>(new Set());
  const selectedModelRef = useRef(DEFAULT_MODEL_PROFILE);
  const selectedReasoningModeRef = useRef(DEFAULT_REASONING_MODE);

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

  const selectedModelOption = useMemo(
    () =>
      modelOptions.find((option) => option.profileKey === selectedModel) ||
      modelOptions[0] ||
      null,
    [modelOptions, selectedModel]
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
  const conversationViewKey = `${activeConversationId}-${isEmptyConversation ? 'empty' : 'messages'}`;

  const { composerHeight, emptyComposerOffset } = useComposerMeasurements({
    mainRef,
    composerStageRef,
    composerShellRef,
    attachment,
    input,
    isEmptyConversation,
  });

  useEffect(() => {
    selectedModelRef.current = selectedModel;
  }, [selectedModel]);

  useEffect(() => {
    selectedReasoningModeRef.current = selectedReasoningMode;
  }, [selectedReasoningMode]);

  const markConversationGenerating = (conversationId: string, isGenerating: boolean) => {
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
  };

  const markConversationStopping = (conversationId: string, isStopping: boolean) => {
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
  };

  const markConversationThinking = (conversationId: string, isThinking: boolean) => {
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
  };

  const clearConversationRequestState = (conversationId: string) => {
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
  };

  useTitlebarDragging(titlebarRef);
  useContextMenuGuard(canUseNativeContextMenu);

  const refreshModelCatalog = useCallback(async () => {
    const catalog = await invoke<ModelCatalogResponse>('get_model_catalog');
    if (!catalog) return;

    const available =
      Array.isArray(catalog.modelOptions) && catalog.modelOptions.length > 0
        ? catalog.modelOptions.map(normalizeModelOption).filter(isModelOption)
        : (
            Array.isArray(catalog.effectiveModels) && catalog.effectiveModels.length > 0
              ? catalog.effectiveModels
              : [DEFAULT_MODEL_PROFILE]
          ).map(buildFallbackModelOption);
    const configuredDefault = (catalog.defaultModel || '').trim();
    const configuredDefaultProfileKey = resolveConfiguredDefaultOptionProfileKey(
      configuredDefault,
      available
    );
    const preservedSelection = available.find(
      (option) => option.profileKey === selectedModelRef.current
    )?.profileKey;
    const nextModel =
      preservedSelection ||
      configuredDefaultProfileKey ||
      available[0]?.profileKey ||
      DEFAULT_MODEL_PROFILE;
    const nextModelOption =
      available.find((option) => option.profileKey === nextModel) || null;
    const preservedReasoning = selectedReasoningModeRef.current;
    const canPreserveReasoning =
      !!nextModelOption?.supportsReasoning &&
      preservedReasoning !== DEFAULT_REASONING_MODE &&
      nextModelOption.supportedReasoningLevels.includes(preservedReasoning);

    setModelOptions(available);
    setSelectedModel(nextModel);
    setSelectedReasoningMode(
      canPreserveReasoning
        ? preservedReasoning
        : nextModelOption?.configuredReasoningEffort || DEFAULT_REASONING_MODE
    );
    if (catalog.configPath) {
      setConfigPath(catalog.configPath);
    }
    if (catalog.cachePath) {
      setCachePath(catalog.cachePath);
    }
  }, []);

  useEffect(() => {
    let mounted = true;

    refreshModelCatalog().catch(() => {
      if (!mounted) {
        return;
      }
      // Keep frontend defaults when backend config cannot be loaded.
    });

    return () => {
      mounted = false;
    };
  }, [refreshModelCatalog]);

  useEffect(() => {
    let mounted = true;

    invoke<SettingsSnapshot>('get_settings_snapshot')
      .then((snapshot) => {
        if (!mounted) {
          return;
        }
        setShowAdvancedRequestOptionsButton(
          resolveShowAdvancedRequestOptionsButton(snapshot.values)
        );
      })
      .catch(() => {});

    return () => {
      mounted = false;
    };
  }, []);

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

            const hydratedLines = hydrateConversationLines(
              detail.lines || []
            );

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
              messageCount: detail.lines?.length || countVisibleMessages(hydratedLines),
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

          // ── Working markers ───────────────────────────────────────
          if (payload.kind === 'working_started') {
            markConversationThinking(mapping.conversationId, false);
            setConversations((prev) =>
              prev.map((c) => {
                if (c.conversationId !== mapping.conversationId) return c;
                return {
                  ...c,
                  lines: [
                    ...c.lines,
                    {
                      kind: 'working_start' as const,
                      id: `ws-${requestId}`,
                      ts: Date.now(),
                      requestId: requestId || '',
                    },
                  ],
                };
              })
            );
            return;
          }

          if (payload.kind === 'working_done') {
            setConversations((prev) =>
              prev.map((c) => {
                if (c.conversationId !== mapping.conversationId) return c;
                return {
                  ...c,
                  lines: [
                    ...c.lines,
                    {
                      kind: 'working_done' as const,
                      id: `wd-${requestId}`,
                      ts: Date.now(),
                      requestId: requestId || '',
                    },
                  ],
                };
              })
            );
            return;
          }

          // ── Text streaming ────────────────────────────────────────
          if (payload.kind === 'delta' && payload.delta) {
            markConversationThinking(mapping.conversationId, false);
            setConversations((prev) =>
              prev.map((c) => {
                if (c.conversationId !== mapping.conversationId) return c;
                const lines = [...c.lines];
                const last = lines[lines.length - 1];
                if (last && last.kind === 'assistant' && (last as AssistantLine).status === 'draft') {
                  lines[lines.length - 1] = {
                    ...last,
                    text: last.text + payload.delta,
                  } as AssistantLine;
                } else {
                  lines.push({
                    kind: 'assistant' as const,
                    id: `asst-${requestId}`,
                    ts: Date.now(),
                    requestId: requestId || '',
                    responseId: '',
                    text: payload.delta || '',
                    status: 'draft' as const,
                  });
                }
                return { ...c, lines };
              })
            );
            return;
          }

          // ── Tool call events ──────────────────────────────────────
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
                      args: payload.toolArguments
                        ? (() => { try { return JSON.parse(payload.toolArguments); } catch { return payload.toolArguments; } })()
                        : undefined,
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
                    const t = l as ToolLine;
                    return {
                      ...t,
                      output: payload.toolOutput
                        ? (() => { try { return JSON.parse(payload.toolOutput); } catch { return payload.toolOutput; } })()
                        : undefined,
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
                    const t = l as ToolLine;
                    return {
                      ...t,
                      args: typeof t.args === 'string'
                        ? t.args + (payload.delta || '')
                        : t.args,
                    } satisfies ToolLine;
                  }
                  return l;
                });
                return { ...c, lines };
              })
            );
            return;
          }

          // ── Done ──────────────────────────────────────────────────
          if (payload.kind === 'done') {
            markConversationGenerating(mapping.conversationId, false);
            markConversationStopping(mapping.conversationId, false);
            markConversationThinking(mapping.conversationId, false);

            setConversations((prev) =>
              prev.map((c) => {
                if (c.conversationId !== mapping.conversationId) return c;
                const lines = c.lines.map((l) => {
                  if (l.kind === 'assistant' && l.id === `asst-${requestId}`) {
                    return {
                      ...l,
                      text: typeof payload.delta === 'string' ? payload.delta : (l as AssistantLine).text,
                      responseId: payload.responseId || (l as AssistantLine).responseId,
                      status: 'done' as const,
                    } satisfies AssistantLine;
                  }
                  return l;
                });
                // If no assistant line exists yet (e.g. simple reply with no text),
                // create one with the final text.
                const hasAssistant = lines.some((l) => l.kind === 'assistant' && l.requestId === requestId);
                const finalLines = hasAssistant
                  ? lines
                  : [
                      ...lines,
                      {
                        kind: 'assistant' as const,
                        id: `asst-${requestId}`,
                        ts: Date.now(),
                        requestId: requestId || '',
                        responseId: payload.responseId || '',
                        text: typeof payload.delta === 'string' ? payload.delta : '',
                        status: 'done' as const,
                      } satisfies AssistantLine,
                    ];
                return applyConversationTitle(
                  {
                    ...c,
                    lines: finalLines,
                    lastMessagePreview: typeof payload.delta === 'string' ? payload.delta : c.lastMessagePreview,
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
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;

    const setup = async () => {
      const currentWindow = getCurrentWindow();
      unlisten = await currentWindow.listen<SettingsSnapshotEvent>(
        'config_snapshot_changed',
        (event) => {
          if (disposed) return;
          setShowAdvancedRequestOptionsButton(
            resolveShowAdvancedRequestOptionsButton(event.payload.values)
          );
          void refreshModelCatalog().catch(() => {});
        }
      );

      if (disposed && unlisten) {
        unlisten();
        unlisten = null;
      }
    };

    void setup();

    return () => {
      disposed = true;
      if (unlisten) {
        unlisten();
        unlisten = null;
      }
    };
  }, [refreshModelCatalog]);

  const handleSend = async (
    textToSend?: string,
    options: {
      appendUserMessage?: boolean;
      targetAssistantMessageId?: string | null;
      conversationIdOverride?: string | null;
    } = {}
  ) => {
    const {
      appendUserMessage = true,
      targetAssistantMessageId = null,
      conversationIdOverride = null,
    } = options;

    const text = (textToSend ?? input).trim();
    if (!text || !activeConversation) return;

    let advancedRequestOptions: ChatRequestOptions = {};
    if (showAdvancedRequestOptionsButton) {
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
    } else if (advancedRequestOptionsError) {
      setAdvancedRequestOptionsError(null);
    }

    if (appendUserMessage) {
      setInput('');
      setAttachment(null);
    }

    const userMessage = appendUserMessage
      ? {
          id: `m-u-${Date.now()}`,
          role: 'user' as const,
          text,
        }
      : null;

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
    markConversationThinking(currentConversationId, false);

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
          const lines = conversation.lines.map((l) => {
            if (l.kind === 'assistant' && l.requestId === requestId && (l as AssistantLine).status === 'draft') {
              return {
                ...l,
                text: response.outputText || (l as AssistantLine).text || (wasStopped ? '已停止' : ''),
                responseId: response.responseId || (l as AssistantLine).responseId,
                status: 'done' as const,
              } satisfies AssistantLine;
            }
            return l;
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
          const lines = conversation.lines.map((l) => {
            if (l.kind === 'assistant' && l.requestId === requestId) {
              return {
                ...l,
                text: '',
                status: 'done' as const,
              } satisfies AssistantLine;
            }
            return l;
          });
          return {
            ...conversation,
            lines,
            lastMessagePreview: text,
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
  };

  const handleSelectReasoningMode = (reasoningMode: string) => {
    setSelectedReasoningMode(reasoningMode || DEFAULT_REASONING_MODE);
  };

  const handleRetryMessage = (assistantMessageId: string) => {
    if (!activeConversation) return;
    if (generatingConversationIds.has(activeConversation.conversationId)) return;
    const lastUserLine = [...(activeConversation.lines || [])].reverse().find((l) => l.kind === 'user');
    if (!lastUserLine) return;

    void handleSend((lastUserLine as { text: string }).text, {
      appendUserMessage: false,
      targetAssistantMessageId: assistantMessageId,
      conversationIdOverride: activeConversation.conversationId,
    });
  };

  const handleStop = async () => {
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
  };

  const handleNewChat = () => {
    if (activeConversation && isConversationEmpty(activeConversation)) {
      setActiveConversationId(activeConversation.conversationId);
      return;
    }

    const newConversation = createLocalConversation();
    setConversations((prevConversations) => [newConversation, ...prevConversations]);
    setActiveConversationId(newConversation.conversationId);
  };

  const handleRenameConversation = async (conversationId: string, title: string) => {
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
  };

  const handleDeleteConversation = async (conversationId: string) => {
    const targetConversation = conversations.find(
      (conversation) => conversation.conversationId === conversationId
    );
    if (!targetConversation) return;
    setConversationToDelete(targetConversation);
  };

  const executeDeleteConversation = async () => {
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
  };

  const handleSuggestionClick = (text: string) => {
    void handleSend(text);
  };

  const handleAttachFile = () => {
    setAttachment({
      name: 'screenshot_data.png',
      type: 'image',
    });
  };

  return (
    <div className="app-shell relative flex h-screen w-screen overflow-hidden bg-transparent font-sans text-slate-100 antialiased select-none">
      <div className="absolute inset-0 -z-10 overflow-hidden bg-[#131314]">
        <div
          className={`absolute left-1/2 top-1/2 h-[550px] w-[550px] -translate-x-1/2 -translate-y-1/2 animate-pulse-fast rounded-full bg-gradient-to-tr from-cyan-500/25 via-purple-500/30 to-pink-500/25 filter blur-[80px] transition-opacity duration-1000 ease-in-out md:blur-[120px] ${
            hasAnyGenerating ? 'opacity-100' : 'pointer-events-none opacity-0'
          }`}
        />
        <div
          className={`absolute left-1/2 top-1/2 h-[500px] w-[500px] -translate-x-1/2 -translate-y-1/2 animate-pulse-slow rounded-full bg-gradient-to-tr from-blue-600/20 via-indigo-500/25 to-purple-600/20 filter blur-[80px] transition-opacity duration-1000 ease-in-out md:blur-[120px] ${
            activeConversation?.lines?.length === 0 && !activeConversationIsGenerating
              ? 'opacity-100'
              : 'pointer-events-none opacity-0'
          }`}
        />
        <div
          className={`absolute left-1/2 top-1/2 h-[300px] w-[300px] -translate-x-1/2 -translate-y-1/2 rounded-full bg-gradient-to-tr from-indigo-950/25 to-purple-950/25 filter blur-[100px] transition-opacity duration-1000 ease-in-out ${
            activeConversation?.lines?.length > 0 && !activeConversationIsGenerating
              ? 'opacity-100'
              : 'pointer-events-none opacity-0'
          }`}
        />
      </div>

      <Sidebar
        isOpen={sidebarOpen}
        conversations={sidebarConversations}
        activeConversationId={activeConversationId}
        onSelectConversation={setActiveConversationId}
        onNewChat={handleNewChat}
        onOpenSettings={() => setSettingsOpen(true)}
        onRenameConversation={handleRenameConversation}
        onDeleteConversation={handleDeleteConversation}
        generatingConversationIds={generatingConversationIds}
      />

      <AppHeader
        titlebarRef={titlebarRef}
        sidebarOpen={sidebarOpen}
        onToggleSidebar={() => setSidebarOpen((prev) => !prev)}
        selectedModel={selectedModel}
        selectedModelOption={selectedModelOption}
        modelOptions={modelOptions}
        onSelectModel={setSelectedModel}
        selectedReasoningMode={selectedReasoningMode}
        onSelectReasoningMode={handleSelectReasoningMode}
        configPath={configPath}
        cachePath={cachePath}
      />

      <main className="flex h-full flex-1 flex-col pt-12">
        <div
          ref={mainRef}
          className={`relative flex min-h-0 flex-1 flex-col transition-[margin] duration-300 ${
            sidebarOpen ? 'ml-64' : 'ml-20'
          }`}
        >
          <div
            className={`flex min-h-0 flex-1 flex-col overflow-hidden transition-opacity duration-300 ${
              isEmptyConversation ? 'pointer-events-none opacity-0' : 'opacity-100'
            }`}
            style={{ paddingBottom: isEmptyConversation ? 0 : `${composerHeight}px` }}
          >
            <div key={conversationViewKey} className="flex min-h-0 flex-1 animate-conversation-content-in">
              <ChatArea
                lines={activeConversation?.lines || []}
                isGenerating={activeConversationIsGenerating}
                isThinking={activeConversationIsThinking}
                activeChatTitle={activeChatTitle}
              />
            </div>
          </div>

          <div
            className={`pointer-events-none absolute inset-x-0 bottom-0 z-10 bg-[#131314] transition-opacity duration-700 ease-[cubic-bezier(0.22,1,0.36,1)] ${
              isEmptyConversation ? 'opacity-0' : 'opacity-100'
            }`}
            style={{ height: `${composerHeight}px` }}
          >
            <div className="pointer-events-none absolute top-0 left-0 right-0 h-10 -translate-y-full bg-gradient-to-t from-[#131314] to-transparent" />
          </div>

          <div
            ref={composerStageRef}
            className="pointer-events-none absolute inset-x-0 bottom-0 z-10 will-change-transform transition-transform duration-700 ease-[cubic-bezier(0.22,1,0.36,1)]"
            style={{
              transform: `translate3d(0, ${isEmptyConversation ? emptyComposerOffset : 0}px, 0)`,
            }}
          >
            <div className="flex w-full flex-col items-center">
              <div
                className={`mx-auto w-full max-w-3xl overflow-hidden px-4 text-center transition-[max-height,margin,opacity,transform] duration-200 ease-out md:px-8 lg:px-12 ${
                  isEmptyConversation
                    ? 'mb-6 max-h-40 translate-y-0 opacity-100 md:max-h-44'
                    : 'mb-0 max-h-0 -translate-y-2 opacity-0'
                }`}
              >
                <h1 className="text-4xl font-semibold tracking-tight md:text-5xl">
                  <span className="animate-gradient bg-gradient-to-r from-blue-400 via-purple-400 to-rose-400 bg-clip-text font-bold text-transparent">
                    Mozi,
                  </span>
                  <br />
                  <span className="text-[#444746] dark:text-[#e3e3e3]">想了解什么，尽管问吧！</span>
                </h1>
              </div>

              <div ref={composerShellRef} className="pointer-events-auto w-full">
                <ChatComposer
                  input={input}
                  onInputChange={setInput}
                  showAdvancedRequestOptionsButton={showAdvancedRequestOptionsButton}
                  advancedRequestOptionsInput={advancedRequestOptionsInput}
                  onAdvancedRequestOptionsInputChange={(value) => {
                    setAdvancedRequestOptionsInput(value);
                    if (advancedRequestOptionsError) {
                      setAdvancedRequestOptionsError(null);
                    }
                  }}
                  advancedRequestOptionsError={advancedRequestOptionsError}
                  attachment={attachment}
                  onRemoveAttachment={() => setAttachment(null)}
                  onAttachFile={handleAttachFile}
                  isGenerating={activeConversationIsGenerating}
                  isStopping={activeConversationIsStopping}
                  onSend={() => void handleSend()}
                  onStop={handleStop}
                />
              </div>
            </div>
          </div>
        </div>
      </main>

      {conversationToDelete && (
        <ConfirmModal
          title="删除对话"
          message={`确定要删除对话“${getConversationDisplayTitle(conversationToDelete)}”吗？此操作无法撤销。`}
          onConfirm={executeDeleteConversation}
          onCancel={() => setConversationToDelete(null)}
        />
      )}

      <SettingsModal isOpen={settingsOpen} onClose={() => setSettingsOpen(false)} />
    </div>
  );
}
