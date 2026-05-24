import { useEffect, useRef, useState } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';
import { Menu, ChevronDown, Image, X, Paperclip, Mic, Send, Square } from 'lucide-react';
import Sidebar from './components/Sidebar';
import ChatArea from './components/ChatArea';

function createConversationId() {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  return `conv-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

function createLocalConversation(conversationId = createConversationId()) {
  return {
    conversationId,
    title: '新对话',
    messages: [],
    lastResponseId: null,
    isLoaded: true
  };
}

function isConversationEmpty(conversation) {
  return Array.isArray(conversation?.messages) && conversation.messages.length === 0;
}

function canUseNativeContextMenu(target) {
  if (!(target instanceof HTMLElement)) {
    return false;
  }

  if (target.closest('input, textarea, [contenteditable="true"]')) {
    return true;
  }

  return Boolean(target.closest('[data-native-context-menu="true"]'));
}

function applyConversationTitle(conversation, nextTitle) {
  const title = (nextTitle || '').trim();
  if (!title) {
    return conversation;
  }

  return {
    ...conversation,
    title
  };
}

function App() {
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [selectedModel, setSelectedModel] = useState('gpt-5-mini');
  const [modelOptions, setModelOptions] = useState([]);
  const [modelDropdownOpen, setModelDropdownOpen] = useState(false);
  const [configPath, setConfigPath] = useState('');
  const [cachePath, setCachePath] = useState('');
  const [input, setInput] = useState('');
  const [isGenerating, setIsGenerating] = useState(false);
  const [isStopping, setIsStopping] = useState(false);
  const [attachment, setAttachment] = useState(null);

  const [conversations, setConversations] = useState([createLocalConversation()]);
  const [activeConversationId, setActiveConversationId] = useState(
    conversations[0].conversationId
  );
  const activeConversation =
    conversations.find((conversation) => conversation.conversationId === activeConversationId) ||
    conversations[0];

  const titlebarRef = useRef(null);
  const streamRequestMapRef = useRef({});
  const streamListenerRef = useRef(null);
  const activeRequestIdRef = useRef(null);
  const stoppedRequestIdsRef = useRef(new Set());
  const textareaRef = useRef(null);

  useEffect(() => {
    if (textareaRef.current) {
      textareaRef.current.style.height = 'auto';
      textareaRef.current.style.height = `${Math.min(textareaRef.current.scrollHeight, 180)}px`;
    }
  }, [input]);

  useEffect(() => {
    const titlebar = titlebarRef.current;
    if (!titlebar) return undefined;

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
        if (!mounted || !Array.isArray(storedConversations)) return;
        if (storedConversations.length === 0) return;

        const restoredConversations = storedConversations.map((conversation) => ({
          conversationId: conversation.conversationId,
          title: conversation.title || '历史会话',
          messages: [],
          lastResponseId: null,
          isLoaded: false
        }));

        setConversations((prevConversations) => {
          const localDrafts = prevConversations.filter((conversation) =>
            isConversationEmpty(conversation)
          );
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
    if (!selectedConversation || selectedConversation.isLoaded) return;

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
            return {
              ...conversation,
              title: detail.title || conversation.title,
              lastResponseId: detail.lastResponseId || null,
              isLoaded: true,
              messages: (detail.messages || []).map((message) => ({
                id: message.id,
                role: message.role,
                text: message.text || '',
                status: 'done',
                errorText: '',
                retryable: false
              }))
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
              return applyConversationTitle({
                ...conversation,
                messages: nextMessages,
                lastResponseId: payload.responseId || conversation.lastResponseId,
                isLoaded: true
              }, payload.conversationTitle);
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

        let nextTitle = conversation.title;
        let nextMessages = [...conversation.messages];

        if (appendUserMessage && userMessage) {
          if (nextMessages.length === 0 && !conversation.title.trim()) {
            nextTitle = text.length > 20 ? `${text.slice(0, 18)}...` : text;
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
          return applyConversationTitle({
            ...conversation,
            messages,
            lastResponseId: response.responseId || null,
            isLoaded: true
          }, response.conversationTitle);
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
          return { ...conversation, messages };
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
          ? { ...conversation, title: nextTitle }
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
              ? { ...conversation, title: updatedSummary.title }
              : conversation
          )
        );
      }
    } catch {
      setConversations((prevConversations) =>
        prevConversations.map((conversation) =>
          conversation.conversationId === conversationId
            ? { ...conversation, title: previousConversation.title }
            : conversation
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
      ? globalThis.confirm(`删除对话“${targetConversation.title}”？此操作无法撤销。`)
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
    <div className="app-shell relative flex h-screen w-screen overflow-hidden bg-[#131314] text-slate-100 antialiased font-sans select-none">
      <Sidebar
        isOpen={sidebarOpen}
        conversations={conversations}
        activeConversationId={activeConversationId}
        onSelectConversation={(conversationId) => setActiveConversationId(conversationId)}
        onNewChat={handleNewChat}
        onRenameConversation={handleRenameConversation}
        onDeleteConversation={handleDeleteConversation}
        isGenerating={isGenerating}
      />

      <div
        className="absolute inset-x-0 top-0 z-40 flex h-12 items-center border-b border-[#2d2f31]/40 bg-[#131314]/90 backdrop-blur pl-[84px]"
        ref={titlebarRef}
      >
        <button
          onClick={() => setSidebarOpen((prev) => !prev)}
          data-no-drag="true"
          className="flex h-7 w-7 items-center justify-center rounded-full text-slate-300 transition hover:bg-[#2d2f31] flex-shrink-0"
          title={sidebarOpen ? '收起菜单' : '展开菜单'}
        >
          <Menu className="h-4.5 w-4.5" />
        </button>

        <div className="flex min-w-0 flex-1 items-center gap-1 pr-6 ml-2">
          <div className="relative flex items-center gap-1.5" data-no-drag="true">
            <button
              onClick={() => setModelDropdownOpen(!modelDropdownOpen)}
              className="flex items-center gap-2 rounded-xl px-3 py-1 text-sm font-medium text-slate-300 transition hover:bg-[#2d2f31] whitespace-nowrap"
              title={
                configPath
                  ? `配置文件: ${configPath}${cachePath ? `\n模型缓存: ${cachePath}` : ''}`
                  : '模型配置'
              }
            >
              <span className="truncate">AgentJax {selectedModel}</span>
              <ChevronDown className="h-3 w-3 text-slate-400" />
            </button>

            {modelDropdownOpen && (
              <div className="absolute top-10 left-0 z-50 w-56 rounded-2xl border border-[#2d2f31] bg-[#1e1f20] p-2 shadow-2xl">
                {modelOptions.map((model, idx) => (
                  <button
                    key={model}
                    onClick={() => {
                      setSelectedModel(model);
                      setModelDropdownOpen(false);
                    }}
                    className={`flex w-full flex-col rounded-xl px-3 py-2 text-left transition hover:bg-[#2d2f31] ${idx > 0 ? 'mt-1' : ''}`}
                  >
                    <span className="text-sm font-medium text-slate-200">{model}</span>
                  </button>
                ))}
              </div>
            )}
          </div>

          <div className="h-full min-w-0 flex-1" />
        </div>
      </div>

      <main
        className={`flex h-full flex-col flex-1 pt-12 transition-all duration-300 ${
          sidebarOpen ? 'pl-64' : 'pl-20'
        }`}
      >
        <ChatArea
          messages={activeConversation?.messages || []}
          isGenerating={isGenerating}
          onRetryMessage={handleRetryMessage}
          onSuggestionClick={handleSuggestionClick}
          activeChatTitle={activeConversation?.title || '新对话'}
        />

        <div className="bg-[#131314] px-4 md:px-6 pb-6 pt-2">
          <div className="mx-auto flex max-w-3xl flex-col">
            <div className="relative flex flex-col rounded-3xl border border-[#2d2f31] bg-[#1e1f20] px-4 py-3 shadow-md transition duration-200 focus-within:border-[#3c4043] focus-within:ring-1 focus-within:ring-[#3c4043]/50">
              {attachment && (
                <div className="mb-2 flex items-center gap-2 self-start rounded-xl border border-[#2d2f31] bg-[#131314] p-1.5 pr-2.5">
                  <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-red-500/10 text-red-400">
                    <Image className="h-5 w-5" />
                  </div>
                  <span className="text-xs font-medium text-slate-300">{attachment.name}</span>
                  <button
                    onClick={() => setAttachment(null)}
                    className="ml-2 rounded-full p-0.5 text-slate-400 hover:bg-[#2d2f31] hover:text-slate-200"
                  >
                    <X className="h-3.5 w-3.5" />
                  </button>
                </div>
              )}

              <div className="flex items-center gap-3">
                <button
                  onClick={handleAttachFile}
                  className="rounded-full p-2 text-slate-400 transition hover:bg-[#2d2f31] hover:text-slate-200"
                  title="上传文件/图片"
                >
                  <Paperclip className="h-5.5 w-5.5" />
                </button>

                <textarea
                  ref={textareaRef}
                  value={input}
                  onChange={(e) => setInput(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' && !e.shiftKey) {
                      e.preventDefault();
                      if (isGenerating) {
                        handleStop();
                      } else {
                        handleSend();
                      }
                    }
                  }}
                  placeholder="问问 AgentJax"
                  rows={1}
                  data-native-context-menu="true"
                  className="max-h-[180px] flex-1 resize-none bg-transparent py-1.5 text-sm text-slate-200 placeholder-slate-500 scrollbar-thin focus:outline-none select-text"
                />

                <div className="flex items-center gap-2">
                  <button
                    className="rounded-full p-2 text-slate-400 transition hover:bg-[#2d2f31] hover:text-slate-200"
                    title="语音输入"
                  >
                    <Mic className="h-5.5 w-5.5" />
                  </button>

                  <button
                    onClick={() => {
                      if (isGenerating) {
                        handleStop();
                      } else {
                        handleSend();
                      }
                    }}
                    disabled={isGenerating ? isStopping : !input.trim()}
                    className={`rounded-full p-2 transition ${
                      isGenerating
                        ? 'bg-red-500/90 text-white hover:bg-red-500 shadow shadow-red-500/20'
                        : input.trim()
                          ? 'bg-gradient-to-tr from-cyan-400 via-purple-500 to-pink-500 text-white hover:opacity-90 cursor-pointer shadow shadow-purple-500/20'
                          : 'bg-transparent text-slate-600'
                    }`}
                    title={isGenerating ? '停止生成' : '发送消息'}
                  >
                    {isGenerating ? <Square className="h-4.5 w-4.5 fill-current" /> : <Send className="h-4.5 w-4.5" />}
                  </button>
                </div>
              </div>
            </div>
          </div>
        </div>
      </main>
    </div>
  );
}

export default App;
