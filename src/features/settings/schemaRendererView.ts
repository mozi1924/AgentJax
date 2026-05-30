import type { SettingsSchemaNode } from './types';

export type SchemaRenderKind = 'field' | 'group' | 'collection' | 'ui';

// Pure dispatch helper used by the renderer and tests to keep v1 compatibility explicit.
export const getSchemaRenderKind = (node: SettingsSchemaNode): SchemaRenderKind => {
  if (node.kind === 'field') return 'field';
  if (node.kind === 'group') return 'group';
  if (node.kind === 'collection') return 'collection';
  return 'ui';
};
