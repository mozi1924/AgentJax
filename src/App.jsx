import { useEffect, useMemo, useRef, useState } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';
import AppHeader from './components/AppHeader';
import Sidebar from './components/Sidebar';
import ChatArea from './components/ChatArea';
import ChatComposer from './components/ChatComposer';
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

function App() {
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [selectedModel, setSelectedModel] = useState('gpt-5-mini');
  const [modelOptions, setModelOptions] = useState([]);
  const [configPath, setConfigPath] = useState('');
  const [cachePath, setCachePath] = useState('');
  const [input, setInput] = useState('');
  const [isGenerating, setIsGenerating] = useState(false);
  const [isStopping, setIsStopping] = useState(false);
  const [attachment, setAttachment] = useState(null);
  const [conversations, setConversations] = useState(() => [createLocalConversation()]);
  const [activeConversationId, setActiveConversationId] = useState(
    () => conversations[0].conversationId
  );

  const titlebarRef = useRef(null);
  const streamRequestMapRef = useRef({});
  const streamListenerRef = useRef(null);
  const activeRequestIdRef = useRef(null);
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
        const available = Array.isArray(catalog.effectiveModels) && catalog.effectiveModels.length > 0
          ? catalog.effectiveModels
          : ['gpt-5-mini'];
        const configuredDefault = (catalog.defaultModel || '').trim();
        const nextModel = configuredDefault && available.includes(configuredDefault)
          ? configuredDefault
          : available[0];

        setModelOptions(available);
        setSelectedModel(nextModel);
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
        const requestId = payload.requestId;
        const mapping = requestId ? streamRequestMapRef.current[requestId] : null;
        if (!mapping) return;

        const eventIndex = Number(payload.eventIndex || 0);
        if (eventIndex > 0) {
          const lastEventIndex = Number(mapping.lastEventIndex || 0);
          if (eventIndex <= lastEventIndex) {
            return;
          }
          mapping.lastEventIndex = eventIndex;
        }

        if (payload.kind === 'delta' && payload.delta) {
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

        if (payload.kind === 'done') {
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
    if (!text || isGenerating || !activeConversation) return;

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

    setIsGenerating(true);

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
    activeRequestIdRef.current = requestId;

    try {
      const response = await invoke('chat_stream', {
        req: {
          input: text,
          conversationId: currentConversationId,
          model: selectedModel,
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
      if (activeRequestIdRef.current === requestId) {
        activeRequestIdRef.current = null;
      }
      setIsGenerating(false);
      setIsStopping(false);
    }
  };

  const handleRetryMessage = (assistantMessageId) => {
    if (isGenerating || !activeConversation) return;
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
    const requestId = activeRequestIdRef.current;
    if (!requestId || isStopping) return;

    stoppedRequestIdsRef.current.add(requestId);
    setIsStopping(true);

    try {
      await invoke('cancel_chat_stream', {
        req: { requestId }
      });
    } catch {
      setIsStopping(false);
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

  const handleDeleteConversation = async (conversationId) => {
    const targetConversation = conversations.find(
      (conversation) => conversation.conversationId === conversationId
    );
    if (!targetConversation) return;

    const confirmed = globalThis.confirm
      ? globalThis.confirm(`删除对话“${getConversationDisplayTitle(targetConversation)}”？此操作无法撤销。`)
      : true;
    if (!confirmed) return;

    const remainingConversations = conversations.filter(
      (conversation) => conversation.conversationId !== conversationId
    );
    const fallbackConversation = createLocalConversation();

    setConversations(
      remainingConversations.length > 0 ? remainingConversations : [fallbackConversation]
    );

    if (activeConversationId === conversationId) {
      setActiveConversationId(
        remainingConversations[0]?.conversationId || fallbackConversation.conversationId
      );
    }

    try {
      await invoke('delete_conversation', {
        req: { conversationId }
      });
    } catch {
      setConversations((prevConversations) => {
        const exists = prevConversations.some(
          (conversation) => conversation.conversationId === conversationId
        );
        if (exists) return prevConversations;
        return [targetConversation, ...prevConversations];
      });
      if (activeConversationId === conversationId) {
        setActiveConversationId(conversationId);
      }
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
    <div className="app-shell relative flex h-screen w-screen overflow-hidden bg-[#131314] font-sans text-slate-100 antialiased select-none">
      <Sidebar
        isOpen={sidebarOpen}
        conversations={sidebarConversations}
        activeConversationId={activeConversationId}
        onSelectConversation={setActiveConversationId}
        onNewChat={handleNewChat}
        onRenameConversation={handleRenameConversation}
        onDeleteConversation={handleDeleteConversation}
        isGenerating={isGenerating}
      />

      <AppHeader
        titlebarRef={titlebarRef}
        sidebarOpen={sidebarOpen}
        onToggleSidebar={() => setSidebarOpen((prev) => !prev)}
        selectedModel={selectedModel}
        modelOptions={modelOptions}
        onSelectModel={setSelectedModel}
        configPath={configPath}
        cachePath={cachePath}
      />

      <main
        className={`flex h-full flex-1 flex-col pt-12 transition-all duration-300 ${
          sidebarOpen ? 'pl-64' : 'pl-20'
        }`}
      >
        <ChatArea
          messages={activeConversation?.messages || []}
          isGenerating={isGenerating}
          onRetryMessage={handleRetryMessage}
          onSuggestionClick={handleSuggestionClick}
          activeChatTitle={activeChatTitle}
        />

        <ChatComposer
          input={input}
          onInputChange={setInput}
          attachment={attachment}
          onRemoveAttachment={() => setAttachment(null)}
          onAttachFile={handleAttachFile}
          isGenerating={isGenerating}
          isStopping={isStopping}
          onSend={() => handleSend()}
          onStop={handleStop}
        />
      </main>
    </div>
  );
}

export default App;
