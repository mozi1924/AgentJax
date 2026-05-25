import { createLocalConversation, isConversationEmpty } from './conversationUtils';
import type {
  Conversation,
  ConversationMessage,
  ConversationSummary,
} from './types';

export const countVisibleMessages = (messages: ConversationMessage[]): number =>
  messages.filter((message) => message.role === 'user' || Boolean(message.text)).length;

export const restoreConversationPreview = (
  conversation: ConversationSummary
): Conversation => ({
  conversationId: conversation.conversationId,
  title: conversation.title || '',
  titleSource: conversation.titleSource || 'stored',
  messages: [],
  lastResponseId: null,
  lastMessagePreview: conversation.lastMessagePreview || '',
  messageCount: conversation.messageCount || 0,
  isLoaded: false,
});

export const mergeWithLocalDrafts = (
  currentConversations: Conversation[],
  storedConversations: ConversationSummary[]
): Conversation[] => {
  const localDrafts = currentConversations.filter((conversation) =>
    isConversationEmpty(conversation)
  );
  const restoredConversations = storedConversations.map(restoreConversationPreview);
  return [...localDrafts, ...restoredConversations];
};

export const ensureAtLeastOneConversation = (
  conversations: Conversation[]
): Conversation[] => {
  if (conversations.length > 0) {
    return conversations;
  }
  return [createLocalConversation()];
};
