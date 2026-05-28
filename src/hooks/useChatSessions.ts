import { useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type {
  ChatRequestOptions,
  ChatStreamResponse,
  ModelOption,
} from '../features/conversations/types';
import { DEFAULT_REASONING_MODE } from '../features/models/modelCatalog';
import {
  applyOptimisticUserMessage,
  applySendFailure,
  applySendResponse,
} from '../features/conversations/sessionState';
import { useChatComposerState } from './useChatComposerState';
import { useConversationRegistry } from './useConversationRegistry';
import { useConversationStreaming } from './useConversationStreaming';

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
  const {
    advancedRequestOptionsError,
    advancedRequestOptionsInput,
    attachment,
    attachPlaceholderFile,
    clearComposerDraft,
    input,
    removeAttachment,
    resolveRequestOptions,
    setInput,
    updateAdvancedRequestOptionsInput,
  } = useChatComposerState();
  const {
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
  } = useConversationRegistry({
    selectedModelId: selectedModelOption?.modelId || selectedModel,
  });

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
  } = useConversationStreaming({
    setConversations,
  });
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

  const sendMessage = useCallback(
    async (
      textToSend?: string,
      options: SendMessageOptions = {}
    ) => {
      const { appendUserMessage = true, conversationIdOverride = null, requestOptions } = options;
      const text = (textToSend ?? input).trim();
      if (!text || !activeConversation) return;

      const advancedRequestOptions = resolveRequestOptions(showAdvancedRequestOptionsButton, requestOptions);
      if (!advancedRequestOptions) {
        return;
      }

      if (appendUserMessage) {
        clearComposerDraft();
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
            wasRequestStopped(requestId),
            response.contextTokenCount
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
      beginConversationRequest,
      clearComposerDraft,
      finishConversationRequest,
      hasPendingRequest,
      input,
      isConversationGenerating,
      markConversationThinking,
      resolveRequestOptions,
      selectedModel,
      selectedModelOption,
      selectedReasoningMode,
      setConversations,
      showAdvancedRequestOptionsButton,
      wasRequestStopped,
    ]
  );

  const stopActiveStream = useCallback(async () => {
    const currentConversationId = activeConversation?.conversationId;
    if (!currentConversationId) return;
    await stopConversationRequest(currentConversationId);
  }, [activeConversation, stopConversationRequest]);

  const confirmDeleteActiveConversation = useCallback(
    () => confirmDeleteConversation(clearConversationRequestState),
    [clearConversationRequestState, confirmDeleteConversation]
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
    confirmDeleteConversation: confirmDeleteActiveConversation,
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
