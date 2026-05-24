import { Plus, Search, MessageSquare, Settings } from 'lucide-react';

export default function Sidebar({
  isOpen,
  conversations,
  activeConversationId,
  onSelectConversation,
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
            <Plus className="h-5 w-5 flex-shrink-0 text-cyan-400" />
            {isOpen && <span>发起新对话</span>}
          </button>

          <button
            className="flex w-full items-center gap-3 rounded-xl pl-[18px] pr-4 py-2.5 text-sm text-slate-400 transition hover:bg-[#2d2f31] hover:text-slate-200 whitespace-nowrap"
            title="搜索对话内容"
          >
            <Search className="h-5 w-5 flex-shrink-0" />
            {isOpen && <span>搜索对话内容</span>}
          </button>
        </div>

        {isOpen && conversations.length > 0 && (
          <div className="space-y-2">
            <h3 className="px-2 text-xs font-semibold uppercase tracking-wider text-slate-500 whitespace-nowrap">
              最近
            </h3>
            <div className="space-y-1">
              {conversations.map((conversation) => (
                <button
                  key={conversation.conversationId}
                  onClick={() => onSelectConversation(conversation.conversationId)}
                  className={`group flex w-full items-center gap-3 rounded-full pl-[18px] pr-4 py-2.5 text-left text-sm transition whitespace-nowrap ${
                    conversation.conversationId === activeConversationId
                      ? 'bg-[#2d2f31] font-medium text-slate-100'
                      : 'text-slate-400 hover:bg-[#2d2f31]/60 hover:text-slate-200'
                  }`}
                  title={conversation.title}
                >
                  <MessageSquare className="h-5 w-5 flex-shrink-0" />
                  <span className="flex-1 truncate pr-1">{conversation.title}</span>
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
          <Settings className="h-5 w-5 flex-shrink-0" />
          {isOpen && <span>设置</span>}
        </button>
      </div>
    </aside>
  );
}
