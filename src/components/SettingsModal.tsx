import { useEffect, useMemo, useState, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { AlertTriangle, CheckCircle, LoaderCircle, Search, Settings2, X } from 'lucide-react';
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
import {
  collectSchemaDataSourceNamespaces,
  filterSchemaNodesForSearch,
  schemaUsesDataSource,
} from '../features/settings/schemaRendererView';
import { resolveLucideIcon } from '../features/icons/lucide';
import { tryGetCurrentWindow } from '../features/tauri/runtime';
import { OverlayScrollArea } from './OverlayScrollArea';
import { useDynamicSettingsSearchIndex } from './settings/schemaRenderer/dataSources/useDynamicSettingsSearchIndex';

const getSectionIcon = (iconName?: string) => resolveLucideIcon(iconName, Settings2);

const sectionMatchesSearch = (
  section: SettingsSectionSchema,
  search: string,
  translate: (key: string) => string,
  dynamicSearchDataSources: ReadonlySet<string>
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
    [...dynamicSearchDataSources].some((dataSource) =>
      schemaUsesDataSource(section.children, dataSource)
    ) ||
    filterSchemaNodesForSearch(section.children, search, translate, {
      preserveDataSourceNodes: false,
    }).length > 0
  );
};

interface SaveStatusBannerProps {
  message: string;
  isError: boolean;
  isSaving: boolean;
  persistent: boolean;
  onDismiss: () => void;
}

function SaveStatusBanner({
  message,
  isError,
  isSaving,
  persistent,
  onDismiss,
}: SaveStatusBannerProps) {
  const [visible, setVisible] = useState(false);
  const [renderMessage, setRenderMessage] = useState('');
  const [progress, setProgress] = useState(100);
  const [secondsLeft, setSecondsLeft] = useState(4);
  
  const timerRef = useRef<any>(null);
  const intervalRef = useRef<any>(null);
  const exitTimeoutRef = useRef<any>(null);

  useEffect(() => {
    if (!message) {
      setVisible(false);
      exitTimeoutRef.current = setTimeout(() => {
        setRenderMessage('');
      }, 300);
      return () => {
        if (exitTimeoutRef.current) clearTimeout(exitTimeoutRef.current);
      };
    }

    if (exitTimeoutRef.current) {
      clearTimeout(exitTimeoutRef.current);
      exitTimeoutRef.current = null;
    }

    setRenderMessage(message);
    setVisible(true);

    if (isSaving || persistent) {
      if (timerRef.current) clearTimeout(timerRef.current);
      if (intervalRef.current) clearInterval(intervalRef.current);
      setProgress(100);
      return;
    }

    const duration = 4000;
    const intervalTime = 40;
    const totalSteps = duration / intervalTime;
    let step = 0;

    setSecondsLeft(4);
    setProgress(100);

    if (timerRef.current) clearTimeout(timerRef.current);
    if (intervalRef.current) clearInterval(intervalRef.current);

    intervalRef.current = setInterval(() => {
      step++;
      const percent = 100 - (step / totalSteps) * 100;
      setProgress(Math.max(0, percent));
      
      const seconds = Math.ceil((duration - step * intervalTime) / 1000);
      setSecondsLeft(Math.max(0, seconds));
    }, intervalTime);

    timerRef.current = setTimeout(() => {
      setVisible(false);
      if (intervalRef.current) clearInterval(intervalRef.current);
      
      exitTimeoutRef.current = setTimeout(() => {
        onDismiss();
        setRenderMessage('');
      }, 300);
    }, duration);

    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
      if (intervalRef.current) clearInterval(intervalRef.current);
    };
  }, [message, isSaving, persistent, onDismiss]);

  if (!renderMessage) return null;

  const isSavingState = isSaving || renderMessage.includes('中') || renderMessage.includes('正在');
  
  return (
    <div
      className={`relative mt-2.5 overflow-hidden rounded-xl border text-xs transition-all duration-300 ease-out px-3 py-2.5 flex items-center justify-between gap-3 ${
        visible
          ? 'opacity-100 translate-y-0 max-h-16'
          : 'opacity-0 -translate-y-2 max-h-0 py-0 border-transparent overflow-hidden'
      } ${
        isError
          ? 'border-rose-500/20 bg-rose-500/10 text-rose-200'
          : isSavingState
          ? 'border-indigo-500/15 bg-indigo-500/10 text-indigo-200'
          : 'border-emerald-500/20 bg-emerald-500/10 text-emerald-200'
      }`}
    >
      <div className="flex items-center gap-2 min-w-0">
        {isSavingState ? (
          <LoaderCircle className="h-3.5 w-3.5 animate-spin shrink-0 text-indigo-400" />
        ) : isError ? (
          <AlertTriangle className="h-3.5 w-3.5 shrink-0 text-rose-400" />
        ) : (
          <CheckCircle className="h-3.5 w-3.5 shrink-0 text-emerald-400" />
        )}
        <span className="truncate">{renderMessage}</span>
      </div>

      {!isSavingState && !persistent && (
        <span className="shrink-0 font-mono text-[10px] text-neutral-500 bg-neutral-900/40 rounded px-1 py-0.5">
          {secondsLeft}s
        </span>
      )}

      {!isSavingState && !persistent && (
        <div className="absolute bottom-0 left-0 right-0 h-0.5 bg-white/5 overflow-hidden">
          <div
            className={`h-full transition-all duration-75 ease-linear ${
              isError ? 'bg-rose-500/40' : 'bg-emerald-500/40'
            }`}
            style={{ width: `${progress}%` }}
          />
        </div>
      )}
    </div>
  );
}

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

  const dataSourceNamespaces = useMemo(
    () => collectSchemaDataSourceNamespaces(sections.flatMap((section) => section.children)),
    [sections]
  );
  const dynamicSearchDataSources = useDynamicSettingsSearchIndex({
    search: settingsSearch,
    namespaces: dataSourceNamespaces,
  });
  const visibleSections = useMemo(
    () =>
      sections.filter((section) =>
        sectionMatchesSearch(section, settingsSearch, t, dynamicSearchDataSources)
      ),
    [dynamicSearchDataSources, sections, settingsSearch, t]
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
            {(() => {
              const hasError = !!loadingError || Object.keys(fieldErrors).length > 0 || statusMessage.includes('失败');
              const activeMessage = loadingError || statusMessage;
              const isSaving = !!savingPath || statusMessage === t('settings.modal.saving') || statusMessage === t('settings.modal.removing_item');
              const persistent = !!loadingError;

              return (
                <SaveStatusBanner
                  message={activeMessage}
                  isError={hasError}
                  isSaving={isSaving}
                  persistent={persistent}
                  onDismiss={() => setStatusMessage('')}
                />
              );
            })()}
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
