import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { MemoryIndexEntry } from '../../../../features/memory/types';
import type { SchemaDataProvider, SchemaDataProviderArgs } from './types';

const asRecord = (value: unknown): Record<string, unknown> =>
  value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};

/**
 * Data provider for the memory manager.
 *
 * Namespace: `memoryManager`
 *
 * Exposed data sources:
 *   - `memoryManager.memories` — MemoryIndexEntry[] (list)
 *   - `memoryManager.selectedMemory` — selected entry (detail)
 *   - `memoryManager.query` — { search, selectedItem }
 *
 * Actions:
 *   - `selectMemory` — select a memory entry
 *   - `openMemory` — open memory file in system editor
 *   - `deleteMemory` — delete a memory entry
 *   - `refreshMemories` — reload the memory list
 */
export function useMemoryDataProvider({
  requestedDataSourceNamespaces,
}: SchemaDataProviderArgs): SchemaDataProvider {
  const enabled = requestedDataSourceNamespaces.includes('memoryManager');
  const [memories, setMemories] = useState<MemoryIndexEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState('');
  const [actionError, setActionError] = useState('');
  const [selectedName, setSelectedName] = useState('');

  const loadMemories = async () => {
    setLoading(true);
    setLoadError('');
    try {
      const list = await invoke<MemoryIndexEntry[]>('list_memories');
      setMemories(list);
    } catch (err) {
      setLoadError(typeof err === 'string' ? err : String(err));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    if (!enabled) return;
    void loadMemories();
  }, [enabled]);

  const selectedMemory = useMemo(
    () => memories.find((m) => m.name === selectedName) || null,
    [memories, selectedName]
  );

  // Reset selection if the selected memory no longer exists (e.g., after delete).
  useEffect(() => {
    if (selectedName && !memories.some((m) => m.name === selectedName)) {
      setSelectedName(memories[0]?.name || '');
    }
  }, [memories, selectedName]);

  return {
    namespace: 'memoryManager',
    enabled,
    getDataSource: (dataSource) => {
      if (dataSource === 'memoryManager.memories') {
        return memories;
      }
      if (dataSource === 'memoryManager.selectedMemory') {
        return selectedMemory;
      }
      if (dataSource === 'memoryManager.query') {
        return {
          selectedItem: selectedName,
        };
      }
      return undefined;
    },
    getStatus: (dataSource) => {
      if (dataSource === 'memoryManager' || dataSource === 'memoryManager.memories') {
        return {
          loading,
          error: loadError,
          loadingText: 'settings.memory.loading',
          errorText: 'settings.memory.error',
        };
      }
      return undefined;
    },
    dispatch: async (action, payload) => {
      const record = asRecord(payload);
      const item = asRecord(record.item);

      if (action === 'selectMemory') {
        setSelectedName(String(record.value || item.name || ''));
        return;
      }

      if (action === 'openMemory') {
        const name = String(record.value || item.name || selectedName);
        if (!name) return;
        try {
          await invoke('open_memory_file', { name });
        } catch (err) {
          setActionError(typeof err === 'string' ? err : String(err));
        }
        return;
      }

      if (action === 'deleteMemory') {
        const name = String(record.value || item.name || '');
        if (!name) return;
        try {
          await invoke('delete_memory', { name });
          setMemories((prev) => prev.filter((m) => m.name !== name));
          setActionError('');
        } catch (err) {
          setActionError(typeof err === 'string' ? err : String(err));
        }
        return;
      }

      if (action === 'refreshMemories') {
        await loadMemories();
        return;
      }
    },
    isSaving: () => false,
  };
}
