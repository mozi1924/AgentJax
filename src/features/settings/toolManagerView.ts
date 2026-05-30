export type ToolSourceType = 'native' | 'mcp' | 'plugin' | 'dynamic' | 'background' | 'control';
export type ToolCategory = 'native' | 'mcp' | 'plugin' | 'session' | 'background';

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
}

export interface ToolManagerSourceSnapshot {
  sourceType: ToolSourceType;
  sourceId: string;
  sourceName: string;
  enabled: boolean;
  status: string;
  exposureMode: string;
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

// Keep source-to-tab mapping centralized so the read-only view and tests agree
// on where control/dynamic tools belong without duplicating UI conditionals.
export const categoryForSource = (sourceType: ToolSourceType): ToolCategory => {
  if (sourceType === 'dynamic') return 'session';
  if (sourceType === 'control') return 'mcp';
  return sourceType;
};

export const sourcesForCategory = (
  sources: ToolManagerSourceSnapshot[],
  category: ToolCategory
) => sources.filter((source) => categoryForSource(source.sourceType) === category);

export const selectToolManagerSource = (
  sources: ToolManagerSourceSnapshot[],
  selectedSourceId: string
) => sources.find((source) => source.sourceId === selectedSourceId) || sources[0] || null;

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
