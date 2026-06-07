import { useState, useCallback, useRef } from 'react';
import { Bot, Plus, Trash2, AlertTriangle } from 'lucide-react';
import { useI18n } from '../../features/i18n';
import SettingsRenderer from './SettingsRenderer';
import { OverlayScrollArea } from '../OverlayScrollArea';
import type { AgentSummary } from '../../hooks/useActiveAgent';
import type {
  SettingsSectionSchema,
  SettingsSnapshot,
} from '../../features/settings/types';

interface AgentsPanelProps {
  agents: AgentSummary[];
  activeAgentId: string;
  switchAgent: (agentId: string) => void;
  createAgent: (agentId: string, templateId?: string) => Promise<void>;
  deleteAgent: (agentId: string) => Promise<void>;
  agentSections: SettingsSectionSchema[];
  snapshot: SettingsSnapshot;
  savingPath: string | null;
  fieldErrors: Record<string, string>;
  agentId: string;
  queryState: {
    search: string;
    onSearchChange: (search: string) => void;
  } | undefined;
  onSaveField: (path: string, value: unknown) => Promise<void>;
  onDeletePath: (path: string) => Promise<void>;
  onAddCollectionItem: (path: string, key: string, value: unknown) => Promise<void>;
}

export default function AgentsPanel({
  agents,
  activeAgentId,
  switchAgent,
  createAgent,
  deleteAgent,
  agentSections,
  snapshot,
  savingPath,
  fieldErrors,
  agentId,
  queryState,
  onSaveField,
  onDeletePath,
  onAddCollectionItem,
}: AgentsPanelProps) {
  const { t } = useI18n();
  const [activeTabId, setActiveTabId] = useState<string>(agentSections[0]?.id || '');
  const [showCreateInput, setShowCreateInput] = useState(false);
  const [newAgentId, setNewAgentId] = useState('');
  const [creating, setCreating] = useState(false);
  const [deleteConfirmId, setDeleteConfirmId] = useState<string | null>(null);
  const createInputRef = useRef<HTMLInputElement>(null);

  const activeSection = agentSections.find((s) => s.id === activeTabId) || agentSections[0];

  const handleCreateAgent = useCallback(async () => {
    const trimmed = newAgentId.trim();
    if (!trimmed || creating) return;
    setCreating(true);
    try {
      await createAgent(trimmed);
      setNewAgentId('');
      setShowCreateInput(false);
    } catch {
      // Error handled by parent
    } finally {
      setCreating(false);
    }
  }, [newAgentId, creating, createAgent]);

  const handleDeleteAgent = useCallback(
    async (id: string) => {
      if (id === 'main') return;
      await deleteAgent(id);
      setDeleteConfirmId(null);
    },
    [deleteAgent]
  );

  const handleSelectAgent = useCallback(
    (id: string) => {
      if (id !== activeAgentId) {
        switchAgent(id);
      }
    },
    [activeAgentId, switchAgent]
  );

  const selectedAgent = agents.find((a) => a.id === activeAgentId);
  const isMain = activeAgentId === 'main';

  return (
    <div className="flex flex-col h-full bg-[#171717]">
      {/* ── Top row: Agent info + Agent list ── */}
      <div className="flex items-start gap-4 px-6 pt-4 pb-3 border-b border-[#242426]/50">
        {/* Agent info */}
        <div className="flex items-center gap-3 min-w-0 shrink-0">
          <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-indigo-500/15 text-indigo-400">
            <Bot className="h-5 w-5" />
          </div>
          <div className="min-w-0">
            <div className="text-[15px] font-semibold text-white truncate">
              {selectedAgent?.label || activeAgentId}
            </div>
            <div className="text-[11px] text-neutral-500">
              {isMain ? t('settings.agents.type_default') : t('settings.agents.type_custom')}
            </div>
          </div>
        </div>

        {/* Agent list */}
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-1 mb-1.5">
            <span className="text-[10px] font-medium text-neutral-500 uppercase tracking-wider">
              {t('settings.agents.list_title')}
            </span>
            <span className="text-[10px] text-neutral-600">{agents.length}</span>
          </div>
          <OverlayScrollArea
            containerClassName="max-h-[72px]"
            className="flex items-center gap-1"
          >
            {agents.map((agent) => {
              const isSelected = agent.id === activeAgentId;
              const isDeleteConfirming = deleteConfirmId === agent.id;
              return (
                <div key={agent.id} className="flex items-center shrink-0">
                  <button
                    onClick={() => handleSelectAgent(agent.id)}
                    className={`flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[12px] font-medium transition shrink-0 ${
                      isSelected
                        ? 'bg-indigo-500/20 text-indigo-300 ring-1 ring-indigo-500/30'
                        : 'bg-[#1e1e20] text-neutral-400 hover:bg-[#2a2a2c] hover:text-white'
                    }`}
                  >
                    {agent.label || agent.id}
                  </button>

                  {/* Delete button (not for main) */}
                  {agent.id !== 'main' && (
                    isDeleteConfirming ? (
                      <div className="flex items-center gap-0.5 ml-0.5">
                        <button
                          onClick={() => handleDeleteAgent(agent.id)}
                          className="flex h-5 w-5 items-center justify-center rounded bg-rose-500/20 text-rose-400 hover:bg-rose-500/30 transition"
                          title={t('settings.agents.confirm_delete')}
                        >
                          <AlertTriangle className="h-3 w-3" />
                        </button>
                        <button
                          onClick={() => setDeleteConfirmId(null)}
                          className="flex h-5 w-5 items-center justify-center rounded text-neutral-500 hover:text-white transition"
                          title={t('settings.agents.cancel_delete')}
                        >
                          <span className="text-[10px]">✕</span>
                        </button>
                      </div>
                    ) : (
                      <button
                        onClick={() => setDeleteConfirmId(agent.id)}
                        className="flex h-5 w-5 items-center justify-center rounded text-neutral-600 hover:bg-rose-500/10 hover:text-rose-400 transition ml-0.5 shrink-0"
                        title={t('settings.agents.delete_agent')}
                      >
                        <Trash2 className="h-3 w-3" />
                      </button>
                    )
                  )}
                </div>
              );
            })}

            {/* Add agent button / input */}
            {showCreateInput ? (
              <div className="flex items-center gap-1 shrink-0">
                <input
                  ref={createInputRef}
                  value={newAgentId}
                  onChange={(e) => setNewAgentId(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') handleCreateAgent();
                    if (e.key === 'Escape') {
                      setShowCreateInput(false);
                      setNewAgentId('');
                    }
                  }}
                  placeholder={t('settings.agents.create_placeholder')}
                  className="h-7 w-32 rounded-lg border border-indigo-500/30 bg-[#1e1e20] px-2 text-[11px] text-white outline-none placeholder:text-neutral-600 focus:border-indigo-500/60"
                  autoFocus
                />
                <button
                  onClick={handleCreateAgent}
                  disabled={!newAgentId.trim() || creating}
                  className="flex h-7 w-7 items-center justify-center rounded-lg bg-indigo-500/20 text-indigo-400 hover:bg-indigo-500/30 transition disabled:opacity-30"
                >
                  <Plus className="h-3.5 w-3.5" />
                </button>
              </div>
            ) : (
              <button
                onClick={() => {
                  setShowCreateInput(true);
                  setTimeout(() => createInputRef.current?.focus(), 50);
                }}
                className="flex h-7 w-7 items-center justify-center rounded-lg border border-dashed border-[#2b2b2d] text-neutral-600 hover:border-indigo-500/30 hover:text-indigo-400 transition shrink-0"
                title={t('settings.agents.create_agent')}
              >
                <Plus className="h-3.5 w-3.5" />
              </button>
            )}
          </OverlayScrollArea>
        </div>
      </div>

      {/* ── Horizontal tabs ── */}
      <div className="flex items-center gap-0 px-6 border-b border-[#242426]/50">
        {agentSections.map((section) => {
          const isActive = section.id === activeTabId;
          return (
            <button
              key={section.id}
              onClick={() => setActiveTabId(section.id)}
              className={`relative px-3.5 py-2.5 text-[12px] font-medium transition shrink-0 ${
                isActive
                  ? 'text-white'
                  : 'text-neutral-500 hover:text-neutral-300'
              }`}
            >
              {t(section.title)}
              {isActive && (
                <div className="absolute bottom-0 left-2 right-2 h-0.5 rounded-full bg-indigo-500" />
              )}
            </button>
          );
        })}
      </div>

      {/* ── Tab content ── */}
      <OverlayScrollArea
        containerClassName="flex-1 min-h-0"
        className="flex h-full flex-col px-6 py-4"
      >
        {activeSection && (
          <SettingsRenderer
            section={activeSection}
            snapshot={snapshot}
            savingPath={savingPath}
            fieldErrors={fieldErrors}
            agentId={agentId}
            queryState={queryState}
            onSaveField={onSaveField}
            onDeletePath={onDeletePath}
            onAddCollectionItem={onAddCollectionItem}
          />
        )}
      </OverlayScrollArea>
    </div>
  );
}
