import {
  applyConversationTitle,
  buildDraftConversationTitle,
  getLastVisibleConversationText,
} from './conversationUtils';
import {
  countVisibleMessages,
  ensureAtLeastOneConversation,
  restoreConversationPreview,
} from './conversationState';
import type {
  AssistantLine,
  ChatRequestOptions,
  Conversation,
  ConversationDetail,
  ConversationSummary,
  ToolLine,
} from './types';

const parsePossiblyJson = (value: string | undefined): unknown => {
  if (!value) {
    return undefined;
  }

  try {
    return JSON.parse(value);
  } catch {
    return value;
  }
};

const inferToolStatus = (
  toolOutput: string | undefined,
  explicitStatus?: 'pending' | 'done' | 'failed'
): ToolLine['status'] => {
  if (explicitStatus) {
    return explicitStatus;
  }

  const parsed = parsePossiblyJson(toolOutput);
  if (parsed && typeof parsed === 'object') {
    const record = parsed as Record<string, unknown>;
    if (record.ok === false || record.error) {
      return 'failed';
    }
  }

  return 'done';
};

export const parseAdvancedRequestOptions = (raw: string): ChatRequestOptions => {
  const trimmed = raw.trim();
  if (!trimmed) {
    return {};
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch {
    throw new Error('高级请求参数不是合法 JSON。');
  }

  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error('高级请求参数必须是 JSON 对象。');
  }

  const source = parsed as Record<string, unknown>;
  const out: ChatRequestOptions = {};

  if (source.text !== undefined) {
    out.text = source.text;
  }

  if (source.include !== undefined) {
    if (!Array.isArray(source.include)) {
      throw new Error('`include` 必须是字符串数组。');
    }
    out.include = source.include
      .map((item) => `${item ?? ''}`.trim())
      .filter((item) => item.length > 0);
  }

  if (source.serviceTier !== undefined) {
    const value = `${source.serviceTier ?? ''}`.trim();
    if (value) {
      out.serviceTier = value;
    }
  }

  if (source.promptCacheKey !== undefined) {
    const value = `${source.promptCacheKey ?? ''}`.trim();
    if (value) {
      out.promptCacheKey = value;
    }
  }

  if (source.clientMetadata !== undefined) {
    if (
      !source.clientMetadata ||
      typeof source.clientMetadata !== 'object' ||
      Array.isArray(source.clientMetadata)
    ) {
      throw new Error('`clientMetadata` 必须是 JSON 对象。');
    }
    out.clientMetadata = source.clientMetadata as Record<string, unknown>;
  }

  if (source.generate !== undefined) {
    if (typeof source.generate !== 'boolean') {
      throw new Error('`generate` 必须是布尔值。');
    }
    out.generate = source.generate;
  }

  return out;
};

export const normalizeAssistantPhase = (
  phase: AssistantLine['phase'] | undefined
): AssistantLine['phase'] => {
  if (phase === 'commentary' || phase === 'final_answer') {
    return phase;
  }
  return null;
};

const updateConversation = (
  conversations: Conversation[],
  conversationId: string,
  updater: (conversation: Conversation) => Conversation
): Conversation[] =>
  conversations.map((conversation) =>
    conversation.conversationId === conversationId ? updater(conversation) : conversation
  );

const isDraftAssistantLineForRequest = (
  line: Conversation['lines'][number] | undefined,
  requestId: string
): line is AssistantLine =>
  Boolean(
    line &&
      line.kind === 'assistant' &&
      (line as AssistantLine).status === 'draft' &&
      (line as AssistantLine).requestId === requestId
  );

const isPhaseCompatibleForDraftReuse = (
  draftPhase: AssistantLine['phase'],
  incomingPhase: AssistantLine['phase']
): boolean => {
  // Unknown-phase deltas/messages should not attach themselves onto an
  // already-classified commentary/final draft, otherwise two phases can blur.
  if (incomingPhase == null) {
    return draftPhase == null;
  }
  return draftPhase == null || draftPhase === incomingPhase;
};

