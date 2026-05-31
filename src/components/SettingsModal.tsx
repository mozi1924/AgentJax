import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { LoaderCircle, Search, Settings2, X } from 'lucide-react';
import { useI18n } from '../features/i18n';
import SettingsRenderer from './settings/SettingsRenderer';
import type {
  SettingsSectionSchema,
  SettingsSnapshot,
  SettingsSnapshotEvent,
  SettingsUiSnapshot,
} from '../features/settings/types';
import {
  appendPathSegment,
  buildOptimisticSnapshot,
  findFirstSection,
} from '../features/settings/utils';
import { filterSchemaNodesForSearch } from '../features/settings/schemaRendererView';
import { resolveLucideIcon } from '../features/icons/lucide';
import { tryGetCurrentWindow } from '../features/tauri/runtime';
import { OverlayScrollArea } from './OverlayScrollArea';

const getSectionIcon = (iconName?: string) => resolveLucideIcon(iconName, Settings2);

const sectionMatchesSearch = (
  section: SettingsSectionSchema,
  search: string,
  translate: (key: string) => string
) => {
  const normalizedSearch = search.trim().toLocaleLowerCase();
  if (!normalizedSearch) return true;
  const sectionText = [
    section.id,
    section.icon,
    section.title,
    section.description,
    translate(section.title),
    section.description ? translate(section.description) : '',
  ]
    .join(' ')
    .toLocaleLowerCase();
  return (
    sectionText.includes(normalizedSearch) ||
    filterSchemaNodesForSearch(section.children, search, translate).length > 0
  );
};

interface SettingsModalProps {
  isOpen: boolean;
  onClose: () => void;
}

