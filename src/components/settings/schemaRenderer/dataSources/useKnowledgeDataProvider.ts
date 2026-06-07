import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { SchemaDataProvider, SchemaDataProviderArgs } from './types';

interface KnowledgeBaseStatus {
  id: string;
  name: string;
  path: string;
  documentCount: number;
  chunkCount: number;
  indexed: boolean;
}

const asRecord = (value: unknown): Record<string, unknown> =>
  value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};

/**
 * Data provider for the knowledge base manager.
 *
 * Namespace: `knowledgeManager`
 *
 * Exposed data sources:
 *   - `knowledgeManager.kbs` — KnowledgeBaseStatus[] (list with index status)
 *
 * Actions:
 *   - `refreshKb` — trigger re-indexing of a KB
 *   - `refreshAll` — reload the status list
 */
export function useKnowledgeDataProvider({
  requestedDataSourceNamespaces,
}: SchemaDataProviderArgs): SchemaDataProvider {
  const enabled = requestedDataSourceNamespaces.includes('knowledgeManager');
  const [kbs, setKbs] = useState<KnowledgeBaseStatus[]>([]);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState('');

  const loadKbStatuses = useCallback(async () => {
    setLoading(true);
    setLoadError('');
    try {
      const list = await invoke<KnowledgeBaseStatus[]>('list_knowledge_bases');
      setKbs(list);
    } catch (err) {
      setLoadError(typeof err === 'string' ? err : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!enabled) return;
    void loadKbStatuses();
  }, [enabled, loadKbStatuses]);

  return {
    namespace: 'knowledgeManager',
    enabled,
    getDataSource: (dataSource) => {
      if (dataSource === 'knowledgeManager.kbs') {
        return kbs;
      }
      return undefined;
    },
    getStatus: (dataSource) => {
      if (dataSource === 'knowledgeManager' || dataSource === 'knowledgeManager.kbs') {
        return {
          loading,
          error: loadError,
          loadingText: 'settings.knowledge.loading',
          errorText: 'settings.knowledge.error',
        };
      }
      return undefined;
    },
    dispatch: async (action, payload) => {
      const record = asRecord(payload);
      const item = asRecord(record.item);

      if (action === 'refreshKb') {
        const kbId = String(record.value || item.id || '');
        if (!kbId) return;
        try {
          await invoke('refresh_knowledge_base', { kbId });
          await loadKbStatuses();
        } catch (err) {
          console.error('Failed to refresh KB:', err);
        }
        return;
      }

      if (action === 'refreshAll') {
        await loadKbStatuses();
        return;
      }
    },
    isSaving: () => false,
  };
}
