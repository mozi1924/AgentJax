import { escapePathSegment } from '../../../../features/settings/utils';

export type ToolSourceType = 'native' | 'mcp' | 'plugin' | 'dynamic' | 'background' | 'control';
export type ToolCategory = 'native' | 'mcp' | 'plugin' | 'session' | 'background';
export type McpExposureMode = 'collapsed' | 'unfolded';

export interface ToolSchemaSummary {
  parameterCount: number;
  required: string[];
  properties: string[];
}

export interface ToolManagerToolSnapshot {
  id: string;
  friendlyName: string;
  modelName: string;
  description: string;
  icon?: string | null;
  enabled: boolean;
  availability: string;
  schemaSummary: ToolSchemaSummary;
  inputSchema?: unknown;
  schemaFormat?: 'json_schema' | 'openai_function' | 'mcp';
  sourceCapabilities?: string[];
  policyPaths?: {
    toolEnabledPath?: string | null;
  };
}

export interface ToolManagerSourceSnapshot {
  sourceType: ToolSourceType;
  sourceId: string;
  sourceName: string;
  enabled: boolean;
  status: string;
  exposureMode: string;
  sourceCapabilities?: string[];
  policyPaths?: {
    sourceEnabledPath?: string | null;
    exposurePath?: string | null;
  };
  tools: ToolManagerToolSnapshot[];
  error?: string | null;
}

export interface ToolManagerSnapshot {
  sources: ToolManagerSourceSnapshot[];
}

export const TOOL_MANAGER_CATEGORIES: Array<{ id: ToolCategory; labelKey: string }> = [
  { id: 'native', labelKey: 'settings.tools.category.native' },
  { id: 'mcp', labelKey: 'settings.tools.category.mcp' },
  { id: 'plugin', labelKey: 'settings.tools.category.plugin' },
  { id: 'session', labelKey: 'settings.tools.category.session' },
  { id: 'background', labelKey: 'settings.tools.category.background' },
];

// Keeps Tool Manager data shaping beside its SchemaRenderer provider instead of
// as a standalone settings view module. The UI remains fully described by schema.
export const categoryForSource = (sourceType: ToolSourceType): ToolCategory => {
  if (sourceType === 'dynamic') return 'session';
  if (sourceType === 'control') return 'mcp';
  return sourceType;
};

export const sourcesForCategory = (
  sources: ToolManagerSourceSnapshot[],
  category: ToolCategory
) => sources.filter((source) => categoryForSource(source.sourceType) === category);

export const sourceIdentityKey = (
  source: Pick<ToolManagerSourceSnapshot, 'sourceType' | 'sourceId'>
) => `${source.sourceType}:${source.sourceId}`;

export const selectToolManagerSource = (
  sources: ToolManagerSourceSnapshot[],
  selectedSourceKey: string
) =>
  sources.find((source) => sourceIdentityKey(source) === selectedSourceKey) ||
  sources.find((source) => source.sourceId === selectedSourceKey) ||
  sources[0] ||
  null;

export const matchesToolSearch = (tool: ToolManagerToolSnapshot, query: string) => {
  const normalized = query.trim().toLowerCase();
  if (!normalized) return true;
  return [tool.friendlyName, tool.id, tool.modelName, tool.description, tool.availability]
    .join(' ')
    .toLowerCase()
    .includes(normalized);
};

export const filterToolsForQuery = (tools: ToolManagerToolSnapshot[], query: string) =>
  tools.filter((tool) => matchesToolSearch(tool, query));

export const toolManagerSourceMatchesQuery = (
  source: ToolManagerSourceSnapshot,
  query: string
) => {
  const normalized = query.trim().toLowerCase();
  if (!normalized) return false;
  const sourceText = [
    source.sourceType,
    source.sourceId,
    source.sourceName,
    source.status,
    source.exposureMode,
    source.error,
  ]
    .join(' ')
    .toLowerCase();
  return sourceText.includes(normalized) || source.tools.some((tool) => matchesToolSearch(tool, query));
};

export const toolManagerSnapshotMatchesQuery = (
  snapshot: ToolManagerSnapshot | null,
  query: string
) => {
  const normalized = query.trim();
  if (!normalized) return false;
  return (snapshot?.sources || []).some((source) =>
    toolManagerSourceMatchesQuery(source, normalized)
  );
};

export const sourcePolicyEnabledPath = (source: ToolManagerSourceSnapshot) => {
  if (source.policyPaths?.sourceEnabledPath) {
    return source.policyPaths.sourceEnabledPath;
  }
  if (source.sourceType === 'plugin') {
    return `tool_manager.plugin_tools.${escapePathSegment(source.sourceId)}.enabled`;
  }
  if (source.sourceType === 'mcp') {
    return `tool_manager.mcp_tools.${escapePathSegment(source.sourceId)}.enabled`;
  }
  return null;
};

export const toolPolicyEnabledPath = (
  source: ToolManagerSourceSnapshot,
  tool: ToolManagerToolSnapshot
) => {
  if (tool.policyPaths?.toolEnabledPath) {
    return tool.policyPaths.toolEnabledPath;
  }
  if (source.sourceType === 'native') {
    return `tool_manager.native_tools.${escapePathSegment(tool.id)}.enabled`;
  }
  if (source.sourceType === 'plugin') {
    return `tool_manager.plugin_tools.${escapePathSegment(source.sourceId)}.tools.${escapePathSegment(
      tool.id
    )}.enabled`;
  }
  if (source.sourceType === 'mcp') {
    return `tool_manager.mcp_tools.${escapePathSegment(source.sourceId)}.tools.${escapePathSegment(
      tool.id
    )}.enabled`;
  }
  return null;
};

export const mcpExposurePolicyPath = (source: ToolManagerSourceSnapshot) =>
  source.policyPaths?.exposurePath ||
  (source.sourceType === 'mcp'
    ? `tool_manager.mcp_tools.${escapePathSegment(source.sourceId)}.exposure`
    : null);

export const isSourcePolicyEditable = (source: ToolManagerSourceSnapshot) =>
  source.sourceType === 'plugin' || source.sourceType === 'mcp';

export const isToolPolicyEditable = (source: ToolManagerSourceSnapshot) =>
  source.sourceType === 'native' || source.sourceType === 'plugin' || source.sourceType === 'mcp';

export const isMcpExposureEditable = (source: ToolManagerSourceSnapshot) =>
  source.sourceType === 'mcp';
