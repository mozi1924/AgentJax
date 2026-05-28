import { countUserAndDoneAssistant, createLocalConversation, isConversationEmpty } from './conversationUtils';
import type {
  Conversation,
  ConversationLine,
  ConversationSummary,
} from './types';

export const restoreConversationPreview = (
  conversation: ConversationSummary
): Conversation => ({
  conversationId: conversation.conversationId,
  title: conversation.title || '',
  titleSource: conversation.titleSource || 'stored',
  lines: [],
  lastMessagePreview: conversation.lastMessagePreview || '',
  messageCount: conversation.messageCount || 0,
  contextTokenCount: 0,
  isLoaded: false,
});

export const mergeWithLocalDrafts = (
  currentConversations: Conversation[],
  storedConversations: ConversationSummary[]
): Conversation[] => {
  const localDrafts = currentConversations.filter((c) => isConversationEmpty(c));
  const restored = storedConversations.map(restoreConversationPreview);
  return [...localDrafts, ...restored];
};

export const ensureAtLeastOneConversation = (
  conversations: Conversation[]
): Conversation[] => {
  if (conversations.length > 0) return conversations;
  return [createLocalConversation()];
};

export const countVisibleMessages = (lines: ConversationLine[]): number =>
  countUserAndDoneAssistant(lines);
