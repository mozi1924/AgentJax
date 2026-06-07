import type { SettingsSchemaNode, SettingsUiSchemaNode } from '../../../features/settings/types';
import type { SchemaRendererDataContext } from './types';

/**
 * Strategy result for a split-pane layout node.
 * Allows overriding layout type and children based on runtime state.
 */
export interface SplitLayoutOverride {
  layout?: string;
  children?: SettingsSchemaNode[];
}

/**
 * Strategy function type: given a UI node and data context, returns
 * an optional layout/children override for a split node.
 */
type SplitLayoutStrategy = (
  node: SettingsUiSchemaNode,
  dataContext: SchemaRendererDataContext
) => SplitLayoutOverride | null;

/**
 * Registered split-layout strategies keyed by node id.
 * Over time, these can be moved into the JSON schema itself
 * (e.g., via a "layoutStrategy" property) rather than being hardcoded here.
 */
const SPLIT_LAYOUT_STRATEGIES: Record<string, SplitLayoutStrategy> = {
  /**
   * Tool manager: when the active tab is "native", "knowledge_base",
   * "session", or "context", the three-pane layout collapses to
   * two-pane (hiding the source list).
   */
  'tool-manager-layout': (node, dataContext): SplitLayoutOverride | null => {
    const queryState = asRecord(dataContext.getDataSource('toolManager.query'));
    const activeTab = queryState.activeTab;
    if (activeTab === 'native' || activeTab === 'knowledge_base' || activeTab === 'session' || activeTab === 'context') {
      return {
        layout: 'two-pane',
        children: node.children?.filter((child) => child.id !== 'tool-source-list'),
      };
    }
    return null;
  },
};

function asRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

/**
 * Apply split-layout strategies for the given node.
 * Returns an override object when a strategy matches, or null if no override is needed.
 */
export function applySplitLayoutStrategy(
  node: SettingsUiSchemaNode,
  dataContext: SchemaRendererDataContext
): SplitLayoutOverride | null {
  const strategy = SPLIT_LAYOUT_STRATEGIES[node.id];
  if (!strategy) return null;
  return strategy(node, dataContext);
}
