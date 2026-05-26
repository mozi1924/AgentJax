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
        className="animate-modal-content flex h-[min(80vh,600px)] w-[min(760px,100%)] overflow-hidden rounded-[20px] border border-[#2b2b2d] bg-[#171717] shadow-2xl shadow-black/80"
      >
        <aside className="flex w-[210px] shrink-0 flex-col border-r border-[#242426]/50 bg-[#171717]">
          <div className="px-4.5 pt-4 pb-2 flex items-center justify-start">
            <button
              onClick={onClose}
              className="flex h-7 w-7 items-center justify-center rounded-lg bg-[#2e2e30] text-[#e3e3e3] hover:bg-[#3e3e40] transition"
              title="Close"
            >
              <X className="h-4 w-4" />
            </button>
          </div>

          <div className="scrollbar-thin flex-1 space-y-1 overflow-y-auto px-3 pb-4 pt-1">
            {registry.sections.map((section) => {
              const Icon = SECTION_ICONS[section.icon as SectionIconName] || Settings2;
              const isActive = section.id === activeSection?.id;
              return (
                <button
                  key={section.id}
                  onClick={() => setActiveSectionId(section.id)}
                  className={`flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left transition ${
                    isActive
                      ? 'bg-[#2a2a2c] text-white'
                      : 'text-neutral-400 hover:bg-[#202022] hover:text-white'
                  }`}
                >
                  <Icon className="h-4 w-4 shrink-0" />
                  <span className="min-w-0 flex-1 truncate text-[13px] font-normal">{section.title}</span>
                </button>
              );
            })}
          </div>
        </aside>

        <section className="flex min-w-0 flex-1 flex-col bg-[#171717]">
          <div className="border-b border-[#242426]/50 px-6 pt-5 pb-3">
            <div className="flex items-center justify-between">
              <h3 className="font-sans text-[17px] font-bold text-white">
                {activeSection?.title}
              </h3>
              <div className="text-right text-xs text-slate-500">
                {savingPath && (
                  <span className="inline-flex items-center gap-1.5 rounded-full border border-cyan-400/20 bg-cyan-400/10 px-2 py-0.5 text-cyan-200">
                    <LoaderCircle className="h-3 w-3 animate-spin" />
                    Saving
                  </span>
                )}
              </div>
            </div>
            {(statusMessage || loadingError) && (
              <div
                className={`mt-2.5 rounded-xl border px-3 py-2 text-xs ${
                  loadingError || Object.keys(fieldErrors).length > 0
                    ? 'border-rose-500/20 bg-rose-500/10 text-rose-200'
                    : 'border-cyan-400/15 bg-cyan-400/10 text-cyan-100'
                }`}
              >
                {loadingError || statusMessage}
              </div>
            )}
          </div>

          <div className="scrollbar-thin flex-1 overflow-y-auto px-6 py-4">
            {loading && (
              <div className="flex h-full items-center justify-center text-slate-400">
                <LoaderCircle className="mr-3 h-4 w-4 animate-spin" />
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
