import { useEffect, useRef, useState } from 'react';
import { Plus, Trash2 } from 'lucide-react';
import type { SettingsFieldSchema, SettingsSnapshot } from '../../../features/settings/types';
import { useI18n } from '../../../features/i18n';
import {
  getFieldOptions,
  getValueAtPath,
  isNodeEnabled,
  resolvePath,
  validateFieldValue,
} from '../../../features/settings/utils';
import type { KeyValueEntry } from './types';
import {
  buildMapFromEntries,
  coerceSelectValue,
  isFullWidthControl,
  mapToKeyValueEntries,
  normalizeFieldValueForDraft,
  nextKeyValueEntryId,
  parseDotenvText,
} from './utils';

const resolveKeyValueMeta = (field: SettingsFieldSchema) => {
  const id = field.id.toLowerCase();
  const path = field.path.toLowerCase();

  if (path.includes('env_http_headers') || id.includes('env-http-headers')) {
    return {
      emptyState: 'settings.renderer.key_value.env_http_headers.empty',
      addLabel: 'settings.renderer.key_value.env_http_headers.add',
      removeTitle: 'settings.renderer.key_value.env_http_headers.remove',
      keyPlaceholder: 'Header-Name',
      valuePlaceholder: 'ENV_VAR',
      allowDotenvImport: false,
    };
  }

  if (path.includes('header') || id.includes('header')) {
    return {
      emptyState: 'settings.renderer.key_value.headers.empty',
      addLabel: 'settings.renderer.key_value.headers.add',
      removeTitle: 'settings.renderer.key_value.headers.remove',
      keyPlaceholder: 'Header-Name',
      valuePlaceholder: 'value',
      allowDotenvImport: false,
    };
  }

  if (path.endsWith('.env') || path === 'env' || id.includes('-env')) {
    return {
      emptyState: 'settings.renderer.key_value.env.empty',
      addLabel: 'settings.renderer.key_value.env.add',
      removeTitle: 'settings.renderer.key_value.env.remove',
      keyPlaceholder: 'KEY',
      valuePlaceholder: 'value',
      allowDotenvImport: true,
    };
  }

  if (path.includes('query_params') || id.includes('query-params')) {
    return {
      emptyState: 'settings.renderer.key_value.query_params.empty',
      addLabel: 'settings.renderer.key_value.query_params.add',
      removeTitle: 'settings.renderer.key_value.query_params.remove',
      keyPlaceholder: 'param',
      valuePlaceholder: 'value',
      allowDotenvImport: false,
    };
  }

  return {
    emptyState: 'settings.renderer.key_value.empty',
    addLabel: 'settings.renderer.key_value.add',
    removeTitle: 'settings.renderer.key_value.remove',
    keyPlaceholder: 'key',
    valuePlaceholder: 'value',
    allowDotenvImport: false,
  };
};

