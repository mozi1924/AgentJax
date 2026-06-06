import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { CSSProperties, MouseEvent as ReactMouseEvent } from 'react';
import { Activity, Plus, Search, Settings } from 'lucide-react';
import SidebarActionMenu from './sidebar/SidebarActionMenu';
import SidebarConversationRow from './sidebar/SidebarConversationRow';
import AgentSwitcher from './sidebar/AgentSwitcher';
import { getConversationDisplayTitle } from '../features/conversations/conversationUtils';
import type { Conversation } from '../features/conversations/types';
import type { AgentSummary } from '../hooks/useActiveAgent';
import { useI18n } from '../features/i18n';
import { OverlayScrollArea } from './OverlayScrollArea';

interface SidebarMenuState {
  conversationId: string;
  source: 'button' | 'context';
  x: number;
  y: number;
}

interface SidebarProps {
  isOpen: boolean;
  conversations: Conversation[];
  activeConversationId: string;
  onSelectConversation: (conversationId: string) => void;
  onNewChat: () => void;
  onOpenSettings: () => void;
  onOpenLcmHealth: () => void;
  onRenameConversation: (conversationId: string, title: string) => Promise<void>;
  onDeleteConversation: (conversationId: string) => Promise<void> | void;
  generatingConversationIds: Set<string>;
  // Agent management
  agents: AgentSummary[];
  activeAgentId: string;
  activeAgent: AgentSummary | null;
  agentsLoading: boolean;
  onSwitchAgent: (agentId: string) => void;
  onCreateAgent: (agentId: string, templateId?: string) => Promise<void>;
  onDeleteAgent: (agentId: string) => Promise<void>;
}

