import type {
  AssistantLine,
  ConversationLine,
  ToolLine,
  UserLine,
} from '../../features/conversations/types';

export type TurnWorkItem =
  | {
      kind: 'assistant';
      line: AssistantLine;
    }
  | {
      kind: 'tool';
      line: ToolLine;
    };

export interface ConversationTurn {
  requestId: string;
  userLines: UserLine[];
  workItems: TurnWorkItem[];
  finalLines: AssistantLine[];
  startedAt: number;
  endedAt: number;
  hasDraft: boolean;
}

export interface TurnActivitySummary {
  kind: 'search' | 'edit' | 'tool' | 'update';
  count: number;
  label: string;
}

const SEARCH_TOOL_PATTERNS = [
  /search/i,
  /\bfind\b/i,
  /\bglob\b/i,
  /\brg\b/i,
  /\bgrep\b/i,
];

const EDIT_TOOL_PATTERNS = [
  /apply_patch/i,
  /edit/i,
  /write/i,
  /push_files/i,
  /create_or_update_file/i,
  /delete_file/i,
];

const pluralize = (count: number, singular: string, plural = `${singular}s`) =>
  (count === 1 ? singular : plural);

const isSearchTool = (toolName: string) =>
  SEARCH_TOOL_PATTERNS.some((pattern) => pattern.test(toolName));

const isEditTool = (toolName: string) =>
  EDIT_TOOL_PATTERNS.some((pattern) => pattern.test(toolName));

const createEmptyTurn = (requestId: string, ts: number): ConversationTurn => ({
  requestId,
  userLines: [],
  workItems: [],
  finalLines: [],
  startedAt: ts,
  endedAt: ts,
  hasDraft: false,
});

/**
 * Groups the flat transcript into request-scoped turns so the UI can render
 * Codex-like "work log + final answer" sections instead of isolated cards.
 */
export function buildConversationTurns(
  lines: ConversationLine[] = []
): ConversationTurn[] {
  const turns = new Map<string, ConversationTurn>();
  const orderedRequestIds: string[] = [];

  for (const line of lines) {
    const requestId = line.requestId || line.id;
    let turn = turns.get(requestId);

    if (!turn) {
      turn = createEmptyTurn(requestId, line.ts);
      turns.set(requestId, turn);
      orderedRequestIds.push(requestId);
    }

    turn.startedAt = Math.min(turn.startedAt, line.ts);
    turn.endedAt = Math.max(turn.endedAt, line.ts);

    if (line.kind === 'user') {
      turn.userLines.push(line);
      continue;
    }

    if (line.kind === 'tool') {
      turn.workItems.push({ kind: 'tool', line });
      continue;
    }

    if (line.status === 'draft') {
      turn.hasDraft = true;
    }

    if (line.phase === 'commentary') {
      turn.workItems.push({ kind: 'assistant', line });
      continue;
    }

    turn.finalLines.push(line);
  }

  return orderedRequestIds
    .map((requestId) => turns.get(requestId))
    .filter((turn): turn is ConversationTurn => Boolean(turn));
}

export function getTurnDurationMs(turn: ConversationTurn): number {
  return Math.max(0, turn.endedAt - turn.startedAt);
}

export function formatTurnDuration(durationMs: number): string {
  const totalSeconds = Math.max(1, Math.round(durationMs / 1000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;

  if (minutes <= 0) {
    return `${seconds}s`;
  }

  if (seconds === 0) {
    return `${minutes}m`;
  }

  return `${minutes}m ${seconds}s`;
}

export function hasTurnWorkLog(turn: ConversationTurn): boolean {
  return turn.workItems.length > 0;
}

export function shouldCollapseTurnWorkLog(turn: ConversationTurn): boolean {
  return hasTurnWorkLog(turn) && turn.finalLines.length > 0 && !turn.hasDraft;
}

export function getTurnActivitySummary(
  turn: ConversationTurn
): TurnActivitySummary[] {
  let searchCount = 0;
  let editCount = 0;
  let toolCount = 0;
  let updateCount = 0;

  for (const item of turn.workItems) {
    if (item.kind === 'assistant') {
      if ((item.line.text || '').trim()) {
        updateCount += 1;
      }
      continue;
    }

    toolCount += 1;
    const toolName = item.line.name || '';
    if (isSearchTool(toolName)) {
      searchCount += 1;
      continue;
    }
    if (isEditTool(toolName)) {
      editCount += 1;
    }
  }

  const genericToolCount = Math.max(0, toolCount - searchCount - editCount);
  const summaries: TurnActivitySummary[] = [];

  if (searchCount > 0) {
    summaries.push({
      kind: 'search',
      count: searchCount,
      label: `Explored ${searchCount} ${pluralize(searchCount, 'search', 'searches')}`,
    });
  }

  if (editCount > 0) {
    summaries.push({
      kind: 'edit',
      count: editCount,
      label: `Edited ${editCount} ${pluralize(editCount, 'change')}`,
    });
  }

  if (genericToolCount > 0) {
    summaries.push({
      kind: 'tool',
      count: genericToolCount,
      label: `Used ${genericToolCount} ${pluralize(genericToolCount, 'tool')}`,
    });
  }

  if (updateCount > 0) {
    summaries.push({
      kind: 'update',
      count: updateCount,
      label: `Shared ${updateCount} ${pluralize(updateCount, 'update')}`,
    });
  }

  return summaries;
}

export function joinAssistantTexts(lines: AssistantLine[]): string {
  return lines
    .map((line) => (line.text || '').trim())
    .filter(Boolean)
    .join('\n\n');
}