export function FieldRow({
  field,
  snapshot,
  savingPath,
  fieldErrors,
  contextPath,
  onSaveField,
}: {
  field: SettingsFieldSchema;
  snapshot: SettingsSnapshot;
  savingPath: string | null;
  fieldErrors: Record<string, string>;
  contextPath?: string;
  onSaveField: (path: string, value: unknown) => Promise<void>;
}) {
  const { t } = useI18n();
  const resolvedPath = resolvePath(field.path, contextPath);
  const value = getValueAtPath(snapshot.values, resolvedPath);
  const secretStatus = snapshot.secretStatuses[resolvedPath];
  const [draft, setDraft] = useState(normalizeFieldValueForDraft(field, value, secretStatus));
  const [keyValueEntries, setKeyValueEntries] = useState<KeyValueEntry[]>([]);
  const latestKeyValueEntries = useRef<KeyValueEntry[]>([]);
  const [dotenvImportOpen, setDotenvImportOpen] = useState(false);
  const [dotenvDraft, setDotenvDraft] = useState('');
  const [dotenvErrors, setDotenvErrors] = useState<string[]>([]);
  const [isDirty, setIsDirty] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);
  const isSaving = savingPath === resolvedPath;
  const disabled = !isNodeEnabled(field, snapshot, contextPath) || isSaving;
  const options = getFieldOptions(field, snapshot, contextPath);
  const parseJsonValues = field.valueType === 'json_map';
  const keyValueMeta = resolveKeyValueMeta(field);
  const allowDotenvImport = field.control === 'key_value' && keyValueMeta.allowDotenvImport;

  useEffect(() => {
    setDraft(normalizeFieldValueForDraft(field, value, secretStatus));
    if (field.control === 'key_value') {
      const nextEntries = mapToKeyValueEntries(value, { stringifyJsonValues: parseJsonValues });
      latestKeyValueEntries.current = nextEntries;
      setKeyValueEntries(nextEntries);
      setDotenvImportOpen(false);
      setDotenvDraft('');
      setDotenvErrors([]);
    }
    setIsDirty(false);
    setLocalError(null);
  }, [field, value, secretStatus, snapshot.revision, parseJsonValues]);

  const persistKeyValueEntries = async (entries: KeyValueEntry[]) => {
    const { map, error } = buildMapFromEntries(entries, { parseJsonValues });
    if (error) {
      setLocalError(error);
      return;
    }

    setLocalError(null);
    if (!map) {
      return;
    }

    await onSaveField(resolvedPath, map);
  };

  const importDotenv = async () => {
    const { entries, errors } = parseDotenvText(dotenvDraft);
    if (errors.length > 0) {
      setDotenvErrors(errors);
      return;
    }

    const baseMapResult = buildMapFromEntries(keyValueEntries);
    if (baseMapResult.error || !baseMapResult.map) {
      setLocalError(baseMapResult.error || 'settings.renderer.key_value.error_empty');
      return;
    }

    const merged = { ...baseMapResult.map };
    entries.forEach((entry) => {
      merged[entry.key] = entry.value;
    });

    const nextEntries = Object.entries(merged).map(([key, value]) => ({
      id: nextKeyValueEntryId(),
      key,
      value: typeof value === 'string' ? value : JSON.stringify(value),
    }));

    setDotenvErrors([]);
    setDotenvImportOpen(false);
    setDotenvDraft('');
    latestKeyValueEntries.current = nextEntries;
    setKeyValueEntries(nextEntries);
    setIsDirty(true);
    await persistKeyValueEntries(nextEntries);
  };

  const commit = async (nextValue: unknown) => {
    const validationError = validateFieldValue(field, nextValue);
    if (validationError) {
      setLocalError(validationError);
      return;
    }

    setLocalError(null);
    if (!isDirty && field.control === 'secret' && `${draft}`.trim() === '') {
      return;
    }

    if (!isDirty && field.control !== 'switch' && field.control !== 'select') {
      return;
    }

    await onSaveField(resolvedPath, nextValue);
  };

  const rawHelper = fieldErrors[resolvedPath] || localError || field.helpText;
  const helperText = rawHelper ? t(rawHelper) : '';
  const fullWidth = isFullWidthControl(field.control);

  return (
    <div className="border-b border-[#242426]/30 py-3 first:pt-0 last:border-b-0">
      <div
        className={`flex ${
          fullWidth ? 'flex-col gap-2' : 'flex-row items-center justify-between gap-4'
        }`}
      >
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5 flex-wrap">
            <h4 className="text-[13.5px] font-medium text-neutral-200">{t(field.title)}</h4>
            {field.advanced && (
              <span className="rounded px-1.5 py-0.5 text-[9px] font-semibold bg-[#2e2e30] text-neutral-400 uppercase tracking-wider">
                {t('settings.renderer.advanced')}
              </span>
            )}
          </div>
          {field.description && (
            <p className="mt-0.5 text-[11.5px] leading-relaxed text-neutral-400/80 max-w-[95%]">
              {t(field.description)}
            </p>
          )}
          {field.control === 'secret' && secretStatus && (
            <p className="mt-1 text-[11px] text-neutral-500">
              {secretStatus.configured
                ? t('settings.renderer.secret.configured', { source: secretStatus.source })
                : t('settings.renderer.secret.not_configured')}
            </p>
          )}
          {field.warningText && (
            <p className="mt-1 text-[11px] text-amber-500/80">{t(field.warningText)}</p>
          )}
          {helperText && (
            <p
              className={`mt-1 text-[11px] ${
                fieldErrors[resolvedPath] || localError ? 'text-rose-400' : 'text-neutral-500'
              }`}
            >
              {helperText}
            </p>
          )}
        </div>

        <div className={fullWidth ? 'w-full mt-1.5' : 'shrink-0 flex items-center justify-end'}>
          {field.control === 'switch' && (
            <label className="relative inline-flex h-5 w-9 items-center cursor-pointer">
              <input
                type="checkbox"
                checked={!!value}
                disabled={disabled}
                onChange={(event) => {
                  void onSaveField(resolvedPath, event.target.checked);
                }}
                className="peer sr-only"
              />
              <span className="absolute inset-0 rounded-full bg-[#3e3e42] transition peer-checked:bg-[#007aff] peer-disabled:opacity-50" />
              <span className="absolute left-0.5 top-0.5 h-4 w-4 rounded-full bg-white transition-transform duration-200 peer-checked:translate-x-4" />
            </label>
          )}

          {field.control === 'select' && (
            <div className="relative inline-flex items-center">
              {field.id === 'accent-color' && (
                <span
                  className="w-2.5 h-2.5 rounded-full mr-1 transition-colors shrink-0"
                  style={{
                    backgroundColor:
                      draft === 'green'
                        ? '#10a37f'
                        : draft === 'blue'
                          ? '#007aff'
                          : draft === 'purple'
                            ? '#a855f7'
                            : draft === 'orange'
                              ? '#f97316'
                              : '#737373',
                  }}
                />
              )}
              <select
                value={draft}
                disabled={disabled}
                onChange={(event) => {
                  const rawValue = event.target.value;
                  setDraft(rawValue);
                  setIsDirty(true);
                  void onSaveField(resolvedPath, coerceSelectValue(field, rawValue));
                }}
                className="appearance-none bg-transparent hover:bg-neutral-800/40 text-neutral-200 text-[13px] font-normal pr-5 pl-2 py-0.5 rounded-md cursor-pointer outline-none transition text-right disabled:opacity-50"
              >
                {options.map((option) => (
                  <option
                    key={`${field.id}-${option.value}`}
                    value={option.value}
                    className="bg-[#1a1b1d] text-neutral-200"
                  >
                    {t(option.label)}
                  </option>
                ))}
              </select>
              <span className="pointer-events-none absolute right-1 top-1/2 -translate-y-1/2 text-neutral-500">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none">
                  <path
                    d="M6 9l6 6 6-6"
                    stroke="currentColor"
                    strokeWidth="2"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                  />
                </svg>
              </span>
            </div>
          )}

          {field.control === 'text' && (
            <input
              type="text"
              value={draft}
              placeholder={field.placeholder ? t(field.placeholder) : ''}
              disabled={disabled}
              onChange={(event) => {
                setDraft(event.target.value);
                setIsDirty(true);
              }}
              onBlur={() => {
                void commit(draft);
              }}
              className="w-[240px] rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 px-2.5 py-1 text-xs text-neutral-200 outline-none transition focus:border-neutral-500 focus:bg-[#222326]/40 disabled:opacity-50"
            />
          )}

          {field.control === 'secret' && (
            <input
              type="password"
              value={draft}
              placeholder={field.placeholder ? t(field.placeholder) : ''}
              disabled={disabled}
              onChange={(event) => {
                setDraft(event.target.value);
                setIsDirty(true);
              }}
              onBlur={() => {
                void commit(draft);
              }}
              className="w-[240px] rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 px-2.5 py-1 text-xs text-neutral-200 outline-none transition focus:border-neutral-500 focus:bg-[#222326]/40 disabled:opacity-50"
            />
          )}

          {field.control === 'textarea' && (
            <textarea
              rows={field.rows || 3}
              value={draft}
              placeholder={field.placeholder ? t(field.placeholder) : ''}
              disabled={disabled}
              onChange={(event) => {
                setDraft(event.target.value);
                setIsDirty(true);
              }}
              onBlur={() => {
                void commit(draft);
              }}
              className="w-full rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 px-2.5 py-2 text-xs text-neutral-200 outline-none transition focus:border-neutral-500 focus:bg-[#222326]/40 disabled:opacity-50"
            />
          )}

          {field.control === 'number' && (
            <input
              type="number"
              value={draft}
              placeholder={field.placeholder ? t(field.placeholder) : ''}
              min={field.min}
              max={field.max}
              step={field.step || (field.valueType === 'integer' ? 1 : 0.1)}
              disabled={disabled}
              onChange={(event) => {
                setDraft(event.target.value);
                setIsDirty(true);
              }}
              onBlur={() => {
                const nextValue =
                  draft === ''
                    ? null
                    : field.valueType === 'integer'
                      ? Number.parseInt(draft, 10)
                      : Number(draft);
                void commit(nextValue);
              }}
              className="w-[100px] text-right rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 px-2.5 py-1 text-xs text-neutral-200 outline-none transition focus:border-neutral-500 focus:bg-[#222326]/40 disabled:opacity-50"
            />
          )}

          {field.control === 'tags' && (
            <textarea
              rows={2}
              value={draft}
              placeholder="Use commas to separate items"
              disabled={disabled}
              onChange={(event) => {
                setDraft(event.target.value);
                setIsDirty(true);
              }}
              onBlur={() => {
                const nextValue = draft
                  .split(',')
                  .map((entry) => entry.trim())
                  .filter(Boolean);
                void commit(nextValue);
              }}
              className="w-full rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 px-2.5 py-2 text-xs text-neutral-200 outline-none transition focus:border-neutral-500 focus:bg-[#222326]/40 disabled:opacity-50"
            />
          )}

          {field.control === 'key_value' && (
            <div className="w-full space-y-2 rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 p-2">
              {keyValueEntries.length === 0 && (
                <p className="px-1 py-1 text-[11px] text-neutral-500">
                  {t(keyValueMeta.emptyState)}
                </p>
              )}

              {keyValueEntries.map((entry) => (
                <div key={entry.id} className="flex items-center gap-2">
                  <input
                    value={entry.key}
                    placeholder={keyValueMeta.keyPlaceholder}
                    disabled={disabled}
                    onChange={(event) => {
                      const next = latestKeyValueEntries.current.map((item) =>
                        item.id === entry.id ? { ...item, key: event.target.value } : item
                      );
                      latestKeyValueEntries.current = next;
                      setKeyValueEntries(next);
                      setIsDirty(true);
                    }}
                    onBlur={() => {
                      void persistKeyValueEntries(latestKeyValueEntries.current);
                    }}
                    className="w-[42%] rounded-md border border-[#2b2b2d] bg-[#171717]/60 px-2 py-1 font-mono text-xs text-neutral-200 outline-none transition focus:border-neutral-500 disabled:opacity-50"
                  />
                  <input
                    value={entry.value}
                    placeholder={keyValueMeta.valuePlaceholder}
                    disabled={disabled}
                    onChange={(event) => {
                      const next = latestKeyValueEntries.current.map((item) =>
                        item.id === entry.id ? { ...item, value: event.target.value } : item
                      );
                      latestKeyValueEntries.current = next;
                      setKeyValueEntries(next);
                      setIsDirty(true);
                    }}
                    onBlur={() => {
                      void persistKeyValueEntries(latestKeyValueEntries.current);
                    }}
                    className="flex-1 rounded-md border border-[#2b2b2d] bg-[#171717]/60 px-2 py-1 font-mono text-xs text-neutral-200 outline-none transition focus:border-neutral-500 disabled:opacity-50"
                  />
                  <button
                    type="button"
                    disabled={disabled}
                    onClick={() => {
                      const next = latestKeyValueEntries.current.filter((item) => item.id !== entry.id);
                      latestKeyValueEntries.current = next;
                      setKeyValueEntries(next);
                      setIsDirty(true);
                      void persistKeyValueEntries(next);
                    }}
                    className="rounded-md p-1 text-neutral-500 transition hover:bg-rose-500/10 hover:text-rose-300 disabled:opacity-50"
                    title={t(keyValueMeta.removeTitle)}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </button>
                </div>
              ))}

              <div className="flex flex-wrap items-center gap-2">
                <button
                  type="button"
                  disabled={disabled}
                  onClick={() => {
                    setKeyValueEntries((current) => {
                      const next = [
                        ...current,
                        { id: nextKeyValueEntryId(), key: '', value: '' },
                      ];
                      latestKeyValueEntries.current = next;
                      return next;
                    });
                    setIsDirty(true);
                  }}
                  className="inline-flex items-center gap-1.5 rounded-md border border-[#2b2b2d] bg-[#2e2e30]/80 px-2 py-1 text-[11px] text-[#e3e3e3] transition hover:bg-[#3e3e40] disabled:opacity-50"
                >
                  <Plus className="h-3 w-3" />
                  {t(keyValueMeta.addLabel)}
                </button>

                {allowDotenvImport && (
                  <button
                    type="button"
                    disabled={disabled}
                    onClick={() => {
                      setDotenvImportOpen((current) => !current);
                      setDotenvErrors([]);
                    }}
                    className="inline-flex items-center gap-1.5 rounded-md border border-[#2b2b2d] bg-[#2e2e30]/80 px-2 py-1 text-[11px] text-[#e3e3e3] transition hover:bg-[#3e3e40] disabled:opacity-50"
                  >
                    {t('settings.renderer.key_value.dotenv_import')}
                  </button>
                )}
              </div>

              {allowDotenvImport && dotenvImportOpen && (
                <div className="mt-2 rounded-md border border-[#2b2b2d] bg-[#171717]/50 p-2">
                  <textarea
                    rows={6}
                    value={dotenvDraft}
                    placeholder={'EXAMPLE_KEY=example\nAPI_URL=https://example.com\nexport TOKEN=abc'}
                    disabled={disabled}
                    onChange={(event) => {
                      setDotenvDraft(event.target.value);
                      setDotenvErrors([]);
                    }}
                    className="w-full rounded-md border border-[#2b2b2d] bg-[#111112]/70 px-2 py-1.5 font-mono text-xs text-neutral-200 outline-none transition focus:border-neutral-500 disabled:opacity-50"
                  />
                  {dotenvErrors.length > 0 && (
                    <div className="mt-2 space-y-0.5 text-[11px] text-rose-300">
                      {dotenvErrors.map((error) => (
                        <p key={error}>{error}</p>
                      ))}
                    </div>
                  )}
                  <div className="mt-2 flex items-center gap-2">
                    <button
                      type="button"
                      disabled={disabled}
                      onClick={() => {
                        void importDotenv();
                      }}
                      className="inline-flex items-center gap-1 rounded-md bg-neutral-200 px-2 py-1 text-[11px] font-medium text-neutral-900 transition hover:bg-white disabled:opacity-50"
                    >
                      {t('settings.renderer.key_value.import')}
                    </button>
                    <button
                      type="button"
                      disabled={disabled}
                      onClick={() => {
                        setDotenvImportOpen(false);
                        setDotenvDraft('');
                        setDotenvErrors([]);
                      }}
                      className="inline-flex items-center gap-1 rounded-md border border-[#2b2b2d] px-2 py-1 text-[11px] text-neutral-300 transition hover:bg-[#2e2e30]/70 disabled:opacity-50"
                    >
                      {t('settings.renderer.key_value.cancel')}
                    </button>
                  </div>
                </div>
              )}
            </div>
          )}

          {field.control === 'json' && (
            <textarea
              rows={5}
              value={draft}
              placeholder={'{\n  \"custom\": true\n}'}
              disabled={disabled}
              onChange={(event) => {
                setDraft(event.target.value);
                setIsDirty(true);
              }}
              onBlur={() => {
                try {
                  const parsed = draft.trim() ? JSON.parse(draft) : {};
                  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
                    setLocalError('settings.renderer.json.invalid');
                    return;
                  }
                  void commit(parsed);
                } catch {
                  setLocalError('settings.renderer.json.invalid');
                }
              }}
              className="w-full rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 px-2.5 py-2 font-mono text-xs text-neutral-200 outline-none transition focus:border-neutral-500 focus:bg-[#222326]/40 disabled:opacity-50"
            />
          )}
        </div>
      </div>
    </div>
  );
}
