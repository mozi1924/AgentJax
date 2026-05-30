import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { ToolManagerSnapshot } from '../../../features/settings/toolManagerView';

// Owns Tool Manager snapshot IO. Plain snapshots stay read-only and do not trigger MCP discovery.
export function useToolManagerSnapshot() {
  const [snapshot, setSnapshot] = useState<ToolManagerSnapshot | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState('');
  const [actionError, setActionError] = useState('');
  const [discoveringSourceId, setDiscoveringSourceId] = useState('');
  const [discoveredSourceIds, setDiscoveredSourceIds] = useState<Set<string>>(new Set());

  const refreshSnapshot = async (options?: { discoverSourceId?: string }) => {
    const request = options?.discoverSourceId
      ? { sourceId: options.discoverSourceId, discover: true }
      : null;
    const nextSnapshot = await invoke<ToolManagerSnapshot>('get_tool_manager_snapshot', {
      request,
    });
    setSnapshot(nextSnapshot);
    if (options?.discoverSourceId) {
      setDiscoveredSourceIds((current) => new Set(current).add(options.discoverSourceId || ''));
    }
    return nextSnapshot;
  };

  const discoverSource = async (sourceId: string) => {
    setDiscoveringSourceId(sourceId);
    setActionError('');
    try {
      await refreshSnapshot({ discoverSourceId: sourceId });
    } catch (err) {
      setActionError(typeof err === 'string' ? err : String(err));
    } finally {
      setDiscoveringSourceId('');
    }
  };

  useEffect(() => {
    let disposed = false;
    setLoading(true);
    setLoadError('');
    invoke<ToolManagerSnapshot>('get_tool_manager_snapshot', { request: null })
      .then((nextSnapshot) => {
        if (!disposed) setSnapshot(nextSnapshot);
      })
      .catch((err) => {
        if (!disposed) setLoadError(typeof err === 'string' ? err : String(err));
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, []);

  return {
    snapshot,
    loading,
    loadError,
    actionError,
    setActionError,
    discoveringSourceId,
    discoveredSourceIds,
    refreshSnapshot,
    discoverSource,
  };
}
