import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { SettingsSnapshot } from '../features/settings/types';

export interface AgentSummary {
  id: string;
  label: string;
}

const DEFAULT_AGENT_ID = 'main';

function getStoredAgentId(): string {
  try {
    return localStorage.getItem('agentjax:activeAgentId') || DEFAULT_AGENT_ID;
  } catch {
    return DEFAULT_AGENT_ID;
  }
}

function storeAgentId(agentId: string) {
  try {
    localStorage.setItem('agentjax:activeAgentId', agentId);
  } catch {
    // Ignore storage errors
  }
}

export function useActiveAgent() {
  const [agents, setAgents] = useState<AgentSummary[]>([]);
  const [activeAgentId, setActiveAgentId] = useState<string>(() => getStoredAgentId());
  const [agentsLoading, setAgentsLoading] = useState(true);
  const hasAttemptedCreate = useRef(false);

  // ── Fetch agent list ──────────────────────────────────────────────────
  const refreshAgents = useCallback(async () => {
    try {
      const list = await invoke<AgentSummary[]>('list_agents');
      setAgents(list);

      // Ensure the active agent exists
      if (list.length > 0 && !list.some((a) => a.id === activeAgentId)) {
        // Fall back to first available agent
        const fallback = list[0];
        setActiveAgentId(fallback.id);
        storeAgentId(fallback.id);
      }
    } catch {
      // Agent registry unavailable — keep current state
    } finally {
      setAgentsLoading(false);
    }
  }, [activeAgentId]);

  // ── Ensure default agent exists on first load ─────────────────────────
  useEffect(() => {
    if (hasAttemptedCreate.current) return;
    hasAttemptedCreate.current = true;

    // Try to ensure the default agent profile exists
    invoke<SettingsSnapshot>('get_settings_snapshot')
      .then(() => refreshAgents())
      .catch(() => refreshAgents());
  }, [refreshAgents]);

  // ── Switch active agent ───────────────────────────────────────────────
  const switchAgent = useCallback((agentId: string) => {
    setActiveAgentId(agentId);
    storeAgentId(agentId);
  }, []);

  // ── Create a new agent ────────────────────────────────────────────────
  const createAgent = useCallback(
    async (agentId: string, templateId?: string) => {
      await invoke('create_agent', {
        req: { agentId, templateId: templateId || null },
      });
      await refreshAgents();
    },
    [refreshAgents]
  );

  // ── Delete an agent ───────────────────────────────────────────────────
  const deleteAgent = useCallback(
    async (agentId: string) => {
      await invoke<boolean>('delete_agent', {
        req: { agentId },
      });
      if (activeAgentId === agentId) {
        // Switch to default if we deleted the active one
        switchAgent(DEFAULT_AGENT_ID);
      }
      await refreshAgents();
    },
    [activeAgentId, refreshAgents, switchAgent]
  );

  // ── Derived ───────────────────────────────────────────────────────────
  const activeAgent = useMemo(
    () => agents.find((a) => a.id === activeAgentId) || null,
    [agents, activeAgentId]
  );

  return {
    /** All discovered agent profiles */
    agents,
    /** The currently active agent ID */
    activeAgentId,
    /** The currently active agent summary object */
    activeAgent,
    /** Whether the agent list is still loading */
    agentsLoading,
    /** Refresh the agent list from the backend */
    refreshAgents,
    /** Switch to a different agent */
    switchAgent,
    /** Create a new agent profile */
    createAgent,
    /** Delete an agent profile */
    deleteAgent,
  };
}
