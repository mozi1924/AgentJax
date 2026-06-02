// Frontend types for sub-agent state and events.

export interface SubAgentState {
  agentId: string;
  parentConversationId: string;
  subagentType: string;
  prompt: string;
  status: 'pending' | 'running' | 'completed' | 'failed' | 'cancelled';
  startedAtUnixMs: number;
  completedAtUnixMs?: number;
  durationMs?: number;
  turnsCompleted: number;
  maxTurns: number;
  error?: string;
}

export interface SubAgentEventPayload {
  agentId: string;
  subagentType?: string;
  parentRequestId?: string;
  text?: string;
  turnsCompleted?: number;
  turnsRemaining?: number;
  callId?: string;
  toolName?: string;
  toolStatus?: string;
  hopIndex?: number;
  result?: unknown;
  error?: string;
  durationMs?: number;
  reason?: string;
}
