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
  hydrateConversationMessages,
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
  ChatStreamEventPayload,
  ChatStreamResponse,
  Conversation,
  ConversationDetail,
  ConversationSummary,
  ModelCatalogResponse,
  ModelOption,
  ToolCall,
} from './features/conversations/types';
import {
  buildFallbackModelOption,
  DEFAULT_MODEL_PROFILE,
  DEFAULT_REASONING_MODE,
  normalizeModelOption,
  resolveConfiguredDefaultOptionProfileKey,
} from './features/models/modelCatalog';
import type { SettingsSnapshotEvent } from './features/settings/types';
import { useComposerMeasurements } from './hooks/useComposerMeasurements';
import { useContextMenuGuard } from './hooks/useContextMenuGuard';
import { useTitlebarDragging } from './hooks/useTitlebarDragging';

interface ComposerAttachment {
  name: string;
  type: string;
}

interface StreamRequestMapping {
  conversationId: string;
  assistantMessageId: string;
  lastEventIndex: number;
}

const isModelOption = (option: ModelOption | null): option is ModelOption =>
  option !== null;

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
  const isEmptyConversation = (activeConversation?.messages?.length ?? 0) === 0;
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

            const hydratedMessages = hydrateConversationMessages(
              detail.messages || [],
              conversation.conversationId
            );

            return {
              ...conversation,
              title: detail.title || conversation.title,
              titleSource: detail.titleSource || conversation.titleSource,
              lastResponseId: detail.lastResponseId || null,
              lastMessagePreview:
                hydratedMessages[hydratedMessages.length - 1]?.text ||
                conversation.lastMessagePreview,
              messageCount:
                detail.messages?.length || countVisibleMessages(hydratedMessages),
              isLoaded: true,
              messages: hydratedMessages,
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
            setConversations((prevConversations) =>
              prevConversations.map((conversation) => {
                if (conversation.conversationId !== mapping.conversationId) return conversation;
                const nextMessages = conversation.messages.map((message) =>
                  message.id === mapping.assistantMessageId
                    ? { ...message, text: `${message.text || ''}${payload.delta}` }
                    : message
                );
                return { ...conversation, messages: nextMessages };
              })
            );
          }

          if (payload.kind === 'tool_call_started') {
            markConversationThinking(mapping.conversationId, false);
            setConversations((prevConversations) =>
              prevConversations.map((conversation) => {
                if (conversation.conversationId !== mapping.conversationId) return conversation;
                const nextMessages = conversation.messages.map((message) => {
                  if (message.id !== mapping.assistantMessageId) return message;
                  const toolCalls: ToolCall[] = Array.isArray(message.toolCalls)
                    ? [...message.toolCalls]
                    : [];
                  if (!toolCalls.some((tool) => tool.id === payload.toolCallId)) {
                    toolCalls.push({
                      id: payload.toolCallId || '',
                      name: payload.toolName || '',
                      arguments: '',
                      output: '',
                      status: 'started',
                    });
                  }
                  return { ...message, toolCalls };
                });
                return { ...conversation, messages: nextMessages };
              })
            );
            return;
          }

          if (payload.kind === 'tool_call_delta') {
            markConversationThinking(mapping.conversationId, false);
            setConversations((prevConversations) =>
              prevConversations.map((conversation) => {
                if (conversation.conversationId !== mapping.conversationId) return conversation;
                const nextMessages = conversation.messages.map((message) => {
                  if (message.id !== mapping.assistantMessageId) return message;
                  const toolCalls = Array.isArray(message.toolCalls)
                    ? message.toolCalls.map((tool) =>
                        tool.id === payload.toolCallId
                          ? { ...tool, arguments: `${tool.arguments || ''}${payload.delta || ''}` }
                          : tool
                      )
                    : [];
                  return { ...message, toolCalls };
                });
                return { ...conversation, messages: nextMessages };
              })
            );
            return;
          }

          if (payload.kind === 'tool_call_done') {
            setConversations((prevConversations) =>
              prevConversations.map((conversation) => {
                if (conversation.conversationId !== mapping.conversationId) return conversation;
                const nextMessages = conversation.messages.map((message) => {
                  if (message.id !== mapping.assistantMessageId) return message;
                  const toolCalls: ToolCall[] = Array.isArray(message.toolCalls)
                    ? message.toolCalls.map((tool) =>
                        tool.id === payload.toolCallId
                          ? {
                              ...tool,
                              status: 'arguments_done' as const,
                              arguments: payload.toolArguments || tool.arguments,
                            }
                          : tool
                      )
                    : [];
                  return { ...message, toolCalls };
                });
                return { ...conversation, messages: nextMessages };
              })
            );
            return;
          }

          if (payload.kind === 'tool_call_exec') {
            setConversations((prevConversations) =>
              prevConversations.map((conversation) => {
                if (conversation.conversationId !== mapping.conversationId) return conversation;
                const nextMessages = conversation.messages.map((message) => {
                  if (message.id !== mapping.assistantMessageId) return message;
                  const toolCalls: ToolCall[] = Array.isArray(message.toolCalls)
                    ? message.toolCalls.map((tool) =>
                        tool.id === payload.toolCallId
                          ? {
                              ...tool,
                              status: 'executed' as const,
                              output: payload.toolOutput || '',
                            }
                          : tool
                      )
                    : [];
                  return { ...message, toolCalls };
                });
                return { ...conversation, messages: nextMessages };
              })
            );
            return;
          }

          if (payload.kind === 'done') {
            markConversationGenerating(mapping.conversationId, false);
            markConversationStopping(mapping.conversationId, false);
            markConversationThinking(mapping.conversationId, false);

            setConversations((prevConversations) =>
              prevConversations.map((conversation) => {
                if (conversation.conversationId !== mapping.conversationId) return conversation;
                const nextMessages = conversation.messages.map((message) =>
                  message.id === mapping.assistantMessageId && typeof payload.delta === 'string'
                    ? {
                        ...message,
                        text: payload.delta,
                        status: 'done' as const,
                        errorText: '',
                        retryable: false,
                      }
                    : message
                );
                return applyConversationTitle(
                  {
                    ...conversation,
                    messages: nextMessages,
                    lastResponseId: payload.responseId || conversation.lastResponseId,
                    lastMessagePreview: payload.delta || conversation.lastMessagePreview,
                    messageCount: countVisibleMessages(nextMessages),
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
        () => {
          if (disposed) return;
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

    const assistantMessageId = targetAssistantMessageId || `m-a-${Date.now()}`;
    setConversations((prevConversations) =>
      prevConversations.map((conversation) => {
        if (conversation.conversationId !== currentConversationId) {
          return conversation;
        }

        const wasEmptyConversation = isConversationEmpty(conversation);
        let nextMessages = [...conversation.messages];
        let nextTitle = conversation.title;

        if (appendUserMessage && userMessage) {
          if (wasEmptyConversation) {
            nextTitle = buildDraftConversationTitle(text);
          }
          nextMessages.push(userMessage);
        }

        const nextAssistantMessage = {
          id: assistantMessageId,
          role: 'assistant' as const,
          text: '',
          status: 'streaming' as const,
          errorText: '',
          retryable: false,
          retryInput: text,
          retryConversationId: currentConversationId,
        };

        const hasTarget =
          targetAssistantMessageId &&
          nextMessages.some((message) => message.id === targetAssistantMessageId);

        if (hasTarget) {
          nextMessages = nextMessages.map((message) =>
            message.id === targetAssistantMessageId
              ? { ...message, ...nextAssistantMessage }
              : message
          );
        } else {
          nextMessages.push(nextAssistantMessage);
        }

        return {
          ...conversation,
          title: nextTitle,
          lastMessagePreview: text,
          messageCount: countVisibleMessages(nextMessages),
          messages: nextMessages,
          isLoaded: true,
        };
      })
    );

    const requestId = `req-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    streamRequestMapRef.current[requestId] = {
      conversationId: currentConversationId,
      assistantMessageId,
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
          requestId,
        },
      });
      const wasStopped = stoppedRequestIdsRef.current.has(requestId);

      setConversations((prevConversations) =>
        prevConversations.map((conversation) => {
          if (conversation.conversationId !== currentConversationId) {
            return conversation;
          }
          const messages = conversation.messages.map((message) =>
            message.id === assistantMessageId
              ? {
                  ...message,
                  text: response.outputText || message.text || (wasStopped ? '已停止' : ''),
                  status: 'done' as const,
                  errorText: '',
                  retryable: false,
                }
              : message
          );
          return applyConversationTitle(
            {
              ...conversation,
              messages,
              lastResponseId: response.responseId || null,
              lastMessagePreview: response.outputText || text,
              messageCount: countVisibleMessages(messages),
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
          const messages = conversation.messages.map((message) =>
            message.id === assistantMessageId
              ? {
                  ...message,
                  text: '',
                  status: 'failed' as const,
                  errorText,
                  retryable: true,
                }
              : message
          );
          return {
            ...conversation,
            messages,
            lastMessagePreview: text,
            messageCount: countVisibleMessages(messages),
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
    const failedMessage = (activeConversation.messages || []).find(
      (message) => message.id === assistantMessageId
    );
    if (!failedMessage?.retryable || !failedMessage?.retryInput) return;

    void handleSend(failedMessage.retryInput, {
      appendUserMessage: false,
      targetAssistantMessageId: assistantMessageId,
      conversationIdOverride:
        failedMessage.retryConversationId ?? activeConversation.conversationId,
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
            activeConversation?.messages?.length === 0 && !activeConversationIsGenerating
              ? 'opacity-100'
              : 'pointer-events-none opacity-0'
          }`}
        />
        <div
          className={`absolute left-1/2 top-1/2 h-[300px] w-[300px] -translate-x-1/2 -translate-y-1/2 rounded-full bg-gradient-to-tr from-indigo-950/25 to-purple-950/25 filter blur-[100px] transition-opacity duration-1000 ease-in-out ${
            activeConversation?.messages?.length > 0 && !activeConversationIsGenerating
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
                messages={activeConversation?.messages || []}
                isGenerating={activeConversationIsGenerating}
                isThinking={activeConversationIsThinking}
                onRetryMessage={handleRetryMessage}
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
