import { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  createLocalConversation,
  getConversationDisplayTitle,
  hydrateConversationLines,
  isConversationEmpty,
  shouldShowConversationInSidebar,
} from '../features/conversations/conversationUtils';
import type {
  ChatRequestOptions,
  ChatStreamResponse,
  Conversation,
  ConversationDetail,
  ConversationSummary,
  ModelOption,
} from '../features/conversations/types';
import { DEFAULT_REASONING_MODE } from '../features/models/modelCatalog';
import {
  applyLoadedConversationDetail,
  applyManualConversationRename,
  applyOptimisticUserMessage,
  applySendFailure,
  applySendResponse,
  mergeStoredConversationsWithDrafts,
  parseAdvancedRequestOptions,
  rebuildConversationListAfterDeletion,
} from '../features/conversations/sessionState';
import { useConversationStreaming } from './useConversationStreaming';

interface ComposerAttachment {
  name: string;
  type: string;
}

interface SendMessageOptions {
  appendUserMessage?: boolean;
  conversationIdOverride?: string | null;
  requestOptions?: ChatRequestOptions;
}

interface UseChatSessionsOptions {
  selectedModel: string;
  selectedModelOption: ModelOption | null;
  selectedReasoningMode: string;
  showAdvancedRequestOptionsButton: boolean;
}

