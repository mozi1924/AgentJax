import { useState, useRef, useEffect } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';
import { Menu, ChevronDown, Image, X, Paperclip, Mic, Send, Square } from 'lucide-react';
import Sidebar from './components/Sidebar';
import ChatArea from './components/ChatArea';

function App() {
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [selectedModel, setSelectedModel] = useState('gpt-5-mini');
  const [modelOptions, setModelOptions] = useState([]);
  const [modelDropdownOpen, setModelDropdownOpen] = useState(false);
  const [configPath, setConfigPath] = useState('');
  const [cachePath, setCachePath] = useState('');
  const [input, setInput] = useState('');
  const [isGenerating, setIsGenerating] = useState(false);
  const [isStopping, setIsStopping] = useState(false);
  const [attachment, setAttachment] = useState(null);

  // Start with an empty chat list containing one new chat
  const [chats, setChats] = useState([
    {
      id: 'chat-1',
      title: '新对话',
      messages: [],
      lastResponseId: null
    }
  ]);

  const [activeChatId, setActiveChatId] = useState('chat-1');
  const activeChat = chats.find((c) => c.id === activeChatId) || chats[0];
  const titlebarRef = useRef(null);
  const streamRequestMapRef = useRef({});
  const streamListenerRef = useRef(null);
  const activeRequestIdRef = useRef(null);
  const stoppedRequestIdsRef = useRef(new Set());

  // Auto-size input box height
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
    let mounted = true;
    invoke('get_model_catalog')
      .then((catalog) => {
        if (!mounted || !catalog) return;
        const available = Array.isArray(catalog.effectiveModels) && catalog.effectiveModels.length > 0
          ? catalog.effectiveModels
          : ['gpt-5-mini'];
        const configuredDefault = (catalog.defaultModel || '').trim();
        const nextModel = configuredDefault && available.includes(configuredDefault) ? configuredDefault : available[0];
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
          setChats((prevChats) =>
            prevChats.map((chat) => {
              if (chat.id !== mapping.chatId) return chat;
              const nextMessages = chat.messages.map((m) =>
                m.id === mapping.assistantMsgId ? { ...m, text: `${m.text || ''}${payload.delta}` } : m
              );
              return { ...chat, messages: nextMessages };
            })
          );
        }

        if (payload.kind === 'done') {
          setChats((prevChats) =>
            prevChats.map((chat) => {
              if (chat.id !== mapping.chatId) return chat;
              const nextMessages = chat.messages.map((m) =>
                m.id === mapping.assistantMsgId && typeof payload.delta === 'string' && payload.delta.length > 0
                  ? { ...m, text: payload.delta }
                  : m
              );
              return {
                ...chat,
                messages: nextMessages,
                lastResponseId: payload.responseId || chat.lastResponseId
              };
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

  const handleSend = async (textToSend) => {
    const text = textToSend || input;
    if (!text.trim() || isGenerating) return;

    // Clear input & attachments
    setInput('');
    setAttachment(null);

    // Create user message
    const userMsg = {
      id: `m-u-${Date.now()}`,
      role: 'user',
      text: text
    };

    // Update active chat messages
    const updatedChats = chats.map((c) => {
      if (c.id === activeChat.id) {
        let updatedTitle = c.title;
        if (c.messages.length === 0) {
          // If first message, rename title
          updatedTitle = text.length > 20 ? `${text.slice(0, 18)}...` : text;
        }
        return {
          ...c,
          title: updatedTitle,
          messages: [...c.messages, userMsg]
        };
      }
      return c;
    });

    setChats(updatedChats);
    setIsGenerating(true);

    const assistantMsgId = `m-a-${Date.now()}`;
    setChats((prevChats) =>
      prevChats.map((c) => {
        if (c.id === activeChat.id) {
          return {
            ...c,
            messages: [
              ...c.messages,
              { id: assistantMsgId, role: 'assistant', text: '' }
            ]
          };
        }
        return c;
      })
    );

    const requestId = `req-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    streamRequestMapRef.current[requestId] = {
      chatId: activeChat.id,
      assistantMsgId,
      lastEventIndex: 0
    };
    activeRequestIdRef.current = requestId;

    try {
      const response = await invoke('chat_with_responses_stream', {
        req: {
          input: text,
          history: (activeChat.messages || [])
            .filter((m) => m && (m.role === 'user' || m.role === 'assistant'))
            .map((m) => ({ role: m.role, text: m.text || '' })),
          previousResponseId: activeChat.lastResponseId,
          model: selectedModel,
          requestId
        }
      });
      const wasStopped = stoppedRequestIdsRef.current.has(requestId);

      setChats((prevChats) =>
        prevChats.map((c) => {
          if (c.id === activeChat.id) {
            const msgs = c.messages.map((m) =>
              m.id === assistantMsgId
                ? { ...m, text: response.outputText || m.text || (wasStopped ? '已停止' : '') }
                : m
            );
            return { ...c, messages: msgs, lastResponseId: response.responseId || null };
          }
          return c;
        })
      );
    } catch (err) {
      const errorText = typeof err === 'string' ? err : '请求失败，请检查配置文件中的 api_key / base_url 和网络连接。';
      setChats((prevChats) =>
        prevChats.map((c) => {
          if (c.id === activeChat.id) {
            const msgs = c.messages.map((m) =>
              m.id === assistantMsgId ? { ...m, text: `调用失败：${errorText}` } : m
            );
            return { ...c, messages: msgs };
          }
          return c;
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
    const newId = `chat-new-${Date.now()}`;
    const newChatObj = {
      id: newId,
      title: '新对话',
      messages: [],
      lastResponseId: null
    };
    setChats([newChatObj, ...chats]);
    setActiveChatId(newId);
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
    <div className="relative flex h-screen w-screen overflow-hidden bg-[#131314] text-slate-100 antialiased font-sans">
      <Sidebar
        isOpen={sidebarOpen}
        chats={chats}
        activeChatId={activeChatId}
        onSelectChat={(id) => setActiveChatId(id)}
        onNewChat={handleNewChat}
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
          messages={activeChat.messages}
          isGenerating={isGenerating}
          onSuggestionClick={handleSuggestionClick}
          activeChatTitle={activeChat.title}
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
                  className="max-h-[180px] flex-1 resize-none bg-transparent py-1.5 text-sm text-slate-200 placeholder-slate-500 scrollbar-thin focus:outline-none"
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