const findReusableDraftAssistantIndex = (
  lines: Conversation['lines'],
  requestId: string,
  phase: AssistantLine['phase']
): number => {
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    const line = lines[index];
    if (!isDraftAssistantLineForRequest(line, requestId)) continue;
    if (!isPhaseCompatibleForDraftReuse(line.phase, phase)) continue;
    return index;
  }
  return -1;
};

const hasVisibleFinalAssistantForRequest = (
  lines: Conversation['lines'],
  requestId: string
): boolean =>
  lines.some(
    (line) =>
      line.kind === 'assistant' &&
      line.requestId === requestId &&
      line.phase === 'final_answer' &&
      Boolean(line.text?.trim())
  );

const finalizeLingeringAssistantDrafts = (
  lines: Conversation['lines'],
  requestId: string,
  outputText: string,
  responseId?: string | null,
  wasStopped?: boolean
): Conversation['lines'] => {
  // When the request promise resolves, we only promote one non-commentary
  // draft to the final answer. Commentary drafts are merely closed out.
  let finalCandidateIndex = -1;
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    const line = lines[index];
    if (!isDraftAssistantLineForRequest(line, requestId)) continue;
    if (line.phase === 'commentary') continue;
    finalCandidateIndex = index;
    break;
  }

  const finalizedLines = lines.map((line, index) => {
    if (!isDraftAssistantLineForRequest(line, requestId)) {
      return line;
    }

    if (index === finalCandidateIndex) {
      return {
        ...line,
        text: outputText || line.text || (wasStopped ? '已停止' : ''),
        responseId: responseId || line.responseId,
        phase: line.phase ?? 'final_answer',
        status: 'done' as const,
      } satisfies AssistantLine;
    }

    return {
      ...line,
      responseId: responseId || line.responseId,
      status: 'done' as const,
    } satisfies AssistantLine;
  });

  if (
    finalCandidateIndex === -1 &&
    outputText &&
    !hasVisibleFinalAssistantForRequest(finalizedLines, requestId)
  ) {
    finalizedLines.push({
      kind: 'assistant' as const,
      id: `asst-${requestId}-final`,
      ts: Date.now(),
      requestId,
      responseId: responseId || '',
      phase: 'final_answer' as const,
      text: outputText,
      status: 'done' as const,
    } satisfies AssistantLine);
  }

  return finalizedLines;
};

export const applyLoadedConversationDetail = (
  conversations: Conversation[],
  conversationId: string,
  detail: ConversationDetail
): Conversation[] =>
  updateConversation(conversations, conversationId, (conversation) => {
    const hydratedLines = detail.lines || [];
    const lastMessagePreview =
      getLastVisibleConversationText(hydratedLines) || conversation.lastMessagePreview;

    return {
      ...conversation,
      title: detail.title || conversation.title,
      titleSource: detail.titleSource || conversation.titleSource,
      lastMessagePreview,
      messageCount: countVisibleMessages(hydratedLines),
      contextTokenCount: detail.contextTokenCount ?? conversation.contextTokenCount,
      isLoaded: true,
      lines: hydratedLines,
    };
  });

export const applyConversationTitleUpdate = (
  conversations: Conversation[],
  conversationId: string,
  title: string
): Conversation[] =>
  updateConversation(conversations, conversationId, (conversation) =>
    applyConversationTitle(conversation, title)
  );

export const applyAssistantDelta = (
  conversations: Conversation[],
  conversationId: string,
  requestId: string,
  deltaText: string,
  phase: AssistantLine['phase'],
  eventIndex?: number
): Conversation[] =>
  updateConversation(conversations, conversationId, (conversation) => {
    const lines = [...conversation.lines];
    const reusableDraftIndex = findReusableDraftAssistantIndex(lines, requestId, phase);
    if (reusableDraftIndex >= 0) {
      const draft = lines[reusableDraftIndex] as AssistantLine;
      lines[reusableDraftIndex] = {
        ...draft,
        phase: phase ?? draft.phase,
        text: String(draft.text) + deltaText,
      } as AssistantLine;
    } else {
      lines.push({
        kind: 'assistant' as const,
        id: `asst-${requestId}-${eventIndex || Date.now()}`,
        ts: Date.now(),
        requestId,
        responseId: '',
        phase,
        text: deltaText,
        status: 'draft' as const,
      });
    }
    return { ...conversation, lines };
  });

