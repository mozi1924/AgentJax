export type ConversationTitleSource = 'pending' | 'manual' | 'stored';
export type AssistantPhase = 'commentary' | 'final_answer';

// ── Conversation lines (matches backend tagged union) ─────────────────────

export type ConversationLine = UserLine | ToolLine | AssistantLine;

export interface UserLine {
  kind: 'user';
  id: string;
  ts: number;
  requestId: string;
  text: string;
}

export interface ToolLine {
  kind: 'tool';
  id: string;
  ts: number;
  startedTs?: number;
  completedTs?: number | null;
  requestId: string;
  callId: string;
  name: string;
  displayName?: string | null;
  description?: string | null;
  icon?: string | null;
  args: unknown;
  output?: unknown;
  status: 'pending' | 'done' | 'failed';
}

export interface AssistantLine {
  kind: 'assistant';
  id: string;
  ts: number;
  requestId: string;
  responseId: string;
  phase: AssistantPhase | null;
  text: string;
  status: 'draft' | 'done';
}

// ── Conversation model ────────────────────────────────────────────────────

export interface Conversation {
  conversationId: string;
  title: string;
  titleSource: ConversationTitleSource;
  lines: ConversationLine[];
  lastMessagePreview: string;
  messageCount: number;
  contextTokenCount: number;
  isLoaded: boolean;
}

// ── Backend DTOs ──────────────────────────────────────────────────────────

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
  lines: ConversationLine[];
  contextTokenCount?: number;
}

// ── Legacy (kept for compat during migration) ─────────────────────────────

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
  toolCalls?: ToolCall[];
}

export type ToolCallStatus = 'started' | 'arguments_done' | 'executed' | 'failed';

export interface ToolCall {
  id: string;
  name: string;
  arguments: string;
  output: string;
  status: ToolCallStatus;
  durationMs?: number | null;
}

// ── Model / stream types ──────────────────────────────────────────────────

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
  contextTokenCount?: number;
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
  toolDisplayName?: string;
  toolDescription?: string;
  toolIcon?: string;
  toolArguments?: string;
  toolOutput?: string;
  contextTokenCount?: number;
  /** Phase hint for assistant text events. */
  phase?: AssistantPhase | null;
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
