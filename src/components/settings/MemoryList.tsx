import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { MemoryIndexEntry } from '../../features/memory/types';

export function MemoryList() {
  const [memories, setMemories] = useState<MemoryIndexEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadMemories = () => {
    setLoading(true);
    invoke<MemoryIndexEntry[]>('list_memories')
      .then((list) => {
        setMemories(list);
        setError(null);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    loadMemories();
  }, []);

  const handleDelete = async (name: string) => {
    if (!confirm(`Delete memory "${name}"? This cannot be undone.`)) return;
    try {
      await invoke('delete_memory', { name });
      setMemories((prev) => prev.filter((m) => m.name !== name));
    } catch (e) {
      setError(String(e));
    }
  };

  const handleOpen = async (name: string) => {
    try {
      await invoke('open_memory_file', { name });
    } catch (e) {
      setError(String(e));
    }
  };

  if (loading) {
    return <p className="text-sm text-zinc-500 p-2">Loading memories...</p>;
  }

  if (error) {
    return (
      <div className="text-sm text-red-500 p-2">
        Failed to load: {error}
        <button className="ml-2 underline" onClick={loadMemories}>Retry</button>
      </div>
    );
  }

  if (memories.length === 0) {
    return (
      <p className="text-sm text-zinc-500 p-2">
        No memories stored yet. Memories are automatically created by the
        background memory sub-agent when you share preferences, project
        conventions, or other persistent information.
      </p>
    );
  }

  return (
    <div className="space-y-1 max-h-80 overflow-y-auto">
      {memories.map((mem) => (
        <div
          key={mem.name}
          className="flex items-center justify-between px-3 py-2 rounded hover:bg-zinc-100 dark:hover:bg-zinc-800/50 group transition-colors"
        >
          <button
            className="flex-1 text-left min-w-0"
            onClick={() => handleOpen(mem.name)}
            title="Click to open in system editor"
          >
            <div className="text-sm font-medium truncate">{mem.name}</div>
            <div className="text-xs text-zinc-500 dark:text-zinc-400 truncate">
              {mem.description}
            </div>
          </button>
          <div className="flex items-center gap-2 ml-2 shrink-0">
            <span className="text-[10px] uppercase tracking-wider px-1.5 py-0.5 rounded bg-zinc-200 dark:bg-zinc-700 text-zinc-600 dark:text-zinc-400">
              {mem.memoryType}
            </span>
            <button
              className="opacity-0 group-hover:opacity-100 text-red-500 hover:text-red-600 transition-opacity p-1"
              onClick={(e) => {
                e.stopPropagation();
                handleDelete(mem.name);
              }}
              title="Delete memory"
            >
              ×
            </button>
          </div>
        </div>
      ))}
    </div>
  );
}