export const applyAssistantMessage = (
  conversations: Conversation[],
  conversationId: string,
  requestId: string,
  messageText: string,
  phase: AssistantLine['phase'],
  responseId?: string | null,
  eventIndex?: number
): Conversation[] =>
  updateConversation(conversations, conversationId, (conversation) => {
    const lines = [...conversation.lines];
    const reusableDraftIndex = findReusableDraftAssistantIndex(lines, requestId, phase);

    if (reusableDraftIndex >= 0) {
      const draft = lines[reusableDraftIndex] as AssistantLine;
      lines[reusableDraftIndex] = {
        ...draft,
        text: messageText || draft.text,
        responseId: responseId || draft.responseId,
        phase: phase ?? draft.phase,
        status: 'done' as const,
      };
    } else {
      lines.push({
        kind: 'assistant' as const,
        id: `asst-${requestId}-${eventIndex || Date.now()}`,
        ts: Date.now(),
        requestId,
        responseId: responseId || '',
        phase,
        text: messageText,
        status: 'done' as const,
      });
    }

    const shouldRefreshPreview =
      phase !== 'commentary' && Boolean((messageText || '').trim());

    return {
      ...conversation,
      lines,
      lastMessagePreview: shouldRefreshPreview
        ? messageText
        : conversation.lastMessagePreview,
      messageCount: countVisibleMessages(lines),
      isLoaded: true,
    };
  });

export const appendPendingToolCall = (
  conversations: Conversation[],
  conversationId: string,
  requestId: string,
  toolCallId?: string,
  toolName?: string,
  toolDisplayName?: string,
  toolDescription?: string,
  toolIcon?: string,
  toolArguments?: string
): Conversation[] =>
  updateConversation(conversations, conversationId, (conversation) => {
    const now = Date.now();
    const callId = toolCallId || '';
    const hasArguments = toolArguments !== undefined;
    const parsedArgs = hasArguments ? parsePossiblyJson(toolArguments) : '';
    let found = false;
    const lines = conversation.lines.map((line) => {
      if (line.kind === 'tool' && (line as ToolLine).callId === callId) {
        found = true;
        const toolLine = line as ToolLine;
        return {
          ...toolLine,
          name: toolName || toolLine.name || '',
          displayName: toolDisplayName || toolLine.displayName || null,
          description: toolDescription || toolLine.description || null,
          icon: toolIcon || toolLine.icon || null,
          args: hasArguments ? parsedArgs : toolLine.args,
          status: 'pending' as const,
        } satisfies ToolLine;
      }
      return line;
    });

    if (!found) {
      lines.push({
        kind: 'tool' as const,
        id: `tool-${requestId}-${callId}`,
        ts: now,
        startedTs: now,
        completedTs: null,
        requestId,
        callId,
        name: toolName || '',
        displayName: toolDisplayName || null,
        description: toolDescription || null,
        icon: toolIcon || null,
        args: parsedArgs,
        status: 'pending' as const,
      });
    }

    return { ...conversation, lines };
  });

