import { useEffect, useMemo, useRef, useState } from 'react';
import { Plus, Trash2 } from 'lucide-react';
import { useI18n } from '../../../../features/i18n';
import { applyKeyValueEntryPatch } from '../../../../features/settings/fieldControlDrafts';
import type { SettingsFieldSchema } from '../../../../features/settings/types';
import type { KeyValueEntry } from '../../renderer/types';
import {
  buildMapFromEntries,
  mapToKeyValueEntries,
  nextKeyValueEntryId,
  parseDotenvText,
} from '../../renderer/utils';
import type { FieldControlProps } from '../types';

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

// Key/value editing owns a structured draft so blur can persist the latest local edit.
export function KeyValueField({
  field,
  value,
  resolvedPath,
  disabled,
  onSaveField,
  setIsDirty,
  setLocalError,
}: FieldControlProps) {
  const { t } = useI18n();
  const parseJsonValues = field.valueType === 'json_map';
  const keyValueMeta = useMemo(() => resolveKeyValueMeta(field), [field]);
  const [entries, setEntries] = useState<KeyValueEntry[]>([]);
  const latestEntries = useRef<KeyValueEntry[]>([]);
  const [dotenvImportOpen, setDotenvImportOpen] = useState(false);
  const [dotenvDraft, setDotenvDraft] = useState('');
  const [dotenvErrors, setDotenvErrors] = useState<string[]>([]);

  useEffect(() => {
    const nextEntries = mapToKeyValueEntries(value, { stringifyJsonValues: parseJsonValues });
    latestEntries.current = nextEntries;
    setEntries(nextEntries);
    setDotenvImportOpen(false);
    setDotenvDraft('');
    setDotenvErrors([]);
  }, [value, parseJsonValues]);

  const persistEntries = async (nextEntries: KeyValueEntry[]) => {
    const { map, error } = buildMapFromEntries(nextEntries, { parseJsonValues });
    if (error) {
      setLocalError(error);
      return;
    }
    if (!map) return;

    setLocalError(null);
    await onSaveField(resolvedPath, map);
  };

  const updateEntry = (entryId: string, patch: Partial<KeyValueEntry>) => {
    const next = applyKeyValueEntryPatch(latestEntries.current, entryId, patch);
    latestEntries.current = next;
    setEntries(next);
    setIsDirty(true);
    return next;
  };

  const importDotenv = async () => {
    const { entries: importedEntries, errors } = parseDotenvText(dotenvDraft);
    if (errors.length > 0) {
      setDotenvErrors(errors);
      return;
    }

    const baseMapResult = buildMapFromEntries(entries);
    if (baseMapResult.error || !baseMapResult.map) {
      setLocalError(baseMapResult.error || 'settings.renderer.key_value.error_empty');
      return;
    }

    const merged = { ...baseMapResult.map };
    importedEntries.forEach((entry) => {
      merged[entry.key] = entry.value;
    });

    const nextEntries = Object.entries(merged).map(([key, entryValue]) => ({
      id: nextKeyValueEntryId(),
      key,
      value: typeof entryValue === 'string' ? entryValue : JSON.stringify(entryValue),
    }));

    setDotenvErrors([]);
    setDotenvImportOpen(false);
    setDotenvDraft('');
    latestEntries.current = nextEntries;
    setEntries(nextEntries);
    setIsDirty(true);
    await persistEntries(nextEntries);
  };

  return (
    <div className="w-full space-y-2 rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 p-2">
      {entries.length === 0 && (
        <p className="px-1 py-1 text-[11px] text-neutral-500">{t(keyValueMeta.emptyState)}</p>
      )}

      {entries.map((entry) => (
        <div key={entry.id} className="flex items-center gap-2">
          <input
            value={entry.key}
            placeholder={keyValueMeta.keyPlaceholder}
            disabled={disabled}
            onChange={(event) => {
              updateEntry(entry.id, { key: event.target.value });
            }}
            onBlur={() => {
              void persistEntries(latestEntries.current);
            }}
            className="w-[42%] rounded-md border border-[#2b2b2d] bg-[#171717]/60 px-2 py-1 font-mono text-xs text-neutral-200 outline-none transition focus:border-neutral-500 disabled:opacity-50"
          />
          <input
            value={entry.value}
            placeholder={keyValueMeta.valuePlaceholder}
            disabled={disabled}
            onChange={(event) => {
              updateEntry(entry.id, { value: event.target.value });
            }}
            onBlur={() => {
              void persistEntries(latestEntries.current);
            }}
            className="flex-1 rounded-md border border-[#2b2b2d] bg-[#171717]/60 px-2 py-1 font-mono text-xs text-neutral-200 outline-none transition focus:border-neutral-500 disabled:opacity-50"
          />
          <button
            type="button"
            disabled={disabled}
            onClick={() => {
              const next = latestEntries.current.filter((item) => item.id !== entry.id);
              latestEntries.current = next;
              setEntries(next);
              setIsDirty(true);
              void persistEntries(next);
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
            setEntries((current) => {
              const next = [
              ...current,
              { id: nextKeyValueEntryId(), key: '', value: '' },
              ];
              latestEntries.current = next;
              return next;
            });
            setIsDirty(true);
          }}
          className="inline-flex items-center gap-1.5 rounded-md border border-[#2b2b2d] bg-[#2e2e30]/80 px-2 py-1 text-[11px] text-[#e3e3e3] transition hover:bg-[#3e3e40] disabled:opacity-50"
        >
          <Plus className="h-3 w-3" />
          {t(keyValueMeta.addLabel)}
        </button>

        {keyValueMeta.allowDotenvImport && (
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

      {keyValueMeta.allowDotenvImport && dotenvImportOpen && (
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
  );
}
