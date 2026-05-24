import { Ellipsis, MessageSquare } from 'lucide-react';
import { getConversationDisplayTitle } from '../../features/conversations/conversationUtils';

export default function SidebarConversationRow({
  conversation,
  isActive,
  isEditing,
  isBusy,
  isMenuOpen,
  draftTitle,
  onDraftTitleChange,
  onSelect,
  onSubmitRename,
  onCancelRename,
  onOpenMenuFromButton,
  onOpenMenuFromContext,
  editingRowRef,
  renameInputRef
}) {
  if (isEditing) {
    return (
      <div
        ref={editingRowRef}
        className="rounded-2xl border border-[#3c4043] bg-[#131314] px-3 py-2"
      >
        <div className="flex items-center gap-2">
          <MessageSquare className="h-4.5 w-4.5 flex-shrink-0 text-slate-500" />
          <input
            ref={renameInputRef}
            value={draftTitle}
            onChange={(event) => onDraftTitleChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault();
                onSubmitRename(conversation.conversationId);
              }
              if (event.key === 'Escape') {
                event.preventDefault();
                onCancelRename();
              }
            }}
            className="flex-1 bg-transparent text-sm text-slate-100 outline-none placeholder:text-slate-500"
            placeholder="输入对话标题"
          />
        </div>
      </div>
    );
  }

  return (
    <div
      onContextMenu={(event) => onOpenMenuFromContext(event, conversation.conversationId, isBusy)}
      className={`group relative rounded-full transition ${
        isActive || isMenuOpen ? 'bg-[#2d2f31]' : 'hover:bg-[#2d2f31]/60'
      }`}
    >
      <button
        onClick={() => onSelect(conversation.conversationId)}
        className={`flex w-full items-center gap-3 whitespace-nowrap rounded-full py-2.5 pl-[18px] pr-11 text-left text-sm transition ${
          isActive ? 'font-medium text-slate-100' : 'text-slate-400 hover:text-slate-200'
        }`}
        title={getConversationDisplayTitle(conversation)}
      >
        <MessageSquare className="h-5 w-5 flex-shrink-0" />
        <span className="min-w-0 flex-1 truncate">{getConversationDisplayTitle(conversation)}</span>
      </button>

      <div className="absolute inset-y-0 right-2 flex items-center">
        <button
          onClick={(event) => onOpenMenuFromButton(event, conversation.conversationId, isBusy)}
          disabled={isBusy}
          className={`rounded-full p-1.5 transition ${
            isMenuOpen
              ? 'bg-[#131314] text-slate-100'
              : 'text-slate-400 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 hover:bg-[#131314] hover:text-slate-100'
          } disabled:cursor-not-allowed disabled:opacity-30`}
          title={isBusy ? '生成中暂不可操作' : '更多操作'}
        >
          <Ellipsis className="h-4 w-4" />
        </button>
      </div>
    </div>
  );
}
