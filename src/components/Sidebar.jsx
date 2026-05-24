import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { Plus, Search, Settings } from 'lucide-react';
import SidebarActionMenu from './sidebar/SidebarActionMenu';
import SidebarConversationRow from './sidebar/SidebarConversationRow';
import { getConversationDisplayTitle } from '../features/conversations/conversationUtils';

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
  const [menuPosition, setMenuPosition] = useState(null);

  const menuRef = useRef(null);
  const renameInputRef = useRef(null);
  const editingRowRef = useRef(null);

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

      if (menuState && menuRef.current && !menuRef.current.contains(target)) {
        setMenuState(null);
      }

      if (editingConversationId && editingRowRef.current && !editingRowRef.current.contains(target)) {
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
    setDraftTitle(getConversationDisplayTitle(conversation));
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

                return (
                  <SidebarConversationRow
                    key={conversation.conversationId}
                    conversation={conversation}
                    isActive={isActive}
                    isEditing={isEditing}
                    isBusy={isBusy}
                    isMenuOpen={isMenuOpen}
                    draftTitle={draftTitle}
                    onDraftTitleChange={setDraftTitle}
                    onSelect={onSelectConversation}
                    onSubmitRename={submitRename}
                    onCancelRename={cancelRename}
                    onOpenMenuFromButton={openMenuFromButton}
                    onOpenMenuFromContext={openMenuFromContext}
                    editingRowRef={editingRowRef}
                    renameInputRef={renameInputRef}
                  />
                );
              })}
            </div>
          </div>
        )}
      </div>

      {menuState && (
        <SidebarActionMenu
          menuRef={menuRef}
          menuPosition={menuPosition}
          onRename={handleMenuRename}
          onDelete={handleMenuDelete}
        />
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
