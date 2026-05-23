import React from 'react';

export default function Sidebar({
  isOpen,
  chats,
  activeChatId,
  onSelectChat,
  onNewChat
}) {
  return (
    <aside
      className={`fixed inset-y-0 left-0 z-30 flex flex-col border-r border-[#2d2f31] bg-[#1e1f20] pt-12 text-slate-200 transition-all duration-300 overflow-hidden ${
        isOpen ? 'w-64' : 'w-20'
      }`}
    >
      <div className="scrollbar-thin flex-1 space-y-6 overflow-y-auto px-3 py-2">
        <div className="space-y-1">
          <button
            onClick={onNewChat}
            className="flex w-full items-center gap-3 rounded-full border border-[#2d2f31]/50 bg-[#131314] pl-[18px] pr-4 py-3 text-sm font-medium transition hover:bg-[#2d2f31] whitespace-nowrap"
            title="发起新对话"
          >
            <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth={2} stroke="currentColor" className="h-5 w-5 flex-shrink-0 text-cyan-400">
              <path strokeLinecap="round" strokeLinejoin="round" d="M12 4.5v15m7.5-7.5h-15" />
            </svg>
            {isOpen && <span>发起新对话</span>}
          </button>

          <button
            className="flex w-full items-center gap-3 rounded-xl pl-[18px] pr-4 py-2.5 text-sm text-slate-400 transition hover:bg-[#2d2f31] hover:text-slate-200 whitespace-nowrap"
            title="搜索对话内容"
          >
            <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth={1.5} stroke="currentColor" className="h-5 w-5 flex-shrink-0">
              <path strokeLinecap="round" strokeLinejoin="round" d="M21 21l-5.197-5.197m0 0A7.5 7.5 0 105.196 5.196a7.5 7.5 0 0010.604 10.604z" />
            </svg>
            {isOpen && <span>搜索对话内容</span>}
          </button>
        </div>

        {isOpen && chats.length > 0 && (
          <div className="space-y-2">
            <h3 className="px-2 text-xs font-semibold uppercase tracking-wider text-slate-500 whitespace-nowrap">
              最近
            </h3>
            <div className="space-y-1">
              {chats.map((chat) => (
                <button
                  key={chat.id}
                  onClick={() => onSelectChat(chat.id)}
                  className={`group flex w-full items-center gap-3 rounded-full pl-[18px] pr-4 py-2.5 text-left text-sm transition whitespace-nowrap ${
                    chat.id === activeChatId
                      ? 'bg-[#2d2f31] font-medium text-slate-100'
                      : 'text-slate-400 hover:bg-[#2d2f31]/60 hover:text-slate-200'
                  }`}
                  title={chat.title}
                >
                  <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth={1.5} stroke="currentColor" className="h-5 w-5 flex-shrink-0">
                    <path strokeLinecap="round" strokeLinejoin="round" d="M7.5 8.25h9m-9 3H12m-9.75 1.51c0 1.6 1.123 2.994 2.707 3.227 1.129.166 2.27.293 3.423.379.35.026.67.21.865.501L12 21l2.755-4.133a1.14 1.14 0 01.865-.501 48.172 48.172 0 003.423-.379c1.584-.233 2.707-1.626 2.707-3.228V6.741c0-1.602-1.123-2.995-2.707-3.228A48.394 48.394 0 0012 3c-2.392 0-4.744.175-7.043.513C3.373 3.746 2.25 5.14 2.25 6.741v6.018z" />
                  </svg>
                  <span className="flex-1 truncate pr-1">{chat.title}</span>
                </button>
              ))}
            </div>
          </div>
        )}
      </div>

      <div className="border-t border-[#2d2f31] p-3">
        <button
          className="flex w-full items-center gap-3 rounded-xl pl-[18px] pr-4 py-2.5 text-sm text-slate-400 transition hover:bg-[#2d2f31] hover:text-slate-200 whitespace-nowrap"
          title="设置"
        >
          <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth={1.5} stroke="currentColor" className="h-5 w-5 flex-shrink-0">
            <path strokeLinecap="round" strokeLinejoin="round" d="M9.594 3.94c.09-.542.56-.94 1.11-.94h2.593c.55 0 1.02.398 1.11.94l.213 1.281c.063.374.313.686.645.87.074.04.147.083.22.127.324.196.72.257 1.075.124l1.217-.456a1.125 1.125 0 011.37.49l1.296 2.247a1.125 1.125 0 01-.26 1.43l-1.003.828c-.293.241-.438.613-.43.992a7.723 7.723 0 010 .255c-.008.378.137.75.43.991l1.004.827c.424.35.534.954.26 1.43l-1.298 2.247a1.125 1.125 0 01-1.369.491l-1.217-.456c-.355-.133-.75-.072-1.076.124a6.57 6.57 0 01-.22.128c-.331.183-.581.495-.644.869l-.213 1.28c-.09.543-.56.94-1.11.94h-2.594c-.55 0-1.02-.398-1.11-.94l-.213-1.281c-.062-.374-.312-.686-.644-.87a6.52 6.52 0 01-.22-.127c-.325-.196-.72-.257-1.076-.124l-1.217.456a1.125 1.125 0 01-1.369-.49l-1.297-2.247a1.125 1.125 0 01.26-1.43l1.004-.827c.292-.24.437-.613.43-.992a6.932 6.932 0 010-.255c.007-.378-.138-.75-.43-.99l-1.004-.828a1.125 1.125 0 01-.26-1.43l1.297-2.247a1.125 1.125 0 011.37-.491l1.216.456c.356.133.751.072 1.076-.124.072-.044.146-.087.22-.128.332-.183.582-.495.644-.869l.214-1.28z" />
            <path strokeLinecap="round" strokeLinejoin="round" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
          </svg>
          {isOpen && <span>设置</span>}
        </button>
      </div>
    </aside>
  );
}