export const applyToolExecution = (
  conversations: Conversation[],
  conversationId: string,
  toolCallId?: string,
  toolOutput?: string,
  toolDisplayName?: string,
  toolDescription?: string,
  toolIcon?: string,
  toolStatus?: 'pending' | 'done' | 'failed',
  toolStartedTs?: number,
  toolCompletedTs?: number
): Conversation[] =>
  updateConversation(conversations, conversationId, (conversation) => {
    const completedTs = toolCompletedTs || Date.now();
    const lines = conversation.lines.map((line) => {
      if (line.kind === 'tool' && (line as ToolLine).callId === toolCallId) {
        const toolLine = line as ToolLine;
        return {
          ...toolLine,
          ts: completedTs,
          startedTs: toolStartedTs || toolLine.startedTs || toolLine.ts,
          completedTs,
          displayName: toolDisplayName || toolLine.displayName || null,
          description: toolDescription || toolLine.description || null,
          icon: toolIcon || toolLine.icon || null,
          output: parsePossiblyJson(toolOutput),
          status: inferToolStatus(toolOutput, toolStatus),
        } satisfies ToolLine;
      }
      return line;
    });
    return { ...conversation, lines };
  });

export const applyToolProgress = (
  conversations: Conversation[],
  conversationId: string,
  toolCallId?: string,
  toolDisplayName?: string,
  toolDescription?: string,
  toolIcon?: string
): Conversation[] =>
  updateConversation(conversations, conversationId, (conversation) => {
    const lines = conversation.lines.map((line) => {
      if (line.kind === 'tool' && (line as ToolLine).callId === toolCallId) {
        const toolLine = line as ToolLine;
        return {
          ...toolLine,
          displayName: toolDisplayName || toolLine.displayName || null,
          description: toolDescription || toolLine.description || null,
          icon: toolIcon || toolLine.icon || null,
          status: 'pending' as const,
        } satisfies ToolLine;
      }
      return line;
    });
    return { ...conversation, lines };
  });

export const applyToolDelta = (
  conversations: Conversation[],
  conversationId: string,
  toolCallId?: string,
  delta?: string
): Conversation[] =>
  updateConversation(conversations, conversationId, (conversation) => {
    const lines = conversation.lines.map((line) => {
      if (line.kind === 'tool' && (line as ToolLine).callId === toolCallId) {
        const toolLine = line as ToolLine;
        return {
          ...toolLine,
          args:
            typeof toolLine.args === 'string'
              ? toolLine.args + (delta || '')
              : toolLine.args === undefined || toolLine.args === null
                ? delta || ''
              : toolLine.args,
        } satisfies ToolLine;
      }
      return line;
    });
    return { ...conversation, lines };
  });

export const applyCompletedRequest = (
  conversations: Conversation[],
  conversationId: string,
  requestId: string,
  responseId?: string | null,
  finalDelta?: string,
  conversationTitle?: string,
  contextTokenCount?: number
): Conversation[] =>
  updateConversation(conversations, conversationId, (conversation) => {
    const lines = conversation.lines.map((line) => {
      if (
        line.kind === 'assistant' &&
        line.requestId === requestId &&
        (line as AssistantLine).phase === 'final_answer'
      ) {
        return {
          ...line,
          responseId: responseId || (line as AssistantLine).responseId,
          status: 'done' as const,
        } satisfies AssistantLine;
      }
      return line;
    });
    const finalText = finalDelta && String(finalDelta).trim() ? String(finalDelta) : '';
    const hasAssistant = lines.some(
      (line) =>
        line.kind === 'assistant' &&
        line.requestId === requestId &&
        (line as AssistantLine).phase === 'final_answer'
    );
    const finalLines = hasAssistant
      ? lines
      : !finalText
        ? lines
        : [
            ...lines,
            {
              kind: 'assistant' as const,
              id: `asst-${requestId}-final`,
              ts: Date.now(),
              requestId,
              responseId: responseId || '',
              phase: 'final_answer' as const,
              text: finalText,
              status: 'done' as const,
            } satisfies AssistantLine,
          ];

    return applyConversationTitle(
      {
        ...conversation,
        lines: finalLines,
        lastMessagePreview: finalText || conversation.lastMessagePreview,
        messageCount: countVisibleMessages(finalLines),
        contextTokenCount:
          typeof contextTokenCount === 'number'
            ? contextTokenCount
            : conversation.contextTokenCount,
        isLoaded: true,
      },
      conversationTitle
    );
  });

