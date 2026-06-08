import { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  createLocalConversation,
  getConversationDisplayTitle,
  hydrateConversationLines,
  isConversationEmpty,
  shouldShowConversationInSidebar,
} from '../features/conversations/conversationUtils';
import { restoreConversationPreview } from '../features/conversations/conversationState';
import type {
  Conversation,
  ConversationDetail,
  ConversationSummary,
} from '../features/conversations/types';
import {
  applyLoadedConversationDetail,
  applyManualConversationRename,
  rebuildConversationListAfterDeletion,
} from '../features/conversations/sessionState';

interface UseConversationRegistryOptions {
  selectedModelId: string;
  agentId?: string;
}

export function useConversationRegistry({ selectedModelId, agentId }: UseConversationRegistryOptions) {
  const initialConversation = useMemo(() => createLocalConversation(), [agentId]);
  const [conversations, setConversations] = useState<Conversation[]>(() => [initialConversation]);
  const [conversationToDelete, setConversationToDelete] = useState<Conversation | null>(null);
  const [activeConversationId, setActiveConversationId] = useState(
    initialConversation.conversationId
  );

  // Reset conversation state when switching agents
  useEffect(() => {
    const fresh = createLocalConversation();
    setConversations([fresh]);
    setActiveConversationId(fresh.conversationId);
  }, [agentId]);

  const activeConversation = useMemo(
    () =>
      conversations.find(
        (conversation) => conversation.conversationId === activeConversationId
      ) || conversations[0],
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

  const isEmptyConversation = (activeConversation?.lines?.length ?? 0) === 0;

  useEffect(() => {
    let mounted = true;

    invoke<ConversationSummary[]>('list_conversations', {
      agentId: agentId || null,
    })
      .then((storedConversations) => {
        if (!mounted || !Array.isArray(storedConversations) || storedConversations.length === 0) {
          return;
        }

        setConversations((prevConversations) => {
          // Only keep the fresh local draft (first item), replace stored conversations
          const localDrafts = prevConversations.filter(
            (c) => c.lines.length === 0 && !storedConversations.some((s) => s.conversationId === c.conversationId)
          );
          const restored = storedConversations.map(restoreConversationPreview);
          return [...localDrafts, ...restored];
        });
      })
      .catch(() => {
        // Keep local fallback conversation list when backend history is unavailable.
      });

    return () => {
      mounted = false;
    };
  }, [agentId]);

  useEffect(() => {
    const selectedConversation = conversations.find(
      (conversation) => conversation.conversationId === activeConversationId
    );
    if (!selectedConversation || selectedConversation.isLoaded) {
      return undefined;
    }

    let disposed = false;
    invoke<ConversationDetail>('load_conversation', {
      req: {
        conversationId: selectedConversation.conversationId,
        model: selectedModelId,
        agentId: agentId || null,
      },
    })
      .then((detail) => {
        if (disposed || !detail) return;

        const hydratedDetail = {
          ...detail,
          lines: hydrateConversationLines(detail.lines || []),
        };
        setConversations((prevConversations) =>
          applyLoadedConversationDetail(
            prevConversations,
            selectedConversation.conversationId,
            hydratedDetail
          )
        );
      })
      .catch(() => {});

    return () => {
      disposed = true;
    };
  }, [activeConversationId, conversations, selectedModelId]);

  const createNewChat = useCallback(() => {
    if (activeConversation && isConversationEmpty(activeConversation)) {
      setActiveConversationId(activeConversation.conversationId);
      return;
    }

    const newConversation = createLocalConversation();
    setConversations((prevConversations) => [newConversation, ...prevConversations]);
    setActiveConversationId(newConversation.conversationId);
  }, [activeConversation]);

  const renameConversation = useCallback(
    async (conversationId: string, title: string) => {
      const nextTitle = (title || '').trim();
      if (!nextTitle) return;

      const previousConversation = conversations.find(
        (conversation) => conversation.conversationId === conversationId
      );
      if (!previousConversation) return;

      setConversations((prevConversations) =>
        applyManualConversationRename(prevConversations, conversationId, nextTitle)
      );

      try {
        const updatedSummary = await invoke<ConversationSummary>('rename_conversation', {
          req: {
            conversationId,
            title: nextTitle,
            agentId: agentId || null,
          },
        });

        if (updatedSummary?.title) {
          setConversations((prevConversations) =>
            applyManualConversationRename(
              prevConversations,
              conversationId,
              updatedSummary.title || nextTitle
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
    },
    [agentId, conversations]
  );

  const requestDeleteConversation = useCallback(
    (conversationId: string) => {
      const targetConversation = conversations.find(
        (conversation) => conversation.conversationId === conversationId
      );
      if (!targetConversation) return;
      setConversationToDelete(targetConversation);
    },
    [conversations]
  );

  const cancelDeleteConversation = useCallback(() => {
    setConversationToDelete(null);
  }, []);

  const confirmDeleteConversation = useCallback(async (
    clearConversationRequestState: (conversationId: string) => void
  ) => {
    if (!conversationToDelete) return;
    const conversationId = conversationToDelete.conversationId;
    const targetConversation = conversationToDelete;
    setConversationToDelete(null);
    clearConversationRequestState(conversationId);

    let rollbackConversation = targetConversation;
    let optimisticNextActiveId: string | null = null;

    setConversations((prevConversations) => {
      const currentTarget = prevConversations.find(
        (conversation) => conversation.conversationId === conversationId
      );
      if (!currentTarget) {
        return prevConversations;
      }

      rollbackConversation = currentTarget;
      const remainingConversations = prevConversations.filter(
        (conversation) => conversation.conversationId !== conversationId
      );

      if (remainingConversations.length > 0) {
        optimisticNextActiveId = remainingConversations[0].conversationId;
        return remainingConversations;
      }

      const fallbackConversation = createLocalConversation();
      optimisticNextActiveId = fallbackConversation.conversationId;
      return [fallbackConversation];
    });

    setActiveConversationId((prevActiveConversationId) =>
      prevActiveConversationId === conversationId
        ? optimisticNextActiveId || prevActiveConversationId
        : prevActiveConversationId
    );

    try {
      const deleted = await invoke<boolean>('delete_conversation', {
        req: { conversationId, agentId: agentId || null },
      });

      if (deleted === false) {
        console.warn('[delete_conversation] conversation file not found for id:', conversationId);
      }

      const storedConversations = await invoke<ConversationSummary[]>('list_conversations', {
        agentId: agentId || null,
      });
      if (Array.isArray(storedConversations)) {
        let nextConversationIds = new Set<string>();
        let fallbackActiveId: string | null = null;

        setConversations((prevConversations) => {
          const rebuilt = rebuildConversationListAfterDeletion(
            prevConversations,
            storedConversations
          );
          nextConversationIds = rebuilt.conversationIds;
          fallbackActiveId = rebuilt.activeConversationId;
          return rebuilt.conversations;
        });

        setActiveConversationId((prevActiveConversationId) => {
          if (nextConversationIds.has(prevActiveConversationId)) {
            return prevActiveConversationId;
          }
          return fallbackActiveId || prevActiveConversationId;
        });
      }
    } catch {
      setConversations((prevConversations) => {
        const exists = prevConversations.some(
          (conversation) => conversation.conversationId === conversationId
        );
        if (exists) return prevConversations;
        return [rollbackConversation, ...prevConversations];
      });
      setActiveConversationId((prevActiveConversationId) =>
        prevActiveConversationId === optimisticNextActiveId
          ? conversationId
          : prevActiveConversationId
      );
      console.error('[delete_conversation] invoke failed for id:', conversationId);
    }
  }, [agentId, conversationToDelete]);

  return {
    activeChatTitle,
    activeConversation,
    activeConversationId,
    cancelDeleteConversation,
    confirmDeleteConversation,
    conversationToDelete,
    conversations,
    createNewChat,
    isEmptyConversation,
    renameConversation,
    requestDeleteConversation,
    setActiveConversationId,
    setConversations,
    sidebarConversations,
  };
}
