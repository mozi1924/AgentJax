import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import {
  Bot,
  Cpu,
  LoaderCircle,
  PlugZap,
  ServerCog,
  Settings2,
  X,
} from 'lucide-react';
import SettingsRenderer from './settings/SettingsRenderer';
import { createSettingsRegistry } from '../features/settings/registry';
import type { SettingsSnapshot, SettingsSnapshotEvent } from '../features/settings/types';
import { buildOptimisticSnapshot, findFirstSection } from '../features/settings/utils';

const SECTION_ICONS = {
  Settings2,
  PlugZap,
  Bot,
  Cpu,
  ServerCog,
} as const;

type SectionIconName = keyof typeof SECTION_ICONS;

interface SettingsModalProps {
  isOpen: boolean;
  onClose: () => void;
}

export default function SettingsModal({ isOpen, onClose }: SettingsModalProps) {
  const registry = useMemo(() => createSettingsRegistry(), []);
  const [snapshot, setSnapshot] = useState<SettingsSnapshot | null>(null);
  const [activeSectionId, setActiveSectionId] = useState(findFirstSection(registry.sections));
  const [loading, setLoading] = useState(false);
  const [loadingError, setLoadingError] = useState('');
  const [savingPath, setSavingPath] = useState<string | null>(null);
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});
  const [statusMessage, setStatusMessage] = useState('');

  useEffect(() => {
    setActiveSectionId((current) => current || findFirstSection(registry.sections));
  }, [registry.sections]);

  useEffect(() => {
    if (!isOpen) {
      return undefined;
    }

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        onClose();
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [isOpen, onClose]);

  useEffect(() => {
    if (!isOpen) {
      return undefined;
    }

    let disposed = false;
    setLoading(true);
    setLoadingError('');
    setStatusMessage('');

    invoke<SettingsSnapshot>('get_settings_snapshot')
      .then((nextSnapshot) => {
        if (disposed) return;
        setSnapshot(nextSnapshot);
      })
      .catch((error) => {
        if (disposed) return;
        setLoadingError(typeof error === 'string' ? error : '无法加载设置快照。');
      })
      .finally(() => {
        if (!disposed) {
          setLoading(false);
        }
      });

    const currentWindow = getCurrentWindow();
    let unlisten: (() => void) | null = null;

    void currentWindow
      .listen<SettingsSnapshotEvent>('config_snapshot_changed', (event) => {
        const payload = event.payload;
        if (!payload) return;
        setSnapshot(payload);
        setSavingPath(null);
        setFieldErrors({});
        setStatusMessage(
          payload.origin === 'external'
            ? 'Configuration reloaded from disk.'
            : 'Settings saved.'
        );
      })
      .then((dispose) => {
        unlisten = dispose;
      });

    return () => {
      disposed = true;
      if (unlisten) {
        unlisten();
      }
    };
  }, [isOpen]);

  const activeSection =
    registry.sections.find((section) => section.id === activeSectionId) || registry.sections[0];

  const applyPatch = async (
    path: string,
    value: unknown,
    operation: 'set' | 'delete' = 'set'
  ) => {
    if (!snapshot) return;
    const previousSnapshot = snapshot;

    setSavingPath(path);
    setFieldErrors((current) => {
      const next = { ...current };
      delete next[path];
      return next;
    });
    setStatusMessage(operation === 'delete' ? 'Removing item…' : 'Saving…');
    setSnapshot(buildOptimisticSnapshot(snapshot, path, value, operation));

    try {
      const nextSnapshot = await invoke<SettingsSnapshot>('apply_settings_patch', {
        patch: {
          path,
          value,
          expectedRevision: previousSnapshot.revision,
          operation,
        },
      });
      setSnapshot(nextSnapshot);
      setStatusMessage(operation === 'delete' ? 'Item removed.' : 'Settings saved.');
    } catch (error) {
      setSnapshot(previousSnapshot);
      setFieldErrors((current) => ({
        ...current,
        [path]: typeof error === 'string' ? error : '保存失败，请稍后重试。',
      }));
      setStatusMessage('Save failed.');
    } finally {
      setSavingPath(null);
    }
  };

  if (!isOpen) {
    return null;
  }

  return (
    <div
      onClick={onClose}
      className="animate-modal-backdrop fixed inset-0 z-[80] flex items-center justify-center bg-black/70 px-5 py-6 backdrop-blur-md"
    >
      <div
        onClick={(event) => event.stopPropagation()}
        className="animate-modal-content flex h-[min(86vh,820px)] w-[min(1080px,100%)] overflow-hidden rounded-[28px] border border-[#2b2b2d] bg-[#1a1b1d] shadow-2xl shadow-black/70"
      >
        <aside className="flex w-[240px] shrink-0 flex-col border-r border-[#2b2b2d] bg-[#202123]">
          <div className="flex items-center justify-between px-4 py-4">
            <div>
              <p className="text-xs font-semibold uppercase tracking-[0.24em] text-slate-500">Settings</p>
              <h2 className="mt-1 font-sans text-xl font-semibold text-slate-100">AgentJax</h2>
            </div>
            <button
              onClick={onClose}
              className="flex h-9 w-9 items-center justify-center rounded-xl text-slate-400 transition hover:bg-[#2b2b2d] hover:text-slate-200"
              title="Close"
            >
              <X className="h-4.5 w-4.5" />
            </button>
          </div>

          <div className="scrollbar-thin flex-1 space-y-1 overflow-y-auto px-3 pb-4">
            {registry.sections.map((section) => {
              const Icon = SECTION_ICONS[section.icon as SectionIconName] || Settings2;
              const isActive = section.id === activeSection?.id;
              return (
                <button
                  key={section.id}
                  onClick={() => setActiveSectionId(section.id)}
                  className={`flex w-full items-center gap-3 rounded-2xl px-3 py-3 text-left transition ${
                    isActive
                      ? 'bg-[#323335] text-white'
                      : 'text-slate-300 hover:bg-[#2a2b2e] hover:text-white'
                  }`}
                >
                  <Icon className="h-4.5 w-4.5 shrink-0" />
                  <span className="min-w-0 flex-1 truncate text-sm font-medium">{section.title}</span>
                </button>
              );
            })}
          </div>
        </aside>

        <section className="flex min-w-0 flex-1 flex-col bg-[#1d1e20]">
          <div className="border-b border-[#2b2b2d] px-8 py-6">
            <div className="flex items-start justify-between gap-6">
              <div className="min-w-0">
                <h3 className="font-sans text-2xl font-semibold tracking-tight text-slate-100">
                  {activeSection?.title}
                </h3>
                {activeSection?.description && (
                  <p className="mt-2 text-sm leading-6 text-slate-400">{activeSection.description}</p>
                )}
                {snapshot?.configPath && (
                  <p className="mt-3 text-xs text-slate-500">Config: {snapshot.configPath}</p>
                )}
              </div>
              <div className="min-w-[150px] text-right text-xs text-slate-500">
                {savingPath && (
                  <span className="inline-flex items-center gap-2 rounded-full border border-cyan-400/20 bg-cyan-400/10 px-3 py-1 text-cyan-200">
                    <LoaderCircle className="h-3.5 w-3.5 animate-spin" />
                    Saving
                  </span>
                )}
              </div>
            </div>
            {(statusMessage || loadingError) && (
              <div
                className={`mt-4 rounded-2xl border px-4 py-3 text-sm ${
                  loadingError || Object.keys(fieldErrors).length > 0
                    ? 'border-rose-500/20 bg-rose-500/10 text-rose-200'
                    : 'border-cyan-400/15 bg-cyan-400/10 text-cyan-100'
                }`}
              >
                {loadingError || statusMessage}
              </div>
            )}
          </div>

          <div className="scrollbar-thin flex-1 overflow-y-auto px-8 py-6">
            {loading && (
              <div className="flex h-full items-center justify-center text-slate-400">
                <LoaderCircle className="mr-3 h-5 w-5 animate-spin" />
                Loading settings…
              </div>
            )}

            {!loading && snapshot && activeSection && (
              <SettingsRenderer
                section={activeSection}
                snapshot={snapshot}
                savingPath={savingPath}
                fieldErrors={fieldErrors}
                onSaveField={(path, value) => applyPatch(path, value, 'set')}
                onDeletePath={(path) => applyPatch(path, undefined, 'delete')}
                onAddCollectionItem={(path, key, value) => applyPatch(`${path}.${key}`, value, 'set')}
              />
            )}
          </div>
        </section>
      </div>
    </div>
  );
}