export const applyOptimisticUserMessage = (
  conversations: Conversation[],
  conversationId: string,
  requestId: string,
  text: string,
  appendUserMessage: boolean
): Conversation[] =>
  updateConversation(conversations, conversationId, (conversation) => {
    const wasEmptyConversation = conversation.lines.length === 0;
    let nextLines = [...conversation.lines];
    let nextTitle = conversation.title;

    if (appendUserMessage) {
      if (wasEmptyConversation) {
        nextTitle = buildDraftConversationTitle(text);
      }
      nextLines.push({
        kind: 'user' as const,
        id: `u-${requestId}`,
        ts: Date.now(),
        requestId,
        text,
      });
    }

    return {
      ...conversation,
      title: nextTitle,
      lastMessagePreview: text,
      messageCount: countVisibleMessages(nextLines),
      lines: nextLines,
      isLoaded: true,
    };
  });

export const applySendResponse = (
  conversations: Conversation[],
  conversationId: string,
  requestId: string,
  outputText: string,
  responseId?: string | null,
  conversationTitle?: string,
  wasStopped?: boolean,
  contextTokenCount?: number
): Conversation[] =>
  updateConversation(conversations, conversationId, (conversation) => {
    const lines = finalizeLingeringAssistantDrafts(
      conversation.lines,
      requestId,
      outputText,
      responseId,
      wasStopped
    );

    return applyConversationTitle(
      {
        ...conversation,
        lines,
        lastMessagePreview: outputText || conversation.lastMessagePreview,
        messageCount: countVisibleMessages(lines),
        contextTokenCount:
          typeof contextTokenCount === 'number'
            ? contextTokenCount
            : conversation.contextTokenCount,
        isLoaded: true,
      },
      conversationTitle
    );
  });

export const applySendFailure = (
  conversations: Conversation[],
  conversationId: string,
  requestId: string,
  fallbackPreview: string
): Conversation[] =>
  updateConversation(conversations, conversationId, (conversation) => {
    const lines = conversation.lines.map((line) => {
      if (line.kind === 'assistant' && line.requestId === requestId) {
        return {
          ...line,
          text: '',
          phase: (line as AssistantLine).phase,
          status: 'done' as const,
        } satisfies AssistantLine;
      }
      return line;
    });
    return {
      ...conversation,
      lines,
      lastMessagePreview: fallbackPreview || conversation.lastMessagePreview,
      messageCount: countVisibleMessages(lines),
    };
  });

export const applyManualConversationRename = (
  conversations: Conversation[],
  conversationId: string,
  title: string
): Conversation[] =>
  updateConversation(conversations, conversationId, (conversation) => ({
    ...conversation,
    title,
    titleSource: 'manual',
  }));

export const mergeStoredConversationsWithDrafts = (
  currentConversations: Conversation[],
  storedConversations: ConversationSummary[]
): Conversation[] => {
  const localDrafts = currentConversations.filter((conversation) => conversation.lines.length === 0);
  const restoredConversations = storedConversations.map(restoreConversationPreview);
  return [...localDrafts, ...restoredConversations];
};

export const rebuildConversationListAfterDeletion = (
  currentConversations: Conversation[],
  storedConversations: ConversationSummary[]
): {
  activeConversationId: string;
  conversationIds: Set<string>;
  conversations: Conversation[];
} => {
  const localDrafts = currentConversations.filter((conversation) => conversation.lines.length === 0);
  const restoredConversations = storedConversations.map(restoreConversationPreview);
  const existingIds = new Set(
    restoredConversations.map((conversation) => conversation.conversationId)
  );
  const remainingLocalDrafts = localDrafts.filter(
    (conversation) => !existingIds.has(conversation.conversationId)
  );
  const conversations = ensureAtLeastOneConversation([
    ...remainingLocalDrafts,
    ...restoredConversations,
  ]);
  return {
    activeConversationId: conversations[0].conversationId,
    conversationIds: new Set(conversations.map((conversation) => conversation.conversationId)),
    conversations,
  };
};
