import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import {
  Ellipsis,
  MessageSquare,
  Pencil,
  Plus,
  Search,
  Settings,
  Trash2
} from 'lucide-react';

export default function Sidebar({
  isOpen,
  conversations,
  activeConversationId,
  onSelectConversation,
  onNewChat,
  onRenameConversation,
  onDeleteConversation,
  isGenerating
}) {
  const [editingConversationId, setEditingConversationId] = useState(null);
  const [draftTitle, setDraftTitle] = useState('');
  const [menuState, setMenuState] = useState(null);

  const sidebarRef = useRef(null);
  const menuRef = useRef(null);
  const renameInputRef = useRef(null);
  const editingRowRef = useRef(null);
  const [menuPosition, setMenuPosition] = useState(null);

  const activeConversation = useMemo(
    () => conversations.find((conversation) => conversation.conversationId === activeConversationId),
    [activeConversationId, conversations]
  );

  useEffect(() => {
    if (!editingConversationId) {
      return undefined;
    }

    const frameId = requestAnimationFrame(() => {
      renameInputRef.current?.focus();
      renameInputRef.current?.select();
    });

    return () => cancelAnimationFrame(frameId);
  }, [editingConversationId]);

  useEffect(() => {
    const handlePointerDown = (event) => {
      const target = event.target;

      if (
        menuState &&
        menuRef.current &&
        !menuRef.current.contains(target)
      ) {
        setMenuState(null);
      }

      if (
        editingConversationId &&
        editingRowRef.current &&
        !editingRowRef.current.contains(target)
      ) {
        setEditingConversationId(null);
        setDraftTitle('');
      }
    };

    const handleKeyDown = (event) => {
      if (event.key !== 'Escape') {
        return;
      }

      if (menuState) {
        setMenuState(null);
      }
      if (editingConversationId) {
        setEditingConversationId(null);
        setDraftTitle('');
      }
    };

    document.addEventListener('pointerdown', handlePointerDown);
    document.addEventListener('keydown', handleKeyDown);

    return () => {
      document.removeEventListener('pointerdown', handlePointerDown);
      document.removeEventListener('keydown', handleKeyDown);
    };
  }, [editingConversationId, menuState]);

  useLayoutEffect(() => {
    if (!menuState || !menuRef.current) {
      setMenuPosition(null);
      return;
    }

    const updateMenuPosition = () => {
      const menuWidth = menuRef.current?.offsetWidth || 160;
      const menuHeight = menuRef.current?.offsetHeight || 104;
      const nextX = Math.max(16, Math.min(menuState.x, window.innerWidth - menuWidth - 16));
      const nextY = Math.max(56, Math.min(menuState.y, window.innerHeight - menuHeight - 16));
      setMenuPosition({ left: `${nextX}px`, top: `${nextY}px` });
    };

    updateMenuPosition();
    window.addEventListener('resize', updateMenuPosition);

    return () => {
      window.removeEventListener('resize', updateMenuPosition);
    };
  }, [menuState]);

  const cancelRename = () => {
    setEditingConversationId(null);
    setDraftTitle('');
  };

  const beginRename = (conversation) => {
    setMenuState(null);
    setEditingConversationId(conversation.conversationId);
    setDraftTitle(conversation.title || '');
  };

  const submitRename = async (conversationId) => {
    const nextTitle = draftTitle.trim();
    if (!nextTitle) {
      cancelRename();
      return;
    }

    await onRenameConversation(conversationId, nextTitle);
    cancelRename();
  };

  const openMenuFromButton = (event, conversationId, disabled) => {
    event.stopPropagation();
    if (disabled) {
      return;
    }

    const rect = event.currentTarget.getBoundingClientRect();
    setMenuState((current) => {
      if (current?.conversationId === conversationId && current.source === 'button') {
        return null;
      }

      return {
        conversationId,
        source: 'button',
        x: rect.right - 168,
        y: rect.bottom + 6
      };
    });
  };

  const openMenuFromContext = (event, conversationId, disabled) => {
    event.preventDefault();
    if (disabled) {
      return;
    }

    setMenuState({
      conversationId,
      source: 'context',
      x: event.clientX,
      y: event.clientY
    });
  };

  const handleMenuRename = () => {
    const conversation = conversations.find(
      (item) => item.conversationId === menuState?.conversationId
    );
    if (!conversation) {
      setMenuState(null);
      return;
    }

    beginRename(conversation);
  };

  const handleMenuDelete = () => {
    if (!menuState?.conversationId) {
      return;
    }

    const conversationId = menuState.conversationId;
    setMenuState(null);
    onDeleteConversation(conversationId);
  };

  return (
    <aside
      ref={sidebarRef}
      className={`fixed inset-y-0 left-0 z-30 flex flex-col overflow-hidden border-r border-[#2d2f31] bg-[#1e1f20] pt-12 text-slate-200 transition-all duration-300 ${
        isOpen ? 'w-64' : 'w-20'
      }`}
    >
      <div className="scrollbar-thin flex-1 space-y-6 overflow-y-auto px-3 py-2">
        <div className="space-y-1">
          <button
            onClick={onNewChat}
            className="flex w-full items-center gap-3 whitespace-nowrap rounded-full border border-[#2d2f31]/50 bg-[#131314] py-3 pl-[18px] pr-4 text-sm font-medium transition hover:bg-[#2d2f31]"
            title="发起新对话"
          >
            <Plus className="h-5 w-5 flex-shrink-0 text-cyan-400" />
            {isOpen && <span>发起新对话</span>}
          </button>

          <button
            className="flex w-full items-center gap-3 whitespace-nowrap rounded-xl py-2.5 pl-[18px] pr-4 text-sm text-slate-400 transition hover:bg-[#2d2f31] hover:text-slate-200"
            title="搜索对话内容"
          >
            <Search className="h-5 w-5 flex-shrink-0" />
            {isOpen && <span>搜索对话内容</span>}
          </button>
        </div>

        {isOpen && conversations.length > 0 && (
          <div className="space-y-2">
            <h3 className="whitespace-nowrap px-2 text-xs font-semibold uppercase tracking-wider text-slate-500">
              最近
            </h3>
            <div className="space-y-1">
              {conversations.map((conversation) => {
                const isActive = conversation.conversationId === activeConversationId;
                const isEditing = conversation.conversationId === editingConversationId;
                const isBusy =
                  isGenerating && conversation.conversationId === activeConversation?.conversationId;
                const isMenuOpen = conversation.conversationId === menuState?.conversationId;

                if (isEditing) {
                  return (
                    <div
                      key={conversation.conversationId}
                      ref={editingRowRef}
                      className="rounded-2xl border border-[#3c4043] bg-[#131314] px-3 py-2"
                    >
                      <div className="flex items-center gap-2">
                        <MessageSquare className="h-4.5 w-4.5 flex-shrink-0 text-slate-500" />
                        <input
                          ref={renameInputRef}
                          value={draftTitle}
                          onChange={(event) => setDraftTitle(event.target.value)}
                          onKeyDown={(event) => {
                            if (event.key === 'Enter') {
                              event.preventDefault();
                              submitRename(conversation.conversationId);
                            }
                            if (event.key === 'Escape') {
                              event.preventDefault();
                              cancelRename();
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
                    key={conversation.conversationId}
                    onContextMenu={(event) =>
                      openMenuFromContext(event, conversation.conversationId, isBusy)
                    }
                    className={`group relative rounded-full transition ${
                      isActive || isMenuOpen ? 'bg-[#2d2f31]' : 'hover:bg-[#2d2f31]/60'
                    }`}
                  >
                    <button
                      onClick={() => onSelectConversation(conversation.conversationId)}
                      className={`flex w-full items-center gap-3 whitespace-nowrap rounded-full py-2.5 pl-[18px] pr-11 text-left text-sm transition ${
                        isActive
                          ? 'font-medium text-slate-100'
                          : 'text-slate-400 hover:text-slate-200'
                      }`}
                      title={conversation.title}
                    >
                      <MessageSquare className="h-5 w-5 flex-shrink-0" />
                      <span className="min-w-0 flex-1 truncate">{conversation.title}</span>
                    </button>

                    <div className="absolute inset-y-0 right-2 flex items-center">
                      <button
                        onClick={(event) =>
                          openMenuFromButton(event, conversation.conversationId, isBusy)
                        }
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
              })}
            </div>
          </div>
        )}
      </div>

      {menuState && (
        <div
          ref={menuRef}
          style={menuPosition || undefined}
          className="sidebar-context-menu fixed z-50 w-40 overflow-hidden rounded-2xl border border-[#3c4043] bg-[#131314]/98 p-1.5 shadow-2xl shadow-black/40 backdrop-blur-md"
        >
          <button
            onClick={handleMenuRename}
            className="flex w-full items-center gap-2 rounded-xl px-3 py-2 text-left text-sm text-slate-200 transition hover:bg-[#2d2f31]"
          >
            <Pencil className="h-4 w-4" />
            <span>重命名</span>
          </button>
          <div className="my-1 border-t border-[#2d2f31]" />
          <button
            onClick={handleMenuDelete}
            className="flex w-full items-center gap-2 rounded-xl px-3 py-2 text-left text-sm text-rose-300 transition hover:bg-[#2d2f31]"
          >
            <Trash2 className="h-4 w-4" />
            <span>删除</span>
          </button>
        </div>
      )}

      <div className="border-t border-[#2d2f31] p-3">
        <button
          className="flex w-full items-center gap-3 whitespace-nowrap rounded-xl py-2.5 pl-[18px] pr-4 text-sm text-slate-400 transition hover:bg-[#2d2f31] hover:text-slate-200"
          title="设置"
        >
          <Settings className="h-5 w-5 flex-shrink-0" />
          {isOpen && <span>设置</span>}
        </button>
      </div>
    </aside>
  );
}