export default function SettingsModal({ isOpen, onClose }: SettingsModalProps) {
  const { t } = useI18n();
  const [sections, setSections] = useState<SettingsSectionSchema[]>([]);
  const [snapshot, setSnapshot] = useState<SettingsSnapshot | null>(null);
  const [activeSectionId, setActiveSectionId] = useState('');
  const [loading, setLoading] = useState(false);
  const [loadingError, setLoadingError] = useState('');
  const [savingPath, setSavingPath] = useState<string | null>(null);
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});
  const [statusMessage, setStatusMessage] = useState('');
  const [settingsSearch, setSettingsSearch] = useState('');

  useEffect(() => {
    setActiveSectionId((current) => {
      if (current && sections.some((section) => section.id === current)) {
        return current;
      }
      return findFirstSection(sections);
    });
  }, [sections]);

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

    invoke<SettingsUiSnapshot>('get_settings_ui_snapshot')
      .then((payload) => {
        if (disposed) return;
        setSections(Array.isArray(payload.sections) ? payload.sections : []);
        setSnapshot(payload.snapshot);
        setActiveSectionId((current) => current || findFirstSection(payload.sections));
      })
      .catch((error) => {
        if (disposed) return;
        setLoadingError(typeof error === 'string' ? error : t('settings.modal.load_error'));
      })
      .finally(() => {
        if (!disposed) {
          setLoading(false);
        }
      });

    const currentWindow = tryGetCurrentWindow();
    let unlisten: (() => void) | null = null;

    if (currentWindow) {
      void currentWindow
        .listen<SettingsSnapshotEvent>('config_snapshot_changed', (event) => {
          const payload = event.payload;
          if (!payload) return;
          setSnapshot(payload);
          setSavingPath(null);
          setFieldErrors({});
          setStatusMessage(
            payload.origin === 'external'
              ? t('settings.modal.reloaded_external')
              : t('settings.modal.saved')
          );
        })
        .then((dispose) => {
          unlisten = dispose;
        });
    }

    return () => {
      disposed = true;
      if (unlisten) {
        unlisten();
      }
    };
  }, [isOpen]);

  const visibleSections = useMemo(
    () => sections.filter((section) => sectionMatchesSearch(section, settingsSearch, t)),
    [sections, settingsSearch, t]
  );

  const activeSection =
    visibleSections.find((section) => section.id === activeSectionId) ||
    visibleSections[0] ||
    sections.find((section) => section.id === activeSectionId) ||
    sections[0];

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
    setStatusMessage(operation === 'delete' ? t('settings.modal.removing_item') : t('settings.modal.saving'));
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
      setStatusMessage(operation === 'delete' ? t('settings.modal.item_removed') : t('settings.modal.saved'));
    } catch (error) {
      setSnapshot(previousSnapshot);
      setFieldErrors((current) => ({
        ...current,
        [path]: typeof error === 'string' ? error : t('settings.modal.save_failed'),
      }));
      setStatusMessage(t('settings.modal.save_failed'));
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
        className="animate-modal-content flex h-[min(86vh,860px)] w-[min(1200px,100%)] overflow-hidden rounded-[20px] border border-[#2b2b2d] bg-[#171717] shadow-2xl shadow-black/80"
      >
        <aside className="flex w-[220px] shrink-0 flex-col border-r border-[#242426]/50 bg-[#171717]">
          <div className="px-4.5 pt-4 pb-2 flex items-center justify-start">
            <button
              onClick={onClose}
              className="flex h-7 w-7 items-center justify-center rounded-lg bg-[#2e2e30] text-[#e3e3e3] hover:bg-[#3e3e40] transition"
              title={t('settings.modal.close')}
            >
              <X className="h-4 w-4" />
            </button>
          </div>

          <OverlayScrollArea containerClassName="flex-1" className="h-full space-y-1 px-3 pb-4 pt-1">
            {visibleSections.map((section) => {
              const Icon = getSectionIcon(section.icon);
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
                  <span className="min-w-0 flex-1 truncate text-[13px] font-normal">{t(section.title)}</span>
                </button>
              );
            })}
            {settingsSearch && visibleSections.length === 0 && (
              <div className="px-2 py-6 text-center text-xs text-neutral-500">
                {t('settings.modal.no_results')}
              </div>
            )}
          </OverlayScrollArea>
        </aside>

        <section className="flex min-w-0 flex-1 flex-col bg-[#171717]">
          <div className="border-b border-[#242426]/50 px-6 pt-5 pb-3">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <h3 className="font-sans text-[17px] font-bold text-white">
                {t(activeSection?.title)}
              </h3>
              <div className="flex min-w-0 items-center gap-2 text-right text-xs text-slate-500">
                <div className="relative min-w-[220px]">
                  <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-neutral-500" />
                  <input
                    value={settingsSearch}
                    onChange={(event) => setSettingsSearch(event.target.value)}
                    placeholder={t('settings.modal.search')}
                    className="h-8 w-full rounded-lg border border-[#2b2b2d] bg-[#141516] pl-8 pr-2.5 text-[12px] text-neutral-200 outline-none transition placeholder:text-neutral-600 focus:border-neutral-500"
                  />
                </div>
                {savingPath && (
                  <span className="inline-flex items-center gap-1.5 rounded-full border border-indigo-500/20 bg-indigo-500/10 px-2 py-0.5 text-indigo-200">
                    <LoaderCircle className="h-3 w-3 animate-spin" />
                    {t('settings.modal.saving')}
                  </span>
                )}
              </div>
            </div>
            {(statusMessage || loadingError) && (
              <div
                className={`mt-2.5 rounded-xl border px-3 py-2 text-xs ${
                  loadingError || Object.keys(fieldErrors).length > 0
                    ? 'border-rose-500/20 bg-rose-500/10 text-rose-200'
                    : 'border-indigo-500/15 bg-indigo-500/10 text-indigo-100'
                }`}
              >
                {loadingError || statusMessage}
              </div>
            )}
          </div>

          <OverlayScrollArea
            containerClassName="flex-1 min-h-0"
            className={`flex h-full flex-col ${activeSection?.id === 'tools' ? '' : 'px-6 py-4'}`}
          >
            {loading && (
              <div className="flex h-full items-center justify-center text-slate-400">
                <LoaderCircle className="mr-3 h-4 w-4 animate-spin" />
                {t('settings.modal.loading')}
              </div>
            )}

            {!loading && snapshot && activeSection && (
              <SettingsRenderer
                section={activeSection}
                snapshot={snapshot}
                savingPath={savingPath}
                fieldErrors={fieldErrors}
                queryState={{
                  search: settingsSearch,
                  onSearchChange: setSettingsSearch,
                }}
                onSaveField={(path, value) => applyPatch(path, value, 'set')}
                onDeletePath={(path) => applyPatch(path, undefined, 'delete')}
                onAddCollectionItem={(path, key, value) =>
                  applyPatch(appendPathSegment(path, key), value, 'set')
                }
              />
            )}
          </OverlayScrollArea>
        </section>
      </div>
    </div>
  );
}
