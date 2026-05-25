import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';
import AppHeader from './components/AppHeader';
import Sidebar from './components/Sidebar';
import ChatArea from './components/ChatArea';
import ChatComposer from './components/ChatComposer';
import ConfirmModal from './components/ConfirmModal';
import {
  applyConversationTitle,
  buildDraftConversationTitle,
  canUseNativeContextMenu,
  createLocalConversation,
  getConversationDisplayTitle,
  hydrateConversationMessages,
  isConversationEmpty,
  shouldShowConversationInSidebar
} from './features/conversations/conversationUtils';

const DEFAULT_MODEL_PROFILE = 'gpt-5-mini';
const DEFAULT_REASONING_MODE = '__default__';

const buildFallbackModelOption = (profileKey) => ({
  profileKey,
  providerKey: '',
  modelId: profileKey,
  supportsReasoning: false,
  supportedReasoningLevels: [],
  configuredReasoningEffort: null
});

const normalizeModelOption = (option) => {
  const profileKey = (option?.profileKey || option?.modelId || '').trim();
  if (!profileKey) {
    return null;
  }

  const configuredReasoningEffort = (option?.configuredReasoningEffort || '').trim().toLowerCase();
  return {
    profileKey,
    providerKey: (option?.providerKey || '').trim(),
    modelId: (option?.modelId || profileKey).trim(),
    supportsReasoning: !!option?.supportsReasoning,
    supportedReasoningLevels: Array.isArray(option?.supportedReasoningLevels)
      ? option.supportedReasoningLevels
        .map((level) => `${level || ''}`.trim().toLowerCase())
        .filter(Boolean)
      : [],
    configuredReasoningEffort: configuredReasoningEffort || null
  };
};

