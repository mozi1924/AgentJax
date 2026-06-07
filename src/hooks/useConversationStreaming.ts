import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { Dispatch, SetStateAction } from 'react';
import type { ChatStreamEventPayload, Conversation } from '../features/conversations/types';
import {
  appendPendingToolCall,
  applyAssistantDelta,
  applyAssistantMessage,
  applyCompletedRequest,
  applyConversationTitleUpdate,
  applyStreamError,
  applyThinkingDelta,
  applyToolDelta,
  applyToolExecution,
  applyToolProgress,
  normalizeAssistantPhase,
} from '../features/conversations/sessionState';
import { tryGetCurrentWindow } from '../features/tauri/runtime';

interface StreamRequestMapping {
  conversationId: string;
  lastEventIndex: number;
}

interface UseConversationStreamingOptions {
  setConversations: Dispatch<SetStateAction<Conversation[]>>;
}

const updateConversationIdSet = (
  setIds: Dispatch<SetStateAction<Set<string>>>,
  conversationId: string,
  enabled: boolean
) => {
  if (!conversationId) return;
  setIds((prev) => {
    const next = new Set(prev);
    if (enabled) {
      next.add(conversationId);
    } else {
      next.delete(conversationId);
    }
    return next;
  });
};

export function useConversationStreaming({ setConversations }: UseConversationStreamingOptions) {
  const [generatingConversationIds, setGeneratingConversationIds] = useState<Set<string>>(
    () => new Set()
  );
  const [stoppingConversationIds, setStoppingConversationIds] = useState<Set<string>>(
    () => new Set()
  );
  const [pendingStreetCount, setPendingStreetCount] = useState(0);

  const [thinkingConversationIds, setThinkingConversationIds] = useState<Set<string>>(
    () => new Set()
  );

  const streamRequestMapRef = useRef<Record<string, StreamRequestMapping>>({});
  const activeConversationRequestRef = useRef<Record<string, string>>({});
  const stoppedRequestIdsRef = useRef<Set<string>>(new Set());

  const markConversationGenerating = useCallback((conversationId: string, isGenerating: boolean) => {
    updateConversationIdSet(setGeneratingConversationIds, conversationId, isGenerating);
  }, []);

  const markConversationStopping = useCallback((conversationId: string, isStopping: boolean) => {
    updateConversationIdSet(setStoppingConversationIds, conversationId, isStopping);
  }, []);

  const markConversationThinking = useCallback((conversationId: string, isThinking: boolean) => {
    updateConversationIdSet(setThinkingConversationIds, conversationId, isThinking);
  }, []);

  const clearConversationRequestState = useCallback(
    (conversationId: string) => {
      if (!conversationId) return;

      const requestId = activeConversationRequestRef.current[conversationId];
      if (requestId) {
        delete activeConversationRequestRef.current[conversationId];
        stoppedRequestIdsRef.current.delete(requestId);
        delete streamRequestMapRef.current[requestId];
      }

      Object.entries(streamRequestMapRef.current).forEach(([candidateRequestId, mapping]) => {
        if (mapping?.conversationId === conversationId) {
          delete streamRequestMapRef.current[candidateRequestId];
          stoppedRequestIdsRef.current.delete(candidateRequestId);
        }
      });

      markConversationGenerating(conversationId, false);
      markConversationStopping(conversationId, false);
      markConversationThinking(conversationId, false);
    },
    [markConversationGenerating, markConversationStopping, markConversationThinking]
  );

  const beginConversationRequest = useCallback(
    (conversationId: string, requestId: string) => {
      streamRequestMapRef.current[requestId] = {
        conversationId,
        lastEventIndex: 0,
      };
      activeConversationRequestRef.current[conversationId] = requestId;
      markConversationGenerating(conversationId, true);
      markConversationStopping(conversationId, false);
      markConversationThinking(conversationId, true);
    },
    [markConversationGenerating, markConversationStopping, markConversationThinking]
  );

  const finishConversationRequest = useCallback(
    (conversationId: string, requestId: string) => {
      delete streamRequestMapRef.current[requestId];
      stoppedRequestIdsRef.current.delete(requestId);
      if (activeConversationRequestRef.current[conversationId] === requestId) {
        delete activeConversationRequestRef.current[conversationId];
      }
      markConversationGenerating(conversationId, false);
      markConversationStopping(conversationId, false);
      markConversationThinking(conversationId, false);
    },
    [markConversationGenerating, markConversationStopping, markConversationThinking]
  );

  const hasPendingRequest = useCallback((conversationId: string) => {
    return Boolean(activeConversationRequestRef.current[conversationId]);
  }, []);

  const wasRequestStopped = useCallback((requestId: string) => {
    return stoppedRequestIdsRef.current.has(requestId);
  }, []);

  const isConversationGenerating = useCallback(
    (conversationId: string) => generatingConversationIds.has(conversationId),
    [generatingConversationIds]
  );

  const isConversationStopping = useCallback(
    (conversationId: string) => stoppingConversationIds.has(conversationId),
    [stoppingConversationIds]
  );

  const isConversationThinking = useCallback(
    (conversationId: string) => thinkingConversationIds.has(conversationId),
    [thinkingConversationIds]
  );

  const stopConversationRequest = useCallback(
    async (conversationId: string) => {
      const requestId = activeConversationRequestRef.current[conversationId];
      if (!requestId || stoppingConversationIds.has(conversationId)) return;

      stoppedRequestIdsRef.current.add(requestId);
      markConversationStopping(conversationId, true);

      try {
        await invoke('cancel_chat_stream', {
          req: { requestId },
        });
      } catch {
        markConversationStopping(conversationId, false);
      }
    },
    [markConversationStopping, stoppingConversationIds]
  );

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;

    const setup = async () => {
      const currentWindow = tryGetCurrentWindow();
      if (!currentWindow) {
        return;
      }

      unlisten = await currentWindow.listen<ChatStreamEventPayload>(
        'chat_stream_event',
        (event) => {
          const payload = event?.payload || {};
          const conversationId = payload.conversationId;

          if (payload.kind === 'title' && conversationId && payload.conversationTitle) {
            setConversations((prev) =>
              applyConversationTitleUpdate(prev, conversationId, payload.conversationTitle || '')
            );
            return;
          }

          // Street notifications: cross-turn events for async work results.
          if (payload.kind === 'street_notification' && conversationId) {
            setPendingStreetCount((prev) => prev + 1);
            // Auto-trigger: if priority meets threshold, start a new turn.
            // The priority is stored in toolName field by the backend.
            const priority = payload.toolName || 'normal';
            const threshold = 'urgent'; // TODO: read from config
            const priorityLevels: Record<string, number> = { low: 0, normal: 1, high: 2, urgent: 3 };
            if ((priorityLevels[priority] || 0) >= (priorityLevels[threshold] || 3)) {
              // High-priority notification — could auto-trigger a turn here.
              // For now, just log; full auto-trigger requires conversation state.
              console.log(`[Street] High-priority notification (${priority}): auto-trigger candidate`);
            }
            return;
          }

          if (payload.kind === 'street_cleared') {
            setPendingStreetCount(0);
            return;
          }

          const requestId = payload.requestId;
          const mapping = requestId ? streamRequestMapRef.current[requestId] : null;
          if (!mapping) return;
          if (conversationId && conversationId !== mapping.conversationId) return;

          const eventIndex = Number(payload.eventIndex || 0);
          if (eventIndex > 0) {
            const lastEventIndex = Number(mapping.lastEventIndex || 0);
            if (eventIndex <= lastEventIndex) {
              return;
            }
            mapping.lastEventIndex = eventIndex;
          }

          if (payload.kind === 'thinking') {
            markConversationThinking(mapping.conversationId, true);
            return;
          }

          if (payload.kind === 'thinking_delta' && payload.delta) {
            setConversations((prev) =>
              applyThinkingDelta(
                prev,
                mapping.conversationId,
                requestId || '',
                String(payload.delta),
                payload.eventIndex
              )
            );
            return;
          }

          if (payload.kind === 'thinking_completed') {
            return;
          }

          if (payload.kind === 'output_started') {
            markConversationThinking(mapping.conversationId, false);
            return;
          }

          if (payload.kind === 'delta' && payload.delta) {
            markConversationThinking(mapping.conversationId, false);
            setConversations((prev) => {
              const updated = applyAssistantDelta(
                prev,
                mapping.conversationId,
                requestId || '',
                String(payload.delta),
                normalizeAssistantPhase(payload.phase),
                payload.eventIndex
              );
              return typeof payload.contextTokenCount === 'number'
                ? updated.map((conv) =>
                    conv.conversationId === mapping.conversationId
                      ? { ...conv, contextTokenCount: payload.contextTokenCount! }
                      : conv
                  )
                : updated;
            });
            return;
          }

          if (payload.kind === 'assistant_message') {
            markConversationThinking(mapping.conversationId, false);
            setConversations((prev) => {
              const updated = applyAssistantMessage(
                prev,
                mapping.conversationId,
                requestId || '',
                String(payload.delta || ''),
                normalizeAssistantPhase(payload.phase),
                payload.responseId,
                payload.eventIndex
              );
              return typeof payload.contextTokenCount === 'number'
                ? updated.map((conv) =>
                    conv.conversationId === mapping.conversationId
                      ? { ...conv, contextTokenCount: payload.contextTokenCount! }
                      : conv
                  )
                : updated;
            });
            return;
          }

          if (payload.kind === 'tool_call_started' || payload.kind === 'tool_call_done') {
            markConversationThinking(mapping.conversationId, false);
            setConversations((prev) => {
              const updated = appendPendingToolCall(
                prev,
                mapping.conversationId,
                requestId || '',
                payload.toolCallId,
                payload.toolName,
                payload.toolDisplayName,
                payload.toolDescription,
                payload.toolIcon,
                payload.toolArguments
              );
              return typeof payload.contextTokenCount === 'number'
                ? updated.map((conv) =>
                    conv.conversationId === mapping.conversationId
                      ? { ...conv, contextTokenCount: payload.contextTokenCount! }
                      : conv
                  )
                : updated;
            });
            return;
          }

          if (payload.kind === 'tool_call_exec') {
            setConversations((prev) => {
              const updated = applyToolExecution(
                prev,
                mapping.conversationId,
                payload.toolCallId,
                payload.toolOutput,
                payload.toolDisplayName,
                payload.toolDescription,
                payload.toolIcon,
                payload.toolStatus,
                payload.toolStartedTs,
                payload.toolCompletedTs
              );
              return typeof payload.contextTokenCount === 'number'
                ? updated.map((conv) =>
                    conv.conversationId === mapping.conversationId
                      ? { ...conv, contextTokenCount: payload.contextTokenCount! }
                      : conv
                  )
                : updated;
            });
            return;
          }

          if (payload.kind === 'tool_call_progress') {
            setConversations((prev) =>
              applyToolProgress(
                prev,
                mapping.conversationId,
                payload.toolCallId,
                payload.toolDisplayName,
                payload.toolDescription,
                payload.toolIcon
              )
            );
            return;
          }

          if (payload.kind === 'tool_call_delta') {
            setConversations((prev) =>
              applyToolDelta(prev, mapping.conversationId, payload.toolCallId, payload.delta)
            );
            return;
          }

          if (payload.kind === 'token_usage') {
            if (typeof payload.contextTokenCount === 'number') {
              setConversations((prev) =>
                prev.map((conv) =>
                  conv.conversationId === mapping.conversationId
                    ? { ...conv, contextTokenCount: payload.contextTokenCount! }
                    : conv
                )
              );
            }
            return;
          }

          if (payload.kind === 'error') {
            markConversationGenerating(mapping.conversationId, false);
            markConversationStopping(mapping.conversationId, false);
            markConversationThinking(mapping.conversationId, false);
            const errorMsg = payload.error || 'An error occurred';
            setConversations((prev) =>
              applyStreamError(prev, mapping.conversationId, errorMsg)
            );
            return;
          }

          if (payload.kind === 'done') {
            markConversationGenerating(mapping.conversationId, false);
            markConversationStopping(mapping.conversationId, false);
            markConversationThinking(mapping.conversationId, false);
            setConversations((prev) =>
              applyCompletedRequest(
                prev,
                mapping.conversationId,
                requestId || '',
                payload.responseId,
                payload.delta,
                payload.conversationTitle,
                // Prefer the final backend count here; when provider usage is
                // available this value has already replaced local estimates.
                payload.contextTokenCount
              )
            );
          }
        }
      );

      if (disposed && unlisten) {
        unlisten();
        unlisten = null;
      }
    };

    void setup();

    return () => {
      disposed = true;
      if (unlisten) {
        unlisten();
      }
    };
  }, [
    markConversationGenerating,
    markConversationStopping,
    markConversationThinking,
    setConversations,
  ]);

  return {
    beginConversationRequest,
    clearConversationRequestState,
    finishConversationRequest,
    generatingConversationIds,
    hasPendingRequest,
    hasAnyGenerating: generatingConversationIds.size > 0,
    isConversationGenerating,
    isConversationStopping,
    isConversationThinking,
    markConversationThinking,
    pendingStreetCount,
    setPendingStreetCount,
    stopConversationRequest,
    stoppingConversationIds,
    thinkingConversationIds,
    wasRequestStopped,
  };
}
