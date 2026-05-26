import type {
  SecretStatus,
  SettingsCollectionSchema,
  SettingsFieldSchema,
} from '../../../features/settings/types';
import { asRecord, asStringArray } from '../../../features/settings/utils';
import type { KeyValueEntry } from './types';

let keyValueEntrySeed = 0;

export const nextKeyValueEntryId = () => {
  keyValueEntrySeed += 1;
  return `kv-${keyValueEntrySeed}`;
};

const formatPrimitive = (value: unknown) => {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return `${value}`;
  return '';
};

export const createDefaultItem = (collection: SettingsCollectionSchema, key: string) => {
  const next = JSON.parse(JSON.stringify(collection.defaultItem)) as Record<string, unknown>;

  if (collection.path === 'providers') {
    next.credential_env = `${key.replace(/[^A-Za-z0-9]/g, '_').toUpperCase()}_API_KEY`;
  }

  if (collection.path === 'models' && typeof next.model === 'string' && !next.model) {
    next.model = key;
  }

  return next;
};

export const coerceSelectValue = (field: SettingsFieldSchema, rawValue: string) => {
  if (field.id === 'mcp-server-inherit-parent-env') {
    if (rawValue === '') return null;
    return rawValue === 'true';
  }
  return rawValue;
};

export const normalizeFieldValueForDraft = (
  field: SettingsFieldSchema,
  value: unknown,
  secretStatus?: SecretStatus
) => {
  if (field.control === 'tags') {
    return asStringArray(value).join(', ');
  }

  if (field.control === 'key_value' || field.control === 'json') {
    return JSON.stringify(value ?? (field.control === 'json' ? {} : {}), null, 2);
  }

  if (field.control === 'secret') {
    return secretStatus?.configured ? '' : formatPrimitive(value);
  }

  if (field.control === 'select') {
    if (value === null || value === undefined) {
      return '';
    }
    if (typeof value === 'boolean') {
      return value ? 'true' : 'false';
    }
    return formatPrimitive(value);
  }

  return formatPrimitive(value);
};

export const isFullWidthControl = (control: string) => {
  return ['textarea', 'tags', 'key_value', 'json'].includes(control);
};

export const mapToKeyValueEntries = (value: unknown): KeyValueEntry[] => {
  const record = asRecord(value);
  return Object.entries(record).map(([key, entryValue]) => ({
    id: nextKeyValueEntryId(),
    key,
    value: `${entryValue ?? ''}`,
  }));
};

export const buildMapFromEntries = (entries: KeyValueEntry[]) => {
  const result: Record<string, string> = {};
  const seen = new Set<string>();

  for (const entry of entries) {
    const key = entry.key.trim();
    const value = entry.value;

    if (!key && !value.trim()) {
      continue;
    }

    if (!key) {
      return { error: '键名不能为空' };
    }

    if (seen.has(key)) {
      return { error: `键名重复: ${key}` };
    }

    seen.add(key);
    result[key] = value;
  }

  return { map: result };
};

export const parseDotenvText = (raw: string) => {
  const entries: Array<{ key: string; value: string }> = [];
  const errors: string[] = [];

  const lines = raw.split(/\r?\n/);
  lines.forEach((line, index) => {
    const lineNo = index + 1;
    const trimmed = line.trim();

    if (!trimmed || trimmed.startsWith('#')) {
      return;
    }

    const exportPrefix = /^export\s+/;
    const body = trimmed.replace(exportPrefix, '');
    const equalIndex = body.indexOf('=');
    if (equalIndex <= 0) {
      errors.push(`Line ${lineNo}: missing '='`);
      return;
    }

    const key = body.slice(0, equalIndex).trim();
    if (!key) {
      errors.push(`Line ${lineNo}: empty key`);
      return;
    }

    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) {
      errors.push(`Line ${lineNo}: invalid key '${key}'`);
      return;
    }

    let value = body.slice(equalIndex + 1).trim();
    if (
      (value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'"))
    ) {
      value = value.slice(1, -1);
    }

    entries.push({ key, value });
  });

  return { entries, errors };
};
