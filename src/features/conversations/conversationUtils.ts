import type {
  AssistantLine,
  Conversation,
  ConversationLine,
} from './types';

export const DEFAULT_CONVERSATION_TITLE = '新对话';

export function createConversationId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  return `conv-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

export function createLocalConversation(
  conversationId = createConversationId()
): Conversation {
  return {
    conversationId,
    title: DEFAULT_CONVERSATION_TITLE,
    titleSource: 'pending',
    lines: [],
    lastMessagePreview: '',
    messageCount: 0,
    isLoaded: true,
  };
}

export function isConversationEmpty(conversation: Conversation | null | undefined): boolean {
  return Array.isArray(conversation?.lines) && conversation.lines.length === 0;
}

export function shouldShowConversationInSidebar(
  conversation: Conversation | null | undefined
): boolean {
  if (!conversation) return false;
  if (Array.isArray(conversation.lines) && conversation.lines.length > 0) return true;
  return (
    Number(conversation.messageCount || 0) > 0 ||
    Boolean((conversation.lastMessagePreview || '').trim())
  );
}

export function canUseNativeContextMenu(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.closest('input, textarea, [contenteditable="true"]')) return true;
  return Boolean(target.closest('[data-native-context-menu="true"]'));
}

export function applyConversationTitle(
  conversation: Conversation,
  nextTitle: string | null | undefined
): Conversation {
  const title = (nextTitle || '').trim();
  if (!title) return conversation;
  return { ...conversation, title };
}

function compactText(rawText: string | null | undefined, maxLength = 28): string {
  const cleaned = (rawText || '').split(/\s+/).filter(Boolean).join(' ');
  if (!cleaned) return '';
  if (cleaned.length <= maxLength) return cleaned;
  return `${cleaned.slice(0, Math.max(0, maxLength - 3))}...`;
}

export function buildDraftConversationTitle(rawText: string): string {
  return compactText(rawText, 30) || DEFAULT_CONVERSATION_TITLE;
}

function getFirstUserMessageText(conversation: Conversation | null | undefined): string {
  if (!Array.isArray(conversation?.lines)) return '';
  const firstUser = conversation.lines.find((l) => l.kind === 'user' && (l.text || '').trim());
  return firstUser && firstUser.kind === 'user' ? firstUser.text : '';
}

export function getConversationDisplayTitle(
  conversation: Conversation | null | undefined
): string {
  const explicitTitle = (conversation?.title || '').trim();
  if (
    explicitTitle &&
    (conversation?.titleSource === 'manual' || explicitTitle !== DEFAULT_CONVERSATION_TITLE)
  ) {
    return explicitTitle;
  }
  const fallback = getFirstUserMessageText(conversation) || conversation?.lastMessagePreview || '';
  return compactText(fallback, 30) || DEFAULT_CONVERSATION_TITLE;
}

export function countUserAndDoneAssistant(lines: ConversationLine[]): number {
  return lines.filter(
    (l) =>
      l.kind === 'user' ||
      (l.kind === 'assistant' && (l as AssistantLine).status === 'done')
  ).length;
}

export function hydrateConversationLines(
  rawLines: ConversationLine[] = []
): ConversationLine[] {
  return rawLines;
}

