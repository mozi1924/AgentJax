import type { SettingsFieldSchema } from './types';

export interface DraftKeyValueEntry {
  id: string;
  key: string;
  value: string;
}

// Converts a numeric draft into the schema value that should be committed.
export const parseNumberDraftValue = (field: Pick<SettingsFieldSchema, 'valueType'>, draft: string) => {
  if (draft === '') return null;
  return field.valueType === 'integer' ? Number.parseInt(draft, 10) : Number(draft);
};

// Tags are edited as text but persisted as a normalized string array.
export const parseTagsDraftValue = (draft: string) =>
  draft
    .split(',')
    .map((entry) => entry.trim())
    .filter(Boolean);

// Applies one key/value edit to the latest known draft array, avoiding stale state saves on blur.
export const applyKeyValueEntryPatch = <T extends DraftKeyValueEntry>(
  entries: T[],
  entryId: string,
  patch: Partial<Pick<DraftKeyValueEntry, 'key' | 'value'>>
): T[] => entries.map((item) => (item.id === entryId ? { ...item, ...patch } : item));
