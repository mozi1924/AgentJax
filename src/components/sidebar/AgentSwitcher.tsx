import { useCallback, useEffect, useRef, useState } from 'react';
import { Check, ChevronDown, Plus, Trash2, User } from 'lucide-react';
import type { AgentSummary } from '../../hooks/useActiveAgent';
import { useI18n } from '../../features/i18n';

interface AgentSwitcherProps {
  agents: AgentSummary[];
  activeAgentId: string;
  activeAgent: AgentSummary | null;
  agentsLoading: boolean;
  onSwitchAgent: (agentId: string) => void;
  onCreateAgent: (agentId: string, templateId?: string) => Promise<void>;
  onDeleteAgent: (agentId: string) => Promise<void>;
}

export default function AgentSwitcher({
  agents,
  activeAgentId,
  activeAgent,
  agentsLoading,
  onSwitchAgent,
  onCreateAgent,
  onDeleteAgent,
}: AgentSwitcherProps) {
  const { t } = useI18n();
  const [isOpen, setIsOpen] = useState(false);
  const [showCreate, setShowCreate] = useState(false);
  const [newAgentId, setNewAgentId] = useState('');
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState('');
  const dropdownRef = useRef<HTMLDivElement | null>(null);
  const createInputRef = useRef<HTMLInputElement | null>(null);

  const $t = useCallback(
    (key: string, replacements?: Record<string, string>) => t(key, replacements),
    [t]
  );

  // Close dropdown on outside click
  useEffect(() => {
    const handlePointerDown = (event: PointerEvent) => {
      if (!dropdownRef.current?.contains(event.target as Node)) {
        setIsOpen(false);
        setShowCreate(false);
        setNewAgentId('');
        setCreateError('');
      }
    };
    document.addEventListener('pointerdown', handlePointerDown);
    return () => document.removeEventListener('pointerdown', handlePointerDown);
  }, []);

  // Focus create input when shown
  useEffect(() => {
    if (showCreate && createInputRef.current) {
      createInputRef.current.focus();
    }
  }, [showCreate]);

  const handleCreate = useCallback(async () => {
    const trimmed = newAgentId.trim().toLowerCase();
    if (!trimmed) {
      setCreateError($t('agent.switcher.error_id_required'));
      return;
    }
    if (!/^[a-z0-9_-]+$/.test(trimmed)) {
      setCreateError($t('agent.switcher.error_id_invalid'));
      return;
    }
    if (agents.some((a) => a.id === trimmed)) {
      setCreateError($t('agent.switcher.error_id_exists', { id: trimmed }));
      return;
    }

    setCreating(true);
    setCreateError('');
    try {
      await onCreateAgent(trimmed);
      onSwitchAgent(trimmed);
      setShowCreate(false);
      setNewAgentId('');
      setIsOpen(false);
    } catch (err: unknown) {
      setCreateError(typeof err === 'string' ? err : $t('agent.switcher.error_create_failed'));
    } finally {
      setCreating(false);
    }
  }, [newAgentId, agents, onCreateAgent, onSwitchAgent]);

  const handleDelete = useCallback(
    async (agentId: string, event: React.MouseEvent) => {
      event.stopPropagation();
      const label = agents.find((a) => a.id === agentId)?.label || agentId;
      if (!confirm($t('agent.switcher.delete_confirm', { label }))) { // eslint-disable-line no-alert
        return;
      }
      try {
        await onDeleteAgent(agentId);
      } catch (err: unknown) {
        console.error('Failed to delete agent:', err);
      }
    },
    [agents, onDeleteAgent]
  );

  const label = activeAgent?.label || activeAgentId;

  return (
    <div ref={dropdownRef} className="relative px-3 pb-2">
      <button
        onClick={() => setIsOpen((prev) => !prev)}
        disabled={agentsLoading}
        className="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm text-slate-300 transition hover:bg-[#2d2f31] disabled:opacity-50"
      >
        <User className="h-4 w-4 shrink-0 text-indigo-400" />
        <span className="min-w-0 flex-1 truncate text-left text-xs font-medium">
          {agentsLoading ? $t('agent.switcher.loading') : label}
        </span>
        <ChevronDown
          className={`h-3.5 w-3.5 shrink-0 text-slate-500 transition-transform ${
            isOpen ? 'rotate-180' : ''
          }`}
        />
      </button>

      {isOpen && (
        <div className="absolute left-3 right-3 top-full z-50 mt-1 overflow-hidden rounded-lg border border-[#2b2b2d] bg-[#1e1e1f] shadow-xl shadow-black/60">
          <div className="max-h-48 overflow-y-auto">
            {agents.map((agent) => (
              <button
                key={agent.id}
                onClick={() => {
                  onSwitchAgent(agent.id);
                  setIsOpen(false);
                }}
                className={`flex w-full items-center gap-2 px-3 py-2 text-left text-xs transition hover:bg-[#2d2f31] ${
                  agent.id === activeAgentId
                    ? 'text-indigo-300'
                    : 'text-slate-300'
                }`}
              >
                <User className="h-3.5 w-3.5 shrink-0" />
                <span className="min-w-0 flex-1 truncate">{agent.label}</span>
                {agent.id === activeAgentId && (
                  <Check className="h-3 w-3 shrink-0 text-indigo-400" />
                )}
                {agent.id !== 'main' && (
                  <Trash2
                    className="h-3 w-3 shrink-0 text-slate-600 hover:text-rose-400"
                    onClick={(e) => handleDelete(agent.id, e)}
                  />
                )}
              </button>
            ))}
          </div>

          {showCreate ? (
            <div className="border-t border-[#2b2b2d] px-3 py-2">
              <input
                ref={createInputRef}
                type="text"
                value={newAgentId}
                onChange={(e) => {
                  setNewAgentId(e.target.value);
                  setCreateError('');
                }}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') handleCreate();
                  if (e.key === 'Escape') {
                    setShowCreate(false);
                    setNewAgentId('');
                    setCreateError('');
                  }
                }}
                placeholder={$t('agent.switcher.create_placeholder')}
                className="w-full rounded-md border border-[#2b2b2d] bg-[#252526] px-2 py-1.5 text-xs text-slate-200 placeholder-slate-500 outline-none focus:border-indigo-500/50"
              />
              {createError && (
                <p className="mt-1 text-[10px] text-rose-400">{createError}</p>
              )}
              <div className="mt-1.5 flex items-center gap-1.5">
                <button
                  onClick={handleCreate}
                  disabled={creating}
                  className="flex items-center gap-1 rounded bg-indigo-600 px-2 py-1 text-[10px] text-white transition hover:bg-indigo-500 disabled:opacity-50"
                >
                  {creating ? $t('agent.switcher.creating') : $t('agent.switcher.create')}
                </button>
                <button
                  onClick={() => {
                    setShowCreate(false);
                    setNewAgentId('');
                    setCreateError('');
                  }}
                  className="rounded px-2 py-1 text-[10px] text-slate-400 transition hover:bg-[#2d2f31]"
                >
                  {$t('agent.switcher.cancel')}
                </button>
              </div>
            </div>
          ) : (
            <button
              onClick={() => setShowCreate(true)}
              className="flex w-full items-center gap-2 border-t border-[#2b2b2d] px-3 py-2 text-left text-xs text-slate-400 transition hover:bg-[#2d2f31]"
            >
              <Plus className="h-3.5 w-3.5" />
              <span>{$t('agent.switcher.new_agent')}</span>
            </button>
          )}
        </div>
      )}
    </div>
  );
}