export function useChatSessions({
  selectedModel,
  selectedModelOption,
  selectedReasoningMode,
  showAdvancedRequestOptionsButton,
}: UseChatSessionsOptions) {
  const initialConversation = useMemo(() => createLocalConversation(), []);
  const [input, setInput] = useState('');
  const [advancedRequestOptionsInput, setAdvancedRequestOptionsInput] = useState('');
  const [advancedRequestOptionsError, setAdvancedRequestOptionsError] = useState<string | null>(
    null
  );
  const [attachment, setAttachment] = useState<ComposerAttachment | null>(null);
  const [conversations, setConversations] = useState<Conversation[]>(() => [initialConversation]);
  const [conversationToDelete, setConversationToDelete] = useState<Conversation | null>(null);
  const [activeConversationId, setActiveConversationId] = useState(
    initialConversation.conversationId
  );

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

  const {
    beginConversationRequest,
    clearConversationRequestState,
    finishConversationRequest,
    generatingConversationIds,
    hasAnyGenerating,
    hasPendingRequest,
    isConversationGenerating,
    isConversationStopping,
    isConversationThinking,
    markConversationThinking,
    stopConversationRequest,
    wasRequestStopped,
  } = useConversationStreaming({ setConversations });

  const activeConversationIsGenerating = Boolean(
    activeConversation?.conversationId &&
      isConversationGenerating(activeConversation.conversationId)
  );
  const activeConversationIsStopping = Boolean(
    activeConversation?.conversationId &&
      isConversationStopping(activeConversation.conversationId)
  );
  const activeConversationIsThinking = Boolean(
    activeConversation?.conversationId &&
      isConversationThinking(activeConversation.conversationId)
  );
  const isEmptyConversation = (activeConversation?.lines?.length ?? 0) === 0;

  useEffect(() => {
    let mounted = true;

    invoke<ConversationSummary[]>('list_conversations')
      .then((storedConversations) => {
        if (!mounted || !Array.isArray(storedConversations) || storedConversations.length === 0) {
          return;
        }

        setConversations((prevConversations) =>
          mergeStoredConversationsWithDrafts(prevConversations, storedConversations)
        );
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
    if (!selectedConversation || selectedConversation.isLoaded) {
      return undefined;
    }

    let disposed = false;
    invoke<ConversationDetail>('load_conversation', {
      req: { conversationId: selectedConversation.conversationId },
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
  }, [activeConversationId, conversations]);

  const sendMessage = useCallback(
    async (
      textToSend?: string,
      options: SendMessageOptions = {}
    ) => {
      const { appendUserMessage = true, conversationIdOverride = null, requestOptions } = options;
      const text = (textToSend ?? input).trim();
      if (!text || !activeConversation) return;

      let advancedRequestOptions: ChatRequestOptions = requestOptions || {};
      if (!requestOptions && showAdvancedRequestOptionsButton) {
        try {
          advancedRequestOptions = parseAdvancedRequestOptions(advancedRequestOptionsInput);
          if (advancedRequestOptionsError) {
            setAdvancedRequestOptionsError(null);
          }
        } catch (error) {
          const message =
            error instanceof Error ? error.message : '高级请求参数解析失败，请检查 JSON 格式。';
          setAdvancedRequestOptionsError(message);
          return;
        }
      } else if (!requestOptions && advancedRequestOptionsError) {
        setAdvancedRequestOptionsError(null);
      }

      if (appendUserMessage) {
        setInput('');
        setAttachment(null);
      }

      const currentConversationId = conversationIdOverride ?? activeConversation.conversationId;
      if (isConversationGenerating(currentConversationId) || hasPendingRequest(currentConversationId)) {
        return;
      }

      const requestId = `req-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      beginConversationRequest(currentConversationId, requestId);
      setConversations((prevConversations) =>
        applyOptimisticUserMessage(
          prevConversations,
          currentConversationId,
          requestId,
          text,
          appendUserMessage
        )
      );

      const selectedReasoningEffort =
        selectedModelOption?.profileKey === selectedModel &&
        selectedReasoningMode !== DEFAULT_REASONING_MODE
          ? selectedReasoningMode
          : null;

      try {
        const response = await invoke<ChatStreamResponse>('chat_stream', {
          req: {
            input: text,
            conversationId: currentConversationId,
            model: selectedModel,
            reasoningEffort: selectedReasoningEffort,
            text: advancedRequestOptions.text,
            include: advancedRequestOptions.include,
            serviceTier: advancedRequestOptions.serviceTier,
            promptCacheKey: advancedRequestOptions.promptCacheKey,
            clientMetadata: advancedRequestOptions.clientMetadata,
            generate: advancedRequestOptions.generate,
            requestId,
          },
        });

        setConversations((prevConversations) =>
          applySendResponse(
            prevConversations,
            currentConversationId,
            requestId,
            response.outputText || '',
            response.responseId,
            response.conversationTitle,
            wasRequestStopped(requestId)
          )
        );
      } catch (error: unknown) {
        markConversationThinking(currentConversationId, false);
        const errorText =
          typeof error === 'string'
            ? error
            : '请求失败，请检查配置文件中的 credential / api_endpoint 和网络连接。';
        setConversations((prevConversations) =>
          applySendFailure(prevConversations, currentConversationId, requestId, errorText || text)
        );
      } finally {
        finishConversationRequest(currentConversationId, requestId);
      }
    },
    [
      activeConversation,
      advancedRequestOptionsError,
      advancedRequestOptionsInput,
      beginConversationRequest,
      finishConversationRequest,
      hasPendingRequest,
      input,
      isConversationGenerating,
      markConversationThinking,
      selectedModel,
      selectedModelOption,
      selectedReasoningMode,
      showAdvancedRequestOptionsButton,
      wasRequestStopped,
    ]
  );

  const stopActiveStream = useCallback(async () => {
    const currentConversationId = activeConversation?.conversationId;
    if (!currentConversationId) return;
    await stopConversationRequest(currentConversationId);
  }, [activeConversation, stopConversationRequest]);

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
    [conversations]
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

  const confirmDeleteConversation = useCallback(async () => {
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
        req: { conversationId },
      });

      if (deleted === false) {
        console.warn('[delete_conversation] conversation file not found for id:', conversationId);
      }

      const storedConversations = await invoke<ConversationSummary[]>('list_conversations');
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
  }, [clearConversationRequestState, conversationToDelete]);

  const attachPlaceholderFile = useCallback(() => {
    setAttachment({
      name: 'screenshot_data.png',
      type: 'image',
    });
  }, []);

  const removeAttachment = useCallback(() => {
    setAttachment(null);
  }, []);

  const updateAdvancedRequestOptionsInput = useCallback(
    (value: string) => {
      setAdvancedRequestOptionsInput(value);
      if (advancedRequestOptionsError) {
        setAdvancedRequestOptionsError(null);
      }
    },
    [advancedRequestOptionsError]
  );

  return {
    activeChatTitle,
    activeConversation,
    activeConversationId,
    activeConversationIsGenerating,
    activeConversationIsStopping,
    activeConversationIsThinking,
    advancedRequestOptionsError,
    advancedRequestOptionsInput,
    attachment,
    attachPlaceholderFile,
    cancelDeleteConversation,
    confirmDeleteConversation,
    conversationToDelete,
    conversations,
    createNewChat,
    generatingConversationIds,
    hasAnyGenerating,
    input,
    isEmptyConversation,
    removeAttachment,
    renameConversation,
    requestDeleteConversation,
    sendMessage,
    setActiveConversationId,
    setInput,
    sidebarConversations,
    stopActiveStream,
    updateAdvancedRequestOptionsInput,
  };
}
