import type { SettingsOption, SettingsSchemaNode, SettingsUiAction } from './types';

export type SchemaRenderKind = 'field' | 'group' | 'collection' | 'ui';

export const DATA_CONTEXT_UI_NODE_KINDS = new Set([
  'layout',
  'panel',
  'tabs',
  'split',
  'toolbar',
  'list',
  'detail',
]);

// Pure dispatch helper used by the renderer and tests to keep v1 compatibility explicit.
export const getSchemaRenderKind = (node: SettingsSchemaNode): SchemaRenderKind => {
  if (node.kind === 'field') return 'field';
  if (node.kind === 'group') return 'group';
  if (node.kind === 'collection') return 'collection';
  return 'ui';
};

export const shouldUseDataContextRenderer = (node: SettingsSchemaNode) =>
  getSchemaRenderKind(node) === 'ui' && DATA_CONTEXT_UI_NODE_KINDS.has(node.kind);

const normalizeSearchText = (value: unknown) =>
  `${value ?? ''}`.trim().toLocaleLowerCase();

const translatedText = (value: unknown, translate: (key: string) => string) => {
  if (typeof value !== 'string') return '';
  return `${value} ${translate(value)}`;
};

const actionSearchText = (action: SettingsUiAction, translate: (key: string) => string) =>
  [
    action.id,
    action.action,
    action.icon,
    action.variant,
    translatedText(action.label, translate),
    action.options
      ?.map((option: SettingsOption) => `${option.value} ${translatedText(option.label, translate)}`)
      .join(' '),
  ].join(' ');

const nodeSearchText = (node: SettingsSchemaNode, translate: (key: string) => string) => {
  const commonText = [
    node.id,
    node.kind,
    node.layout,
    node.variant,
    node.icon,
    node.badge,
    node.dataSource,
    node.emptyText,
    translatedText(node.title, translate),
    translatedText(node.description, translate),
    translatedText(node.helpText, translate),
    translatedText(node.warningText, translate),
    node.actions?.map((action) => actionSearchText(action, translate)).join(' '),
  ];

  if (node.kind === 'field') {
    commonText.push(
      node.path,
      node.control,
      node.valueType,
      node.optionSourceKey,
      translatedText(node.placeholder, translate),
      node.options
        ?.map((option) => `${option.value} ${translatedText(option.label, translate)}`)
        .join(' ')
    );
  }

  if (node.kind === 'collection') {
    commonText.push(
      node.path,
      node.valueType,
      translatedText(node.addLabel, translate),
      translatedText(node.keyLabel, translate),
      translatedText(node.itemLabel, translate)
    );
  }

  if ('tabs' in node && node.tabs) {
    commonText.push(
      node.tabs.map((tab) => `${tab.id} ${tab.icon || ''} ${translatedText(tab.title, translate)}`).join(' ')
    );
  }

  return commonText.join(' ');
};

const cloneWithChildren = (
  node: SettingsSchemaNode,
  children?: SettingsSchemaNode[]
): SettingsSchemaNode => ({ ...node, children } as SettingsSchemaNode);

const filterNodeForSearch = (
  node: SettingsSchemaNode,
  normalizedQuery: string,
  translate: (key: string) => string
): SettingsSchemaNode | null => {
  const ownMatch = normalizeSearchText(nodeSearchText(node, translate)).includes(normalizedQuery);

  const filteredChildren =
    'children' in node && Array.isArray(node.children)
      ? filterSchemaNodesForSearch(node.children, normalizedQuery, translate)
      : undefined;

  const filteredTabs =
    'tabs' in node && Array.isArray(node.tabs)
      ? node.tabs
          .map((tab) => {
            const tabMatch = normalizeSearchText(
              `${tab.id} ${tab.icon || ''} ${translatedText(tab.title, translate)}`
            ).includes(normalizedQuery);
            const children = filterSchemaNodesForSearch(tab.children, normalizedQuery, translate);
            return tabMatch || children.length > 0 ? { ...tab, children: tabMatch ? tab.children : children } : null;
          })
          .filter((tab): tab is NonNullable<typeof tab> => Boolean(tab))
      : undefined;

  if (ownMatch || node.dataSource) {
    return node;
  }

  if (filteredChildren?.length) {
    return cloneWithChildren(node, filteredChildren);
  }

  if (filteredTabs?.length && 'tabs' in node) {
    return { ...node, tabs: filteredTabs } as SettingsSchemaNode;
  }

  return null;
};

// Filters the schema tree without mutating the registry; data-source nodes stay visible
// because their searchable rows are supplied at render time rather than encoded in JSON.
export const filterSchemaNodesForSearch = (
  nodes: SettingsSchemaNode[],
  query: string,
  translate: (key: string) => string = (key) => key
) => {
  const normalizedQuery = normalizeSearchText(query);
  if (!normalizedQuery) return nodes;
  return nodes
    .map((node) => filterNodeForSearch(node, normalizedQuery, translate))
    .filter((node): node is SettingsSchemaNode => Boolean(node));
};

export const schemaUsesDataSource = (
  nodes: SettingsSchemaNode[],
  dataSourcePrefix: string
): boolean =>
  nodes.some((node) => {
    if (node.dataSource?.startsWith(dataSourcePrefix)) return true;
    if ('children' in node && node.children && schemaUsesDataSource(node.children, dataSourcePrefix)) {
      return true;
    }
    if ('tabs' in node && node.tabs) {
      return node.tabs.some((tab) => schemaUsesDataSource(tab.children, dataSourcePrefix));
    }
    if (node.itemTemplate && schemaUsesDataSource([node.itemTemplate], dataSourcePrefix)) {
      return true;
    }
    return false;
  });
