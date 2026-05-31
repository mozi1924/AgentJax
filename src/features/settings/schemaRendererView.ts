import type {
  SettingsOption,
  SettingsSchemaNode,
  SettingsUiAction,
  SettingsUiProperty,
} from './types';

export type SchemaRenderKind = 'field' | 'group' | 'collection' | 'ui';

interface SchemaSearchFilterOptions {
  preserveDataSourceNodes?: boolean;
}

export const DATA_CONTEXT_UI_NODE_KINDS = new Set([
  'layout',
  'panel',
  'tabs',
  'split',
  'toolbar',
  'list',
  'detail',
]);

const DATA_CONTEXT_BOUND_UI_NODE_KINDS = new Set([
  'badge',
  'metric',
  'empty_state',
  'action',
]);

// Pure dispatch helper used by the renderer and tests to keep v1 compatibility explicit.
export const getSchemaRenderKind = (node: SettingsSchemaNode): SchemaRenderKind => {
  if (node.kind === 'field') return 'field';
  if (node.kind === 'group') return 'group';
  if (node.kind === 'collection') return 'collection';
  return 'ui';
};

export const shouldUseDataContextRenderer = (node: SettingsSchemaNode) =>
  getSchemaRenderKind(node) === 'ui' &&
  (DATA_CONTEXT_UI_NODE_KINDS.has(node.kind) ||
    (DATA_CONTEXT_BOUND_UI_NODE_KINDS.has(node.kind) &&
      Boolean(node.dataSource || node.actions?.some((action) => action.dataSource))));

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

const propertySearchText = (
  property: SettingsUiProperty,
  translate: (key: string) => string
) =>
  [
    property.id,
    property.value,
    property.variant,
    translatedText(property.label, translate),
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
    node.properties?.map((property) => propertySearchText(property, translate)).join(' '),
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
  translate: (key: string) => string,
  options: Required<SchemaSearchFilterOptions>
): SettingsSchemaNode | null => {
  const ownMatch = normalizeSearchText(nodeSearchText(node, translate)).includes(normalizedQuery);

  const filteredChildren =
    'children' in node && Array.isArray(node.children)
      ? filterSchemaNodesForSearch(node.children, normalizedQuery, translate, options)
      : undefined;

  const filteredTabs =
    'tabs' in node && Array.isArray(node.tabs)
      ? node.tabs
          .map((tab) => {
            const tabMatch = normalizeSearchText(
              `${tab.id} ${tab.icon || ''} ${translatedText(tab.title, translate)}`
            ).includes(normalizedQuery);
            const children = filterSchemaNodesForSearch(tab.children, normalizedQuery, translate, options);
            return tabMatch || children.length > 0 ? { ...tab, children: tabMatch ? tab.children : children } : null;
          })
          .filter((tab): tab is NonNullable<typeof tab> => Boolean(tab))
      : undefined;

  const filteredItemTemplate = node.itemTemplate
    ? filterNodeForSearch(node.itemTemplate, normalizedQuery, translate, options)
    : undefined;

  if (ownMatch || (options.preserveDataSourceNodes && node.dataSource)) {
    return node;
  }

  if (filteredChildren?.length) {
    return cloneWithChildren(node, filteredChildren);
  }

  if (filteredTabs?.length && 'tabs' in node) {
    return { ...node, tabs: filteredTabs } as SettingsSchemaNode;
  }

  if (filteredItemTemplate && node.itemTemplate) {
    return { ...node, itemTemplate: filteredItemTemplate } as SettingsSchemaNode;
  }

  return null;
};

// Filters the schema tree without mutating the registry. Render-time filtering keeps
// data-source surfaces visible so providers can filter rows; section-list filtering
// can disable that preservation to avoid false positive section matches.
export const filterSchemaNodesForSearch = (
  nodes: SettingsSchemaNode[],
  query: string,
  translate: (key: string) => string = (key) => key,
  options: SchemaSearchFilterOptions = {}
) => {
  const normalizedQuery = normalizeSearchText(query);
  if (!normalizedQuery) return nodes;
  const resolvedOptions = {
    preserveDataSourceNodes: options.preserveDataSourceNodes ?? true,
  };
  return nodes
    .map((node) => filterNodeForSearch(node, normalizedQuery, translate, resolvedOptions))
    .filter((node): node is SettingsSchemaNode => Boolean(node));
};

export const schemaUsesDataSource = (
  nodes: SettingsSchemaNode[],
  dataSourcePrefix: string
): boolean =>
  collectSchemaDataSources(nodes).some(
    (dataSource) => dataSource === dataSourcePrefix || dataSource.startsWith(`${dataSourcePrefix}.`)
  );

const collectNodeDataSourceNamespaces = (
  node: SettingsSchemaNode,
  dataSources: Set<string>
) => {
  if (node.dataSource) {
    dataSources.add(node.dataSource);
  }
  if ('children' in node && node.children) {
    node.children.forEach((child) => collectNodeDataSourceNamespaces(child, dataSources));
  }
  if ('tabs' in node && node.tabs) {
    node.tabs.forEach((tab) =>
      tab.children.forEach((child) => collectNodeDataSourceNamespaces(child, dataSources))
    );
  }
  if (node.itemTemplate) {
    collectNodeDataSourceNamespaces(node.itemTemplate, dataSources);
  }
  node.actions?.forEach((action) => {
    if (action.dataSource) {
      dataSources.add(action.dataSource);
    }
  });
};

export const collectSchemaDataSources = (nodes: SettingsSchemaNode[]) => {
  const dataSources = new Set<string>();
  nodes.forEach((node) => collectNodeDataSourceNamespaces(node, dataSources));
  return [...dataSources].sort();
};

export const collectSchemaDataSourceNamespaces = (nodes: SettingsSchemaNode[]) =>
  [...new Set(collectSchemaDataSources(nodes).map((dataSource) => dataSource.split('.')[0]))].sort();
