import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  toolManagerSnapshotMatchesQuery,
  type ToolManagerSnapshot,
} from './toolManagerData';
import {
  pluginSettingsSnapshotMatchingDataSources,
  type PluginSettingsSnapshot,
} from './pluginSettingsData';

type SearchMatchState = Record<string, boolean>;

// Gives the settings modal enough dynamic search knowledge to keep provider
// sections discoverable from the global search box without embedding provider UI.
export function useDynamicSettingsSearchIndex({
  search,
  namespaces,
}: {
  search: string;
  namespaces: string[];
}): ReadonlySet<string> {
  const [matches, setMatches] = useState<SearchMatchState>({});
  const namespacesKey = useMemo(() => [...namespaces].sort().join('\n'), [namespaces]);

  useEffect(() => {
    const query = search.trim();
    const requestedNamespaces = new Set(namespaces);
    if (!query || requestedNamespaces.size === 0) {
      setMatches({});
      return undefined;
    }

    let disposed = false;
    const tasks: Array<Promise<string[]>> = [];

    if (requestedNamespaces.has('toolManager')) {
      tasks.push(
        invoke<ToolManagerSnapshot>('get_tool_manager_snapshot', { request: null })
          .then(
            (snapshot): string[] =>
              toolManagerSnapshotMatchesQuery(snapshot, query) ? ['toolManager'] : []
          )
          .catch((): string[] => [])
      );
    }

    if (requestedNamespaces.has('plugin')) {
      tasks.push(
        invoke<PluginSettingsSnapshot>('get_plugin_settings_snapshot')
          .then(
            (snapshot): string[] =>
              pluginSettingsSnapshotMatchingDataSources(snapshot, query)
          )
          .catch((): string[] => [])
      );
    }

    if (tasks.length === 0) {
      setMatches({});
      return undefined;
    }

    void Promise.all(tasks).then((entries) => {
      if (disposed) return;
      setMatches(
        Object.fromEntries(entries.flat().map((dataSource) => [dataSource, true])) as SearchMatchState
      );
    });

    return () => {
      disposed = true;
    };
  }, [namespacesKey, search]);

  return useMemo(
    () => new Set(Object.entries(matches).filter(([, matched]) => matched).map(([key]) => key)),
    [matches]
  );
}
