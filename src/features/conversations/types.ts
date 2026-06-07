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
  /** Reasoning / thinking content streamed before the final response.
   *  Rendered as a collapsible "Thinking..." section in the UI. */
  thinking?: string;
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
  /** Last error that interrupted the agent turn, if any. */
  lastError?: string | null;
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

// ── Model / stream types ──────────────────────────────────────────────────

export interface ModelOption {
  profileKey: string;
  providerKey: string;
  modelId: string;
  /** Optional user-facing friendly name. When absent, show modelId. */
  name?: string;
  supportsReasoning: boolean;
  supportedReasoningLevels: string[];
  configuredReasoningEffort: string | null;
  /** Model kind from provider plugin: "chat", "embedding", etc. */
  kind?: string;
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
  toolStatus?: 'pending' | 'done' | 'failed';
  toolStartedTs?: number;
  toolCompletedTs?: number;
  toolDurationMs?: number;
  contextTokenCount?: number;
  /** Phase hint for assistant text events. */
  phase?: AssistantPhase | null;
  /** Sub-agent identifier — present for sub-agent lifecycle events. */
  agentId?: string;
  /** Error message for error events from the backend. */
  error?: string | null;
}

