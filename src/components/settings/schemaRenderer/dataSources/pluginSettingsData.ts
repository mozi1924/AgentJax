export interface PluginSettingsSnapshot {
  dataSources: Record<string, unknown>;
}

export const asPluginRecord = (value: unknown): Record<string, unknown> =>
  value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};

export const pluginItemIdentity = (item: unknown) => {
  const record = asPluginRecord(item);
  const value = record.id ?? record.key ?? record.name ?? '';
  return value === null || value === undefined ? '' : String(value);
};

export const pluginItemMatchesQuery = (item: unknown, query: string) => {
  const normalizedQuery = query.trim().toLowerCase();
  if (!normalizedQuery) return true;
  return (JSON.stringify(item) ?? '').toLowerCase().includes(normalizedQuery);
};

export const pluginSettingsSnapshotMatchesQuery = (
  snapshot: PluginSettingsSnapshot | null,
  query: string
) => {
  return pluginSettingsSnapshotMatchingDataSources(snapshot, query).length > 0;
};

export const pluginSettingsSnapshotMatchingDataSources = (
  snapshot: PluginSettingsSnapshot | null,
  query: string
) => {
  const normalizedQuery = query.trim().toLowerCase();
  if (!normalizedQuery) return [];
  return Object.entries(snapshot?.dataSources || {})
    .filter(([dataSource, value]) =>
      `${dataSource} ${JSON.stringify(value) ?? ''}`.toLowerCase().includes(normalizedQuery)
    )
    .map(([dataSource]) => dataSource);
};

export const selectedSourceForPluginDetail = (dataSource: string) => {
  if (dataSource.endsWith('.selectedItem')) {
    return dataSource.slice(0, -'.selectedItem'.length);
  }
  if (dataSource.endsWith('.selected')) {
    return dataSource.slice(0, -'.selected'.length);
  }
  return '';
};

// Resolves manifest-backed plugin arrays into renderer-ready list items. The
// activeKey convention lets generic list rows style selection without a
// plugin-specific React component.
export const resolvePluginSettingsList = ({
  snapshot,
  dataSource,
  search,
  selectedIds,
}: {
  snapshot: PluginSettingsSnapshot | null;
  dataSource: string;
  search: string;
  selectedIds: Record<string, string>;
}) => {
  const rawItems = snapshot?.dataSources[dataSource];
  if (!Array.isArray(rawItems)) return [];
  const filteredItems = rawItems.filter((item) => pluginItemMatchesQuery(item, search));
  const currentId = selectedIds[dataSource] || pluginItemIdentity(filteredItems[0]);
  const activeId = filteredItems.some((item) => pluginItemIdentity(item) === currentId)
    ? currentId
    : pluginItemIdentity(filteredItems[0]);
  return filteredItems.map((item) => ({
    ...asPluginRecord(item),
    activeKey: activeId,
  }));
};

export const resolvePluginSelectedItem = ({
  snapshot,
  dataSource,
  search,
  selectedIds,
}: {
  snapshot: PluginSettingsSnapshot | null;
  dataSource: string;
  search: string;
  selectedIds: Record<string, string>;
}) => {
  const listSource = selectedSourceForPluginDetail(dataSource);
  if (!listSource) return undefined;
  const items = resolvePluginSettingsList({
    snapshot,
    dataSource: listSource,
    search,
    selectedIds,
  });
  const currentId = selectedIds[listSource] || pluginItemIdentity(items[0]);
  return items.find((item) => pluginItemIdentity(item) === currentId) || items[0] || {};
};