export default function Sidebar({
  isOpen,
  conversations,
  activeConversationId,
  onSelectConversation,
  onNewChat,
  onOpenSettings,
  onOpenLcmHealth,
  onRenameConversation,
  onDeleteConversation,
  generatingConversationIds,
  agents,
  activeAgentId,
  activeAgent,
  agentsLoading,
  onSwitchAgent,
  onCreateAgent,
  onDeleteAgent,
}: SidebarProps) {
  const { t } = useI18n();
  const [editingConversationId, setEditingConversationId] = useState<string | null>(null);
  const [draftTitle, setDraftTitle] = useState('');
  const [menuState, setMenuState] = useState<SidebarMenuState | null>(null);
  const [menuPosition, setMenuPosition] = useState<CSSProperties | null>(null);

  const menuRef = useRef<HTMLDivElement | null>(null);
  const renameInputRef = useRef<HTMLInputElement | null>(null);
  const editingRowRef = useRef<HTMLDivElement | null>(null);

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
    const handleDocumentMouseDown = (event: MouseEvent) => {
      const target = event.target as Node | null;
      if (!target) return;

      if (menuState && menuRef.current && !menuRef.current.contains(target)) {
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

    const handleKeyDown = (event: KeyboardEvent) => {
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

    document.addEventListener('mousedown', handleDocumentMouseDown);
    document.addEventListener('keydown', handleKeyDown);

    return () => {
      document.removeEventListener('mousedown', handleDocumentMouseDown);
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

  const beginRename = (conversation: Conversation) => {
    setMenuState(null);
    setEditingConversationId(conversation.conversationId);
    setDraftTitle(getConversationDisplayTitle(conversation, t));
  };

  const submitRename = async (conversationId: string) => {
    const nextTitle = draftTitle.trim();
    if (!nextTitle) {
      cancelRename();
      return;
    }

    await onRenameConversation(conversationId, nextTitle);
    cancelRename();
  };

  const openMenuFromButton = (
    event: ReactMouseEvent<HTMLButtonElement>,
    conversationId: string,
    disabled: boolean
  ) => {
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
        y: rect.bottom + 6,
      };
    });
  };

  const openMenuFromContext = (
    event: ReactMouseEvent<HTMLDivElement>,
    conversationId: string,
    disabled: boolean
  ) => {
    event.preventDefault();
    if (disabled) {
      return;
    }

    setMenuState({
      conversationId,
      source: 'context',
      x: event.clientX,
      y: event.clientY,
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

  const handleMenuDelete = async () => {
    if (!menuState?.conversationId) {
      return;
    }

    const conversationId = menuState.conversationId;
    setMenuState(null);
    await onDeleteConversation(conversationId);
  };

  return (
    <aside
      className={`fixed inset-y-0 left-0 z-30 flex flex-col overflow-visible bg-transparent text-slate-200 transition-all duration-300 ${
        isOpen ? 'w-64' : 'w-20'
      }`}
    >
      <div
        className={`absolute inset-y-0 left-0 -z-10 w-64 border-r border-[#2d2f31]/60 bg-[#1e1f20] transition-transform duration-300 ease-in-out ${
          isOpen ? 'translate-x-0' : '-translate-x-full'
        }`}
      />

      <div className="h-12 shrink-0" />

      {/* Agent switcher — always visible, collapses in icon mode */}
      <div className="shrink-0 pt-1 pb-2">
        <AgentSwitcher
          agents={agents}
          activeAgentId={activeAgentId}
          activeAgent={activeAgent}
          agentsLoading={agentsLoading}
          onSwitchAgent={onSwitchAgent}
          onCreateAgent={onCreateAgent}
          onDeleteAgent={onDeleteAgent}
        />
      </div>

      <OverlayScrollArea containerClassName="flex-1" className="h-full space-y-6 px-3 py-2">
        <div className="space-y-2">
          <button
            onClick={onNewChat}
            className={`flex h-11 items-center whitespace-nowrap border border-[#2d2f31]/50 bg-[#131314] text-sm font-medium transition-all duration-300 hover:bg-[#2d2f31] ${
              isOpen ? 'ml-0 w-[232px] rounded-full pl-[18px] pr-4' : 'ml-[6px] w-11 rounded-full pl-[12px]'
            }`}
            title={t('sidebar.new_chat')}
          >
            <Plus className="h-5 w-5 flex-shrink-0 text-slate-300" />
            <span
              className={`transform overflow-hidden whitespace-nowrap transition-all duration-500 ease-out ${
                isOpen
                  ? 'ml-3 max-w-[150px] translate-x-0 opacity-100'
                  : 'pointer-events-none max-w-0 -translate-x-4 opacity-0'
              }`}
            >
              {t('sidebar.new_chat')}
            </span>
          </button>

          <button
            className={`flex h-11 items-center whitespace-nowrap text-sm text-slate-400 transition-all duration-300 hover:bg-[#2d2f31] hover:text-slate-200 ${
              isOpen ? 'ml-0 w-[232px] rounded-xl pl-[18px] pr-4' : 'ml-[6px] w-11 rounded-full pl-[12px]'
            }`}
            title={t('sidebar.search')}
          >
            <Search className="h-5 w-5 flex-shrink-0" />
            <span
              className={`transform overflow-hidden whitespace-nowrap transition-all duration-500 ease-out ${
                isOpen
                  ? 'ml-3 max-w-[150px] translate-x-0 opacity-100'
                  : 'pointer-events-none max-w-0 -translate-x-4 opacity-0'
              }`}
            >
              {t('sidebar.search')}
            </span>
          </button>
        </div>

        <div
          className={`space-y-4 transform transition-all duration-500 ease-out ${
            isOpen
              ? 'visible max-h-[1000px] translate-x-0 opacity-100'
              : 'invisible pointer-events-none max-h-0 -translate-x-8 overflow-hidden opacity-0'
          }`}
        >
          {conversations.length > 0 && (
            <div className="space-y-2">
              <h3 className="whitespace-nowrap px-2 text-xs font-semibold tracking-wider text-slate-500 uppercase">
                {t('sidebar.recent')}
              </h3>
              <div className="space-y-1">
                {conversations.map((conversation) => {
                  const isActive = conversation.conversationId === activeConversationId;
                  const isEditing = conversation.conversationId === editingConversationId;
                  const isBusy =
                    generatingConversationIds?.has(conversation.conversationId) || false;
                  const isMenuOpen =
                    conversation.conversationId === menuState?.conversationId;

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
      </OverlayScrollArea>

      {menuState && (
        <SidebarActionMenu
          menuRef={menuRef}
          menuPosition={menuPosition}
          onRename={handleMenuRename}
          onDelete={handleMenuDelete}
        />
      )}

      <div className="shrink-0 space-y-1 p-3">
        <button
          onClick={onOpenLcmHealth}
          className={`flex h-9 items-center whitespace-nowrap text-xs text-slate-500 transition-all duration-300 hover:bg-[#2d2f31] hover:text-slate-200 ${
            isOpen ? 'ml-0 w-[232px] rounded-xl pl-[18px] pr-4' : 'ml-[6px] w-11 rounded-xl pl-[12px]'
          }`}
          title="LCM Health"
        >
          <Activity className="h-4 w-4 flex-shrink-0" />
          <span
            className={`transform overflow-hidden whitespace-nowrap transition-all duration-500 ease-out ${
              isOpen
                ? 'ml-3 max-w-[150px] translate-x-0 opacity-100'
                : 'pointer-events-none max-w-0 -translate-x-4 opacity-0'
            }`}
          >
            LCM Health
          </span>
        </button>
        <button
          onClick={onOpenSettings}
          className={`flex h-11 items-center whitespace-nowrap text-sm text-slate-400 transition-all duration-300 hover:bg-[#2d2f31] hover:text-slate-200 ${
            isOpen ? 'ml-0 w-[232px] rounded-xl pl-[18px] pr-4' : 'ml-[6px] w-11 rounded-xl pl-[12px]'
          }`}
          title={t('sidebar.settings')}
        >
          <Settings className="h-5 w-5 flex-shrink-0" />
          <span
            className={`transform overflow-hidden whitespace-nowrap transition-all duration-500 ease-out ${
              isOpen
                ? 'ml-3 max-w-[150px] translate-x-0 opacity-100'
                : 'pointer-events-none max-w-0 -translate-x-4 opacity-0'
            }`}
          >
            {t('sidebar.settings')}
          </span>
        </button>
      </div>
    </aside>
  );
}
