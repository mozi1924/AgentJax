export type ConversationTitleSource = 'pending' | 'manual' | 'stored';

export type ToolCallStatus =
  | 'started'
  | 'arguments_done'
  | 'executed'
  | 'failed';

export interface ToolCall {
  id: string;
  name: string;
  arguments: string;
  output: string;
  status: ToolCallStatus;
  durationMs?: number | null;
}

export type MessageRole = 'user' | 'assistant' | 'system';
export type MessageStatus = 'streaming' | 'done' | 'failed' | 'interrupted';

export interface ConversationMessage {
  id: string;
  role: MessageRole;
  text: string;
  status?: MessageStatus;
  errorText?: string;
  retryable?: boolean;
  retryInput?: string;
  retryConversationId?: string | null;
  createdAtUnixMs?: number;
  responseId?: string | null;
  timelineEvents?: unknown[] | null;
  toolCalls?: ToolCall[];
}

export interface Conversation {
  conversationId: string;
  title: string;
  titleSource: ConversationTitleSource;
  messages: ConversationMessage[];
  lastResponseId: string | null;
  lastMessagePreview: string;
  messageCount: number;
  isLoaded: boolean;
}

export interface ConversationSummary {
  conversationId: string;
  title?: string;
  titleSource?: ConversationTitleSource;
  lastMessagePreview?: string;
  messageCount?: number;
}

export interface ConversationDetail {
  conversationId: string;
  title?: string;
  titleSource?: ConversationTitleSource;
  messages: RawConversationMessage[];
  lastResponseId?: string | null;
}

export interface ModelOption {
  profileKey: string;
  providerKey: string;
  modelId: string;
  supportsReasoning: boolean;
  supportedReasoningLevels: string[];
  configuredReasoningEffort: string | null;
}

export interface ModelCatalogResponse {
  modelOptions?: Array<Partial<ModelOption> | null>;
  effectiveModels?: string[];
  defaultModel?: string;
  configPath?: string;
  cachePath?: string;
}

export interface ChatStreamResponse {
  outputText?: string;
  responseId?: string | null;
  conversationTitle?: string;
}

export interface ChatRequestOptions {
  text?: unknown;
  include?: string[];
  serviceTier?: string;
  promptCacheKey?: string;
  clientMetadata?: Record<string, unknown>;
  generate?: boolean;
}

export interface ChatStreamEventPayload {
  kind?: string;
  requestId?: string;
  conversationId?: string;
  conversationTitle?: string;
  eventIndex?: number;
  delta?: string;
  responseId?: string | null;
  toolCallId?: string;
  toolName?: string;
  toolArguments?: string;
  toolOutput?: string;
}

interface RawToolCallTimelineEvent {
  type: string;
  callId?: string;
  name?: string;
  arguments?: unknown;
  output?: unknown;
  status?: string;
  durationMs?: number | null;
}

interface RawFunctionCallContextItem {
  type: string;
  call_id?: string;
  callId?: string;
  name?: string;
  arguments?: string;
  output?: string;
}

export interface RawConversationMessage {
  id: string;
  role: MessageRole;
  text?: string;
  createdAtUnixMs?: number;
  responseId?: string | null;
  timelineEvents?: RawToolCallTimelineEvent[];
  contextItems?: RawFunctionCallContextItem[];
}
