import { useState, useRef, useEffect } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import Sidebar from './components/Sidebar';
import ChatArea from './components/ChatArea';
import { getAIResponse, mockResponses } from './utils/mockResponses';

function App() {
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [selectedModel, setSelectedModel] = useState('Flash');
  const [modelDropdownOpen, setModelDropdownOpen] = useState(false);
  const [input, setInput] = useState('');
  const [isGenerating, setIsGenerating] = useState(false);
  const [mockAttachment, setMockAttachment] = useState(null);

  // Start with an empty chat list containing one new chat
  const [chats, setChats] = useState([
    {
      id: 'chat-1',
      title: '新对话',
      messages: []
    }
  ]);

  const [activeChatId, setActiveChatId] = useState('chat-1');
  const activeChat = chats.find((c) => c.id === activeChatId) || chats[0];
  const titlebarRef = useRef(null);

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

  const handleSend = (textToSend) => {
    const text = textToSend || input;
    if (!text.trim() || isGenerating) return;

    // Clear input & attachments
    setInput('');
    setMockAttachment(null);

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

    // Add empty assistant response slot
    setTimeout(() => {
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

      // Simulate streaming response
      const rawResponseText = getAIResponse(text);
      let streamedText = '';
      let index = 0;

      const streamTimer = setInterval(() => {
        if (index < rawResponseText.length) {
          // Progressively append chunks of characters
          const nextChunkSize = Math.floor(Math.random() * 10) + 4;
          streamedText += rawResponseText.substring(index, index + nextChunkSize);
          index += nextChunkSize;

          setChats((prevChats) =>
            prevChats.map((c) => {
              if (c.id === activeChat.id) {
                const msgs = [...c.messages];
                const lastMsg = msgs[msgs.length - 1];
                if (lastMsg && lastMsg.role === 'assistant') {
                  lastMsg.text = streamedText;
                }
                return { ...c, messages: msgs };
              }
              return c;
            })
          );
        } else {
          clearInterval(streamTimer);
          setIsGenerating(false);
        }
      }, 20);
    }, 600);
  };

  const handleNewChat = () => {
    const newId = `chat-new-${Date.now()}`;
    const newChatObj = {
      id: newId,
      title: '新对话',
      messages: []
    };
    setChats([newChatObj, ...chats]);
    setActiveChatId(newId);
  };

  const handleSuggestionClick = (text) => {
    handleSend(text);
  };

  const handleAttachMockFile = () => {
    setMockAttachment({
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
          <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth={1.5} stroke="currentColor" className="h-4.5 w-4.5">
            <path strokeLinecap="round" strokeLinejoin="round" d="M3.75 6.75h16.5M3.75 12h16.5m-16.5 5.25h16.5" />
          </svg>
        </button>

        <div className="flex min-w-0 flex-1 items-center gap-1 pr-6 ml-2">
          <div className="relative flex items-center gap-1.5" data-no-drag="true">
            <button
              onClick={() => setModelDropdownOpen(!modelDropdownOpen)}
              className="flex items-center gap-2 rounded-xl px-3 py-1 text-sm font-medium text-slate-300 transition hover:bg-[#2d2f31] whitespace-nowrap"
            >
              <span className="truncate">AgentJax {selectedModel}</span>
              <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth={2.5} stroke="currentColor" className="h-3 w-3 text-slate-400">
                <path strokeLinecap="round" strokeLinejoin="round" d="M19.5 8.25l-7.5 7.5-7.5-7.5" />
              </svg>
            </button>

            {modelDropdownOpen && (
              <div className="absolute top-10 left-0 z-50 w-56 rounded-2xl border border-[#2d2f31] bg-[#1e1f20] p-2 shadow-2xl">
                <button
                  onClick={() => {
                    setSelectedModel('Flash');
                    setModelDropdownOpen(false);
                  }}
                  className="flex w-full flex-col rounded-xl px-3 py-2 text-left transition hover:bg-[#2d2f31]"
                >
                  <span className="text-sm font-medium text-slate-200">AgentJax Flash</span>
                  <span className="text-[11px] text-slate-500">轻量快速，适合日常简单问答</span>
                </button>
                <button
                  onClick={() => {
                    setSelectedModel('Pro');
                    setModelDropdownOpen(false);
                  }}
                  className="mt-1 flex w-full flex-col rounded-xl px-3 py-2 text-left transition hover:bg-[#2d2f31]"
                >
                  <span className="text-sm font-medium text-purple-300">AgentJax Pro</span>
                  <span className="text-[11px] text-slate-500">超强推理，支持复杂数据分析</span>
                </button>
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
              {mockAttachment && (
                <div className="mb-2 flex items-center gap-2 self-start rounded-xl border border-[#2d2f31] bg-[#131314] p-1.5 pr-2.5">
                  <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-red-500/10 text-red-400">
                    <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth={1.5} stroke="currentColor" className="h-5 w-5">
                      <path strokeLinecap="round" strokeLinejoin="round" d="M2.25 15.75l5.159-5.159a2.25 2.25 0 013.182 0l5.159 5.159m-1.5-1.5l1.409-1.409a2.25 2.25 0 013.182 0l2.909 2.909m-18 3.75h16.5a1.5 1.5 0 001.5-1.5V6a1.5 1.5 0 00-1.5-1.5H3.75A1.5 1.5 0 002.25 6v12a1.5 1.5 0 001.5 1.5zm10.5-11.25h.008v.008h-.008V8.25zm.375 0a.375.375 0 11-.75 0 .375.375 0 01.75 0z" />
                    </svg>
                  </div>
                  <span className="text-xs font-medium text-slate-300">{mockAttachment.name}</span>
                  <button
                    onClick={() => setMockAttachment(null)}
                    className="ml-2 rounded-full p-0.5 text-slate-400 hover:bg-[#2d2f31] hover:text-slate-200"
                  >
                    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="h-3.5 w-3.5">
                      <path d="M6.28 5.22a.75.75 0 00-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 101.06 1.06L10 11.06l3.72 3.72a.75.75 0 101.06-1.06L11.06 10l3.72-3.72a.75.75 0 00-1.06-1.06L10 8.94 6.28 5.22z" />
                    </svg>
                  </button>
                </div>
              )}

              <div className="flex items-center gap-3">
                <button
                  onClick={handleAttachMockFile}
                  className="rounded-full p-2 text-slate-400 transition hover:bg-[#2d2f31] hover:text-slate-200"
                  title="上传文件/图片"
                >
                  <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth={1.8} stroke="currentColor" className="h-5.5 w-5.5">
                    <path strokeLinecap="round" strokeLinejoin="round" d="M12 4.5v15m7.5-7.5h-15" />
                  </svg>
                </button>

                <textarea
                  ref={textareaRef}
                  value={input}
                  onChange={(e) => setInput(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' && !e.shiftKey) {
                      e.preventDefault();
                      handleSend();
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
                    <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth={1.8} stroke="currentColor" className="h-5.5 w-5.5">
                      <path strokeLinecap="round" strokeLinejoin="round" d="M12 18.75a6 6 0 006-6v-1.5m-6 7.5a6 6 0 01-6-6v-1.5m6 7.5v3.75m-3.75 0h7.5M12 15.75a3 3 0 01-3-3V4.5a3 3 0 116 0v8.25a3 3 0 01-3 3z" />
                    </svg>
                  </button>

                  <button
                    onClick={() => handleSend()}
                    disabled={!input.trim() || isGenerating}
                    className={`rounded-full p-2 transition ${
                      input.trim() && !isGenerating
                        ? 'bg-gradient-to-tr from-cyan-400 via-purple-500 to-pink-500 text-white hover:opacity-90 cursor-pointer shadow shadow-purple-500/20'
                        : 'bg-transparent text-slate-600'
                    }`}
                    title="发送消息"
                  >
                    <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth={2.2} stroke="currentColor" className="h-4.5 w-4.5">
                      <path strokeLinecap="round" strokeLinejoin="round" d="M6 12L3.269 3.126A59.768 59.768 0 0121.485 12 59.77 59.77 0 013.27 20.876L5.999 12zm0 0h7.5" />
                    </svg>
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