function App() {
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [selectedModel, setSelectedModel] = useState(DEFAULT_MODEL_PROFILE);
  const [modelOptions, setModelOptions] = useState([]);
  const [selectedReasoningMode, setSelectedReasoningMode] = useState(DEFAULT_REASONING_MODE);
  const [configPath, setConfigPath] = useState('');
  const [cachePath, setCachePath] = useState('');
  const [input, setInput] = useState('');
  const [generatingConversationIds, setGeneratingConversationIds] = useState(() => new Set());
  const [stoppingConversationIds, setStoppingConversationIds] = useState(() => new Set());
  const [thinkingConversationIds, setThinkingConversationIds] = useState(() => new Set());
  const [attachment, setAttachment] = useState(null);
  const [composerHeight, setComposerHeight] = useState(0);
  const [emptyComposerOffset, setEmptyComposerOffset] = useState(0);
  const [conversations, setConversations] = useState(() => [createLocalConversation()]);
  const [conversationToDelete, setConversationToDelete] = useState(null);
  const [activeConversationId, setActiveConversationId] = useState(
    () => conversations[0].conversationId
  );

  const titlebarRef = useRef(null);
  const mainRef = useRef(null);
  const composerStageRef = useRef(null);
  const composerShellRef = useRef(null);
  const streamRequestMapRef = useRef({});
  const streamListenerRef = useRef(null);
  const activeConversationRequestRef = useRef({});
  const stoppedRequestIdsRef = useRef(new Set());

  const activeConversation = useMemo(
    () =>
      conversations.find((conversation) => conversation.conversationId === activeConversationId) ||
      conversations[0],
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

  const markConversationGenerating = (conversationId, isGenerating) => {
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

  const markConversationStopping = (conversationId, isStopping) => {
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

  const markConversationThinking = (conversationId, isThinking) => {
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

  const clearConversationRequestState = (conversationId) => {
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

  useEffect(() => {
    const titlebar = titlebarRef.current;
    if (!titlebar) {
      return undefined;
    }

    const appWindow = getCurrentWindow();
    const handleMouseDown = async (event) => {
      if (event.buttons !== 1) return;
      if (event.target.closest('[data-no-drag="true"]')) return;
      await appWindow.startDragging();
    };

    titlebar.addEventListener('mousedown', handleMouseDown);
    return () => {
      titlebar.removeEventListener('mousedown', handleMouseDown);
    };
  }, []);

  useLayoutEffect(() => {
    const mainElement = mainRef.current;
    const composerStageElement = composerStageRef.current;
    const composerShellElement = composerShellRef.current;
    if (!mainElement || !composerStageElement || !composerShellElement) {
      return undefined;
    }

    const updateMeasurements = () => {
      const mainBounds = mainElement.getBoundingClientRect();
      const composerBounds = composerShellElement.getBoundingClientRect();
      const stageBounds = composerStageElement.getBoundingClientRect();

      const nextComposerHeight = composerBounds.height;
      const centeredTop = Math.max(0, (mainBounds.height - stageBounds.height) / 2);
      const dockedTop = Math.max(0, mainBounds.height - stageBounds.height);
      const nextEmptyOffset = centeredTop - dockedTop;

      setComposerHeight((previousHeight) =>
        Math.abs(previousHeight - nextComposerHeight) > 0.5 ? nextComposerHeight : previousHeight
      );
      setEmptyComposerOffset((previousOffset) =>
        Math.abs(previousOffset - nextEmptyOffset) > 0.5 ? nextEmptyOffset : previousOffset
      );
    };

    updateMeasurements();

    const resizeObserver = new ResizeObserver(() => {
      updateMeasurements();
    });

    resizeObserver.observe(mainElement);
    resizeObserver.observe(composerStageElement);
    resizeObserver.observe(composerShellElement);

    return () => {
      resizeObserver.disconnect();
    };
  }, [attachment, input, isEmptyConversation]);

  useEffect(() => {
    const handleContextMenu = (event) => {
      if (event.defaultPrevented) {
        return;
      }

      if (canUseNativeContextMenu(event.target)) {
        return;
      }

      event.preventDefault();
    };

    document.addEventListener('contextmenu', handleContextMenu);
    return () => {
      document.removeEventListener('contextmenu', handleContextMenu);
    };
  }, []);

  useEffect(() => {
    let mounted = true;

    invoke('get_model_catalog')
      .then((catalog) => {
        if (!mounted || !catalog) return;
        const available = Array.isArray(catalog.modelOptions) && catalog.modelOptions.length > 0
          ? catalog.modelOptions.map(normalizeModelOption).filter(Boolean)
          : (
            Array.isArray(catalog.effectiveModels) && catalog.effectiveModels.length > 0
              ? catalog.effectiveModels
              : [DEFAULT_MODEL_PROFILE]
          ).map(buildFallbackModelOption);
        const availableProfileKeys = available.map((option) => option.profileKey);
        const configuredDefault = (catalog.defaultModel || '').trim();
        const nextModel = configuredDefault && availableProfileKeys.includes(configuredDefault)
          ? configuredDefault
          : available[0]?.profileKey || DEFAULT_MODEL_PROFILE;

        setModelOptions(available);
        setSelectedModel(nextModel);
        const nextModelOption = available.find((option) => option.profileKey === nextModel);
        setSelectedReasoningMode(nextModelOption?.configuredReasoningEffort || DEFAULT_REASONING_MODE);
        if (catalog.configPath) {
          setConfigPath(catalog.configPath);
        }
        if (catalog.cachePath) {
          setCachePath(catalog.cachePath);
        }
      })
      .catch(() => {
        // Keep frontend defaults when backend config cannot be loaded.
      });

    return () => {
      mounted = false;
    };
  }, []);

  useEffect(() => {
    let mounted = true;

    invoke('list_conversations')
      .then((storedConversations) => {
        if (!mounted || !Array.isArray(storedConversations) || storedConversations.length === 0) {
          return;
        }

        const restoredConversations = storedConversations.map((conversation) => ({
          conversationId: conversation.conversationId,
          title: conversation.title || '',
          titleSource: conversation.titleSource || 'stored',
          messages: [],
          lastResponseId: null,
          lastMessagePreview: conversation.lastMessagePreview || '',
          messageCount: conversation.messageCount || 0,
          isLoaded: false
        }));

        setConversations((prevConversations) => {
          const localDrafts = prevConversations.filter((conversation) => isConversationEmpty(conversation));
          return [...localDrafts, ...restoredConversations];
        });
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
    invoke('load_conversation', {
      req: { conversationId: selectedConversation.conversationId }
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
                hydratedMessages[hydratedMessages.length - 1]?.text || conversation.lastMessagePreview,
              messageCount: detail.messages?.length || hydratedMessages.filter((message) => message.text).length,
              isLoaded: true,
              messages: hydratedMessages
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
      const unlisten = await currentWindow.listen('chat_stream_event', (event) => {
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
                const toolCalls = Array.isArray(message.toolCalls) ? [...message.toolCalls] : [];
                if (!toolCalls.some((t) => t.id === payload.toolCallId)) {
                  toolCalls.push({
                    id: payload.toolCallId,
                    name: payload.toolName,
                    arguments: '',
                    output: '',
                    status: 'started'
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
                  ? message.toolCalls.map((t) =>
                      t.id === payload.toolCallId
                        ? { ...t, arguments: `${t.arguments || ''}${payload.delta || ''}` }
                        : t
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
                const toolCalls = Array.isArray(message.toolCalls)
                  ? message.toolCalls.map((t) =>
                      t.id === payload.toolCallId
                        ? {
                            ...t,
                            status: 'arguments_done',
                            arguments: payload.toolArguments || t.arguments
                          }
                        : t
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
                const toolCalls = Array.isArray(message.toolCalls)
                  ? message.toolCalls.map((t) =>
                      t.id === payload.toolCallId
                        ? { ...t, status: 'executed', output: payload.toolOutput || '' }
                        : t
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
                      status: 'done',
                      errorText: '',
                      retryable: false
                    }
                  : message
              );
              return applyConversationTitle(
                {
                  ...conversation,
                  messages: nextMessages,
                  lastResponseId: payload.responseId || conversation.lastResponseId,
                  lastMessagePreview: payload.delta || conversation.lastMessagePreview,
                  messageCount: nextMessages.filter((message) => message.role === 'user' || message.text).length,
                  isLoaded: true
                },
                payload.conversationTitle
              );
            })
          );
        }
      });

      if (disposed) {
        unlisten();
        return;
      }

      if (streamListenerRef.current) {
        streamListenerRef.current();
      }
      streamListenerRef.current = unlisten;
    };

    setup();

    return () => {
      disposed = true;
      if (streamListenerRef.current) {
        streamListenerRef.current();
        streamListenerRef.current = null;
      }
    };
  }, []);

  const handleSend = async (textToSend, options = {}) => {
    const {
      appendUserMessage = true,
      targetAssistantMessageId = null,
      conversationIdOverride = null
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
          role: 'user',
          text
        }
      : null;

    const currentConversationId = conversationIdOverride ?? activeConversation.conversationId;
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
          role: 'assistant',
          text: '',
          status: 'streaming',
          errorText: '',
          retryable: false,
          retryInput: text,
          retryConversationId: currentConversationId
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
          messageCount: nextMessages.filter((message) => message.role === 'user' || message.text).length,
          messages: nextMessages,
          isLoaded: true
        };
      })
    );

    const requestId = `req-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    streamRequestMapRef.current[requestId] = {
      conversationId: currentConversationId,
      assistantMessageId,
      lastEventIndex: 0
    };
    activeConversationRequestRef.current[currentConversationId] = requestId;

    const selectedReasoningEffort =
      selectedModelOption?.profileKey === selectedModel &&
      selectedReasoningMode !== DEFAULT_REASONING_MODE
        ? selectedReasoningMode
        : null;

    try {
      const response = await invoke('chat_stream', {
        req: {
          input: text,
          conversationId: currentConversationId,
          model: selectedModel,
          reasoningEffort: selectedReasoningEffort,
          requestId
        }
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
                  status: 'done',
                  errorText: '',
                  retryable: false
                }
              : message
          );
          return applyConversationTitle(
            {
              ...conversation,
              messages,
              lastResponseId: response.responseId || null,
              lastMessagePreview: response.outputText || text,
              messageCount: messages.filter((message) => message.role === 'user' || message.text).length,
              isLoaded: true
            },
            response.conversationTitle
          );
        })
      );
    } catch (err) {
      markConversationThinking(currentConversationId, false);
      const errorText = typeof err === 'string'
        ? err
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
                  status: 'failed',
                  errorText,
                  retryable: true
                }
              : message
          );
          return {
            ...conversation,
            messages,
            lastMessagePreview: text,
            messageCount: messages.filter((message) => message.role === 'user' || message.text).length
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

  const handleSelectReasoningMode = (reasoningMode) => {
    setSelectedReasoningMode(reasoningMode || DEFAULT_REASONING_MODE);
  };

  const handleRetryMessage = (assistantMessageId) => {
    if (!activeConversation) return;
    if (generatingConversationIds.has(activeConversation.conversationId)) return;
    const failedMessage = (activeConversation.messages || []).find(
      (message) => message.id === assistantMessageId
    );
    if (!failedMessage?.retryable || !failedMessage?.retryInput) return;

    handleSend(failedMessage.retryInput, {
      appendUserMessage: false,
      targetAssistantMessageId: assistantMessageId,
      conversationIdOverride:
        failedMessage.retryConversationId ?? activeConversation.conversationId
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
        req: { requestId }
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

  const handleRenameConversation = async (conversationId, title) => {
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
      const updatedSummary = await invoke('rename_conversation', {
        req: {
          conversationId,
          title: nextTitle
        }
      });

      if (updatedSummary?.title) {
        setConversations((prevConversations) =>
          prevConversations.map((conversation) =>
            conversation.conversationId === conversationId
              ? { ...conversation, title: updatedSummary.title, titleSource: 'manual' }
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

  const handleDeleteConversation = (conversationId) => {
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
    let optimisticNextActiveId = null;

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
      const deleted = await invoke('delete_conversation', {
        req: { conversationId }
      });

      if (deleted === false) {
        console.warn('[delete_conversation] conversation file not found for id:', conversationId);
      }

      const storedConversations = await invoke('list_conversations');
      if (Array.isArray(storedConversations)) {
        let nextConversationIds = new Set();
        let fallbackActiveId = null;

        setConversations((prevConversations) => {
          const localDrafts = prevConversations.filter((conversation) =>
            isConversationEmpty(conversation)
          );

          const restoredConversations = storedConversations.map((conversation) => ({
            conversationId: conversation.conversationId,
            title: conversation.title || '',
            titleSource: conversation.titleSource || 'stored',
            messages: [],
            lastResponseId: null,
            lastMessagePreview: conversation.lastMessagePreview || '',
            messageCount: conversation.messageCount || 0,
            isLoaded: false
          }));

          const existingIds = new Set(restoredConversations.map((conversation) => conversation.conversationId));
          const remainingLocalDrafts = localDrafts.filter(
            (conversation) => !existingIds.has(conversation.conversationId)
          );

          const nextConversations = [...remainingLocalDrafts, ...restoredConversations];
          if (nextConversations.length === 0) {
            const fallbackConversation = createLocalConversation();
            nextConversationIds = new Set([fallbackConversation.conversationId]);
            fallbackActiveId = fallbackConversation.conversationId;
            return [fallbackConversation];
          }

          nextConversationIds = new Set(
            nextConversations.map((conversation) => conversation.conversationId)
          );
          fallbackActiveId = nextConversations[0].conversationId;
          return nextConversations;
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

  const handleSuggestionClick = (text) => {
    handleSend(text);
  };

  const handleAttachFile = () => {
    setAttachment({
      name: 'screenshot_data.png',
      type: 'image'
    });
  };

  return (
    <div className="app-shell relative flex h-screen w-screen overflow-hidden font-sans text-slate-100 antialiased select-none bg-transparent">
      {/* Animated Glowing Background */}
      <div className="absolute inset-0 -z-10 overflow-hidden bg-[#131314]">
        {/* Layer 1: Model Outputting Glow */}
        <div
          className={`absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 rounded-full filter blur-[80px] md:blur-[120px] bg-gradient-to-tr from-cyan-500/25 via-purple-500/30 to-pink-500/25 animate-pulse-fast w-[550px] h-[550px] transition-opacity duration-1000 ease-in-out ${
            hasAnyGenerating ? 'opacity-100' : 'opacity-0 pointer-events-none'
          }`}
        />
        {/* Layer 2: New Chat Welcome Glow */}
        <div
          className={`absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 rounded-full filter blur-[80px] md:blur-[120px] bg-gradient-to-tr from-blue-600/20 via-indigo-500/25 to-purple-600/20 animate-pulse-slow w-[500px] h-[500px] transition-opacity duration-1000 ease-in-out ${
            activeConversation?.messages?.length === 0 && !activeConversationIsGenerating ? 'opacity-100' : 'opacity-0 pointer-events-none'
          }`}
        />
        {/* Layer 3: Active Chat Faint Glow */}
        <div
          className={`absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 rounded-full filter blur-[100px] bg-gradient-to-tr from-indigo-950/25 to-purple-950/25 w-[300px] h-[300px] transition-opacity duration-1000 ease-in-out ${
            activeConversation?.messages?.length > 0 && !activeConversationIsGenerating ? 'opacity-100' : 'opacity-0 pointer-events-none'
          }`}
        />
      </div>

      <Sidebar
        isOpen={sidebarOpen}
        conversations={sidebarConversations}
        activeConversationId={activeConversationId}
        onSelectConversation={setActiveConversationId}
        onNewChat={handleNewChat}
        onRenameConversation={handleRenameConversation}
        onDeleteConversation={handleDeleteConversation}
        generatingConversationIds={generatingConversationIds}
        onToggleSidebar={() => setSidebarOpen((prev) => !prev)}
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

      <main
        className="flex h-full flex-1 flex-col pt-12"
      >
        <div
          ref={mainRef}
          className={`relative flex min-h-0 flex-1 flex-col transition-[margin] duration-300 ${
            sidebarOpen ? 'ml-64' : 'ml-20'
          }`}
        >
          {/* Chat Area Container */}
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
                onSuggestionClick={handleSuggestionClick}
                activeChatTitle={activeChatTitle}
              />
            </div>
          </div>

          {/* Bottom background mask for scrollable messages */}
          <div
            className={`absolute bottom-0 inset-x-0 bg-[#131314] z-10 pointer-events-none transition-opacity duration-700 ease-[cubic-bezier(0.22,1,0.36,1)] ${
              isEmptyConversation ? 'opacity-0' : 'opacity-100'
            }`}
            style={{ height: `${composerHeight}px` }}
          >
            <div className="absolute top-0 left-0 right-0 h-10 -translate-y-full bg-gradient-to-t from-[#131314] to-transparent pointer-events-none" />
          </div>

          <div
            ref={composerStageRef}
            className="pointer-events-none absolute inset-x-0 bottom-0 z-10 transition-transform duration-700 ease-[cubic-bezier(0.22,1,0.36,1)] will-change-transform"
            style={{
              transform: `translate3d(0, ${isEmptyConversation ? emptyComposerOffset : 0}px, 0)`
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
                  onSend={() => handleSend()}
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
    </div>
  );
}

export default App;
