export const DEFAULT_CONVERSATION_TITLE = '新对话';

export function createConversationId() {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  return `conv-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

export function createLocalConversation(conversationId = createConversationId()) {
  return {
    conversationId,
    title: DEFAULT_CONVERSATION_TITLE,
    titleSource: 'pending',
    messages: [],
    lastResponseId: null,
    lastMessagePreview: '',
    messageCount: 0,
    isLoaded: true
  };
}

export function isConversationEmpty(conversation) {
  return Array.isArray(conversation?.messages) && conversation.messages.length === 0;
}

export function shouldShowConversationInSidebar(conversation) {
  if (!conversation) {
    return false;
  }

  if (Array.isArray(conversation.messages) && conversation.messages.length > 0) {
    return true;
  }

  return Number(conversation.messageCount || 0) > 0 || Boolean((conversation.lastMessagePreview || '').trim());
}

export function canUseNativeContextMenu(target) {
  if (!(target instanceof HTMLElement)) {
    return false;
  }

  if (target.closest('input, textarea, [contenteditable="true"]')) {
    return true;
  }

  return Boolean(target.closest('[data-native-context-menu="true"]'));
}

export function applyConversationTitle(conversation, nextTitle) {
  const title = (nextTitle || '').trim();
  if (!title) {
    return conversation;
  }

  return {
    ...conversation,
    title
  };
}

function compactText(rawText, maxLength = 28) {
  const cleaned = (rawText || '').split(/\s+/).filter(Boolean).join(' ');
  if (!cleaned) {
    return '';
  }

  if (cleaned.length <= maxLength) {
    return cleaned;
  }

  return `${cleaned.slice(0, Math.max(0, maxLength - 3))}...`;
}

export function buildDraftConversationTitle(rawText) {
  return compactText(rawText, 30) || DEFAULT_CONVERSATION_TITLE;
}

function getFirstUserMessageText(conversation) {
  if (!Array.isArray(conversation?.messages)) {
    return '';
  }

  const firstUserMessage = conversation.messages.find(
    (message) => message.role === 'user' && (message.text || '').trim()
  );

  return firstUserMessage?.text || '';
}

export function getConversationDisplayTitle(conversation) {
  const explicitTitle = (conversation?.title || '').trim();
  if (
    explicitTitle &&
    (conversation?.titleSource === 'manual' || explicitTitle !== DEFAULT_CONVERSATION_TITLE)
  ) {
    return explicitTitle;
  }

  const fallbackTitle = getFirstUserMessageText(conversation) || conversation?.lastMessagePreview || '';
  return compactText(fallbackTitle, 30) || DEFAULT_CONVERSATION_TITLE;
}

function createConversationMessage(message) {
  const toolCallsMap = {};

  if (Array.isArray(message.timelineEvents)) {
    for (const item of message.timelineEvents) {
      if (item.type === 'toolCall') {
        const callId = item.callId;
        if (callId) {
          toolCallsMap[callId] = {
            id: callId,
            name: item.name,
            arguments: typeof item.arguments === 'object' ? JSON.stringify(item.arguments) : item.arguments || '',
            output: typeof item.output === 'object' ? JSON.stringify(item.output) : item.output || '',
            status: item.status === 'success' ? 'executed' : 'failed',
            durationMs: item.durationMs
          };
        }
      }
    }
  }

  if (Array.isArray(message.contextItems)) {
    for (const item of message.contextItems) {
      if (item.type === 'function_call') {
        const callId = item.call_id || item.callId;
        if (callId && !toolCallsMap[callId]) {
          toolCallsMap[callId] = {
            id: callId,
            name: item.name,
            arguments: item.arguments || '',
            output: '',
            status: 'arguments_done'
          };
        }
      }
    }

    for (const item of message.contextItems) {
      if (item.type === 'function_call_output') {
        const callId = item.call_id || item.callId;
        if (callId && toolCallsMap[callId]) {
          if (!toolCallsMap[callId].output) {
            toolCallsMap[callId].output = item.output || '';
            toolCallsMap[callId].status = 'executed';
          }
        }
      }
    }
  }

  return {
    id: message.id,
    role: message.role,
    text: message.text || '',
    status: 'done',
    errorText: '',
    retryable: false,
    retryInput: '',
    retryConversationId: null,
    createdAtUnixMs: message.createdAtUnixMs || 0,
    responseId: message.responseId || null,
    timelineEvents: message.timelineEvents || null,
    toolCalls: Object.values(toolCallsMap)
  };
}

function findRetryableUserMessage(messages) {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (!message || !(message.text || '').trim()) {
      continue;
    }

    if (message.role === 'assistant') {
      return null;
    }

    if (message.role === 'user') {
      return message;
    }
  }

  return null;
}

export function createInterruptedAssistantMessage(userMessage, conversationId) {
  if (!userMessage || !(userMessage.text || '').trim()) {
    return null;
  }

  return {
    id: `retry-${userMessage.id}`,
    role: 'assistant',
    text: '',
    status: 'interrupted',
    errorText: '上次发送后在模型回复前中断了，这条提问还没有拿到回答。',
    retryable: true,
    retryInput: userMessage.text,
    retryConversationId: conversationId
  };
}

export function hydrateConversationMessages(rawMessages = [], conversationId) {
  const messages = rawMessages.map(createConversationMessage);
  const pendingUserMessage = findRetryableUserMessage(messages);
  const retryMessage = createInterruptedAssistantMessage(pendingUserMessage, conversationId);

  if (retryMessage) {
    messages.push(retryMessage);
  }

  return messages;
}
