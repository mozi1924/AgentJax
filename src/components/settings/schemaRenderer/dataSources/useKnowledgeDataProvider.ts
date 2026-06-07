import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { tryGetCurrentWindow } from '../../../../features/tauri/runtime';
import type { SchemaDataProvider, SchemaDataProviderArgs } from './types';

interface KnowledgeBaseStatus {
  id: string;
  name: string;
  path: string;
  documentCount: number;
  chunkCount: number;
  indexed: boolean;
}

/// Real-time progress event emitted by the backend during indexing.
interface KbIndexingProgress {
  kbId: string;
  phase: string;
  processed: number;
  total: number;
  currentFile: string;
  chunksCreated: number;
  done: boolean;
  error: string | null;
}

/// Per-KB indexing state tracked from progress events.
interface KbProgressState {
  kbId: string;
  phase: string;
  processed: number;
  total: number;
  currentFile: string;
  chunksCreated: number;
  done: boolean;
  error: string | null;
  startedAt: number;
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
 *   - `knowledgeManager.indexingProgress` — Map<string, KbProgressState> (per-KB progress)
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
  /// Per-KB indexing progress map: kbId → KbProgressState
  const [indexingProgress, setIndexingProgress] = useState<Map<string, KbProgressState>>(new Map());
  /// Track active KB refresh invocations so progress events update the right state
  const activeRefreshRef = useRef<Set<string>>(new Set());

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

  // ── Listen for real-time indexing progress events from the backend ──
  useEffect(() => {
    const currentWindow = tryGetCurrentWindow();
    if (!currentWindow) return;

    let unlisten: (() => void) | null = null;
    void currentWindow
      .listen<KbIndexingProgress>('kb_indexing_progress', (event) => {
        const p = event.payload;
        setIndexingProgress((prev) => {
          const next = new Map(prev);
          next.set(p.kbId, {
            kbId: p.kbId,
            phase: p.phase,
            processed: p.processed,
            total: p.total,
            currentFile: p.currentFile,
            chunksCreated: p.chunksCreated,
            done: p.done,
            error: p.error,
            startedAt: prev.get(p.kbId)?.startedAt ?? Date.now(),
          });
          return next;
        });
        // When indexing completes, refresh the KB status list.
        if (p.done && activeRefreshRef.current.has(p.kbId)) {
          activeRefreshRef.current.delete(p.kbId);
          void loadKbStatuses();
        }
      })
      .then((dispose) => {
        unlisten = dispose;
      });

    return () => {
      unlisten?.();
    };
  }, [loadKbStatuses]);

  return {
    namespace: 'knowledgeManager',
    enabled,
    getDataSource: (dataSource) => {
      if (dataSource === 'knowledgeManager.kbs') {
        return kbs;
      }
      if (dataSource === 'knowledgeManager.indexingProgress') {
        return indexingProgress;
      }
      // Per-KB progress: knowledgeManager.kbProgress.<kbId>
      if (dataSource?.startsWith('knowledgeManager.kbProgress.')) {
        const kbId = dataSource.slice('knowledgeManager.kbProgress.'.length);
        return indexingProgress.get(kbId) ?? null;
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
        // Mark as actively refreshing so the event listener knows to
        // auto-reload status when the progress stream signals done.
        activeRefreshRef.current.add(kbId);
        // Clear any stale progress for this KB.
        setIndexingProgress((prev) => {
          const next = new Map(prev);
          next.delete(kbId);
          return next;
        });
        try {
          await invoke('refresh_knowledge_base', { kbId });
        } catch (err) {
          console.error('Failed to refresh KB:', err);
          // Mark as error in progress state
          setIndexingProgress((prev) => {
            const next = new Map(prev);
            next.set(kbId, {
              kbId,
              phase: 'error',
              processed: 0,
              total: 0,
              currentFile: '',
              chunksCreated: 0,
              done: true,
              error: typeof err === 'string' ? err : String(err),
              startedAt: Date.now(),
            });
            return next;
          });
          activeRefreshRef.current.delete(kbId);
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
