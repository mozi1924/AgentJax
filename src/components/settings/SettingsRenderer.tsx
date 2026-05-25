import { useEffect, useMemo, useState } from 'react';
import { ChevronDown, ChevronRight, Plus, Trash2 } from 'lucide-react';
import type {
  SecretStatus,
  SettingsCollectionSchema,
  SettingsFieldSchema,
  SettingsSchemaNode,
  SettingsSectionSchema,
  SettingsSnapshot,
} from '../../features/settings/types';
import {
  asRecord,
  asStringArray,
  getCollectionItems,
  getFieldOptions,
  getValueAtPath,
  isNodeEnabled,
  isNodeVisible,
  resolvePath,
  validateFieldValue,
} from '../../features/settings/utils';

interface SettingsRendererProps {
  section: SettingsSectionSchema;
  snapshot: SettingsSnapshot;
  savingPath: string | null;
  fieldErrors: Record<string, string>;
  onSaveField: (path: string, value: unknown) => Promise<void>;
  onDeletePath: (path: string) => Promise<void>;
  onAddCollectionItem: (path: string, key: string, value: Record<string, unknown>) => Promise<void>;
}

interface NodeListProps extends Omit<SettingsRendererProps, 'section'> {
  nodes: SettingsSchemaNode[];
  contextPath?: string;
}

const formatPrimitive = (value: unknown) => {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return `${value}`;
  return '';
};

const createDefaultItem = (collection: SettingsCollectionSchema, key: string) => {
  const next = JSON.parse(JSON.stringify(collection.defaultItem)) as Record<string, unknown>;

  if (collection.path === 'providers') {
    next.credential_env = `${key.replace(/[^A-Za-z0-9]/g, '_').toUpperCase()}_API_KEY`;
  }

  if (collection.path === 'models' && typeof next.model === 'string' && !next.model) {
    next.model = key;
  }

  return next;
};

const coerceSelectValue = (field: SettingsFieldSchema, rawValue: string) => {
  if (field.id === 'mcp-server-inherit-parent-env') {
    if (rawValue === '') return null;
    return rawValue === 'true';
  }
  return rawValue;
};

const normalizeFieldValueForDraft = (
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

function FieldRow({
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
  const resolvedPath = resolvePath(field.path, contextPath);
  const value = getValueAtPath(snapshot.values, resolvedPath);
  const secretStatus = snapshot.secretStatuses[resolvedPath];
  const [draft, setDraft] = useState(normalizeFieldValueForDraft(field, value, secretStatus));
  const [isDirty, setIsDirty] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);
  const isSaving = savingPath === resolvedPath;
  const disabled = !isNodeEnabled(field, snapshot, contextPath) || isSaving;
  const options = getFieldOptions(field, snapshot);

  useEffect(() => {
    setDraft(normalizeFieldValueForDraft(field, value, secretStatus));
    setIsDirty(false);
    setLocalError(null);
  }, [field, value, secretStatus, snapshot.revision]);

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

  const helperText = fieldErrors[resolvedPath] || localError || field.helpText;

  return (
    <div className="border-b border-[#2b2b2d] py-4 first:pt-0">
      <div className="flex items-start justify-between gap-6">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <h4 className="font-medium text-slate-100">{field.title}</h4>
            {field.advanced && (
              <span className="rounded-full border border-slate-700 px-2 py-0.5 text-[10px] uppercase tracking-[0.18em] text-slate-500">
                Advanced
              </span>
            )}
          </div>
          {field.description && <p className="mt-1 text-sm leading-6 text-slate-400">{field.description}</p>}
          {field.control === 'secret' && secretStatus && (
            <p className="mt-2 text-xs text-slate-500">
              {secretStatus.configured
                ? `Current secret stored via ${secretStatus.source}. Leave blank to keep it unchanged.`
                : 'No secret configured yet.'}
            </p>
          )}
          {field.warningText && <p className="mt-2 text-xs text-amber-300/90">{field.warningText}</p>}
          {helperText && (
            <p className={`mt-2 text-xs ${fieldErrors[resolvedPath] || localError ? 'text-rose-300' : 'text-slate-500'}`}>
              {helperText}
            </p>
          )}
        </div>

        <div className="w-[320px] max-w-[45%] shrink-0">
          {field.control === 'switch' && (
            <label className="inline-flex cursor-pointer items-center justify-end gap-3">
              <span className="text-xs text-slate-500">{value ? 'On' : 'Off'}</span>
              <span className="relative inline-flex h-7 w-12 items-center">
                <input
                  type="checkbox"
                  checked={!!value}
                  disabled={disabled}
                  onChange={(event) => {
                    void onSaveField(resolvedPath, event.target.checked);
                  }}
                  className="peer sr-only"
                />
                <span className="absolute inset-0 rounded-full bg-[#3b3b3f] transition peer-checked:bg-cyan-500 peer-disabled:opacity-50" />
                <span className="absolute left-1 top-1 h-5 w-5 rounded-full bg-white transition peer-checked:translate-x-5" />
              </span>
            </label>
          )}

          {field.control === 'select' && (
            <select
              value={draft}
              disabled={disabled}
              onChange={(event) => {
                const rawValue = event.target.value;
                setDraft(rawValue);
                setIsDirty(true);
                void onSaveField(resolvedPath, coerceSelectValue(field, rawValue));
              }}
              className="w-full rounded-xl border border-[#343437] bg-[#191a1c] px-3 py-2.5 text-sm text-slate-100 outline-none transition focus:border-cyan-400/60 disabled:opacity-50"
            >
              {options.map((option) => (
                <option key={`${field.id}-${option.value}`} value={option.value}>
                  {option.label}
                </option>
              ))}
            </select>
          )}

          {(field.control === 'text' || field.control === 'secret') && (
            <input
              type={field.control === 'secret' ? 'password' : 'text'}
              value={draft}
              placeholder={field.placeholder}
              disabled={disabled}
              onChange={(event) => {
                setDraft(event.target.value);
                setIsDirty(true);
              }}
              onBlur={() => {
                void commit(draft);
              }}
              onKeyDown={(event) => {
                if (event.key === 'Enter') {
                  event.preventDefault();
                  (event.target as HTMLInputElement).blur();
                }
              }}
              className="w-full rounded-xl border border-[#343437] bg-[#191a1c] px-3 py-2.5 text-sm text-slate-100 outline-none transition focus:border-cyan-400/60 disabled:opacity-50"
            />
          )}

          {field.control === 'textarea' && (
            <textarea
              rows={field.rows || 4}
              value={draft}
              placeholder={field.placeholder}
              disabled={disabled}
              onChange={(event) => {
                setDraft(event.target.value);
                setIsDirty(true);
              }}
              onBlur={() => {
                void commit(draft);
              }}
              className="w-full rounded-2xl border border-[#343437] bg-[#191a1c] px-3 py-2.5 text-sm text-slate-100 outline-none transition focus:border-cyan-400/60 disabled:opacity-50"
            />
          )}

          {field.control === 'number' && (
            <input
              type="number"
              value={draft}
              placeholder={field.placeholder}
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
                  draft === '' ? null : field.valueType === 'integer' ? Number.parseInt(draft, 10) : Number(draft);
                void commit(nextValue);
              }}
              className="w-full rounded-xl border border-[#343437] bg-[#191a1c] px-3 py-2.5 text-sm text-slate-100 outline-none transition focus:border-cyan-400/60 disabled:opacity-50"
            />
          )}

          {field.control === 'tags' && (
            <textarea
              rows={3}
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
              className="w-full rounded-2xl border border-[#343437] bg-[#191a1c] px-3 py-2.5 text-sm text-slate-100 outline-none transition focus:border-cyan-400/60 disabled:opacity-50"
            />
          )}

          {field.control === 'key_value' && (
            <textarea
              rows={5}
              value={draft}
              placeholder={"{\n  \"KEY\": \"value\"\n}"}
              disabled={disabled}
              onChange={(event) => {
                setDraft(event.target.value);
                setIsDirty(true);
              }}
              onBlur={() => {
                try {
                  const parsed = draft.trim() ? JSON.parse(draft) : {};
                  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
                    setLocalError('请输入合法的 JSON 对象');
                    return;
                  }
                  void commit(parsed);
                } catch {
                  setLocalError('请输入合法的 JSON 对象');
                }
              }}
              className="w-full rounded-2xl border border-[#343437] bg-[#191a1c] px-3 py-2.5 font-mono text-sm text-slate-100 outline-none transition focus:border-cyan-400/60 disabled:opacity-50"
            />
          )}

          {field.control === 'json' && (
            <textarea
              rows={6}
              value={draft}
              placeholder={"{\n  \"custom\": true\n}"}
              disabled={disabled}
              onChange={(event) => {
                setDraft(event.target.value);
                setIsDirty(true);
              }}
              onBlur={() => {
                try {
                  const parsed = draft.trim() ? JSON.parse(draft) : {};
                  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
                    setLocalError('请输入合法的 JSON 对象');
                    return;
                  }
                  void commit(parsed);
                } catch {
                  setLocalError('请输入合法的 JSON 对象');
                }
              }}
              className="w-full rounded-2xl border border-[#343437] bg-[#191a1c] px-3 py-2.5 font-mono text-sm text-slate-100 outline-none transition focus:border-cyan-400/60 disabled:opacity-50"
            />
          )}
        </div>
      </div>
    </div>
  );
}

function CollectionEditor({
  collection,
  snapshot,
  savingPath,
  fieldErrors,
  contextPath,
  onSaveField,
  onDeletePath,
  onAddCollectionItem,
}: {
  collection: SettingsCollectionSchema;
  snapshot: SettingsSnapshot;
  savingPath: string | null;
  fieldErrors: Record<string, string>;
  contextPath?: string;
  onSaveField: (path: string, value: unknown) => Promise<void>;
  onDeletePath: (path: string) => Promise<void>;
  onAddCollectionItem: (path: string, key: string, value: Record<string, unknown>) => Promise<void>;
}) {
  const resolvedPath = resolvePath(collection.path, contextPath);
  const items = getCollectionItems(collection, snapshot, contextPath);
  const [expandedKeys, setExpandedKeys] = useState<Record<string, boolean>>({});
  const [adding, setAdding] = useState(false);
  const [newKey, setNewKey] = useState('');
  const [newKeyError, setNewKeyError] = useState('');

  useEffect(() => {
    if (items.length === 1) {
      setExpandedKeys({ [items[0][0]]: true });
      return;
    }

    setExpandedKeys((current) => {
      const next: Record<string, boolean> = {};
      items.forEach(([key], index) => {
        next[key] = current[key] ?? index === 0;
      });
      return next;
    });
  }, [items]);

  return (
    <div className="space-y-4">
      <div className="flex items-end justify-between gap-4">
        <div>
          <h4 className="font-medium text-slate-100">{collection.title}</h4>
          {collection.description && <p className="mt-1 text-sm text-slate-400">{collection.description}</p>}
        </div>
        <button
          onClick={() => setAdding((current) => !current)}
          className="inline-flex items-center gap-2 rounded-xl border border-cyan-400/20 bg-cyan-400/10 px-3 py-2 text-sm font-medium text-cyan-200 transition hover:bg-cyan-400/15"
        >
          <Plus className="h-4 w-4" />
          {collection.addLabel}
        </button>
      </div>

      {adding && (
        <div className="rounded-2xl border border-[#2d2f31] bg-[#17181a] p-4">
          <div className="flex items-center gap-3">
            <input
              value={newKey}
              onChange={(event) => {
                setNewKey(event.target.value);
                setNewKeyError('');
              }}
              placeholder={collection.keyLabel}
              className="flex-1 rounded-xl border border-[#343437] bg-[#191a1c] px-3 py-2.5 text-sm text-slate-100 outline-none transition focus:border-cyan-400/60"
            />
            <button
              onClick={() => {
                const candidate = newKey.trim();
                const pattern = collection.keyPattern ? new RegExp(collection.keyPattern) : null;
                if (!candidate) {
                  setNewKeyError('请输入一个 key');
                  return;
                }
                if (pattern && !pattern.test(candidate)) {
                  setNewKeyError('key 格式不合法');
                  return;
                }
                if (items.some(([itemKey]) => itemKey === candidate)) {
                  setNewKeyError('这个 key 已经存在');
                  return;
                }
                void onAddCollectionItem(resolvedPath, candidate, createDefaultItem(collection, candidate)).then(() => {
                  setExpandedKeys((current) => ({ ...current, [candidate]: true }));
                  setAdding(false);
                  setNewKey('');
                });
              }}
              className="rounded-xl bg-slate-100 px-4 py-2.5 text-sm font-medium text-slate-900 transition hover:bg-white"
            >
              Create
            </button>
          </div>
          {newKeyError && <p className="mt-2 text-xs text-rose-300">{newKeyError}</p>}
        </div>
      )}

      <div className="space-y-3">
        {items.length === 0 && (
          <div className="rounded-2xl border border-dashed border-[#2d2f31] px-4 py-8 text-center text-sm text-slate-500">
            No items configured yet.
          </div>
        )}

        {items.map(([itemKey, itemValue]) => {
          const itemPath = `${resolvedPath}.${itemKey}`;
          const itemRecord = asRecord(itemValue);
          const subtitle =
            typeof itemRecord.model === 'string' && itemRecord.model
              ? itemRecord.model
              : typeof itemRecord.command === 'string' && itemRecord.command
                ? itemRecord.command
                : typeof itemRecord.uri === 'string' && itemRecord.uri
                  ? itemRecord.uri
                  : '';
          const isExpanded = !!expandedKeys[itemKey];

          return (
            <div key={itemPath} className="overflow-hidden rounded-[24px] border border-[#2d2f31] bg-[#17181a]">
              <div className="flex items-center justify-between gap-3 px-4 py-3.5">
                <button
                  onClick={() => setExpandedKeys((current) => ({ ...current, [itemKey]: !current[itemKey] }))}
                  className="flex min-w-0 flex-1 items-center gap-3 text-left"
                >
                  <span className="flex h-8 w-8 items-center justify-center rounded-xl bg-[#232427] text-slate-300">
                    {isExpanded ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm font-semibold text-slate-100">{itemKey}</span>
                    {subtitle && <span className="mt-0.5 block truncate text-xs text-slate-500">{subtitle}</span>}
                  </span>
                </button>
                <button
                  onClick={() => {
                    void onDeletePath(itemPath);
                  }}
                  className="rounded-xl p-2 text-slate-500 transition hover:bg-rose-500/10 hover:text-rose-300"
                  title={`Delete ${collection.itemLabel}`}
                >
                  <Trash2 className="h-4 w-4" />
                </button>
              </div>

              {isExpanded && (
                <div className="border-t border-[#26272a] px-4 py-4">
                  <NodeList
                    nodes={collection.children}
                    snapshot={snapshot}
                    savingPath={savingPath}
                    fieldErrors={fieldErrors}
                    contextPath={itemPath}
                    onSaveField={onSaveField}
                    onDeletePath={onDeletePath}
                    onAddCollectionItem={onAddCollectionItem}
                  />
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function NodeList({
  nodes,
  snapshot,
  savingPath,
  fieldErrors,
  contextPath,
  onSaveField,
  onDeletePath,
  onAddCollectionItem,
}: NodeListProps) {
  const visibleNodes = useMemo(
    () => nodes.filter((node) => isNodeVisible(node, snapshot, contextPath)),
    [contextPath, nodes, snapshot]
  );

  return (
    <div className="space-y-5">
      {visibleNodes.map((node) => {
        if (node.kind === 'field') {
          return (
            <FieldRow
              key={`${contextPath || 'root'}:${node.id}`}
              field={node}
              snapshot={snapshot}
              savingPath={savingPath}
              fieldErrors={fieldErrors}
              contextPath={contextPath}
              onSaveField={onSaveField}
            />
          );
        }

        if (node.kind === 'group') {
          return (
            <section key={`${contextPath || 'root'}:${node.id}`} className="rounded-[24px] border border-[#2d2f31] bg-[#17181a] px-4 py-4">
              <div className="mb-3">
                <h3 className="font-medium text-slate-100">{node.title}</h3>
                {node.description && <p className="mt-1 text-sm text-slate-400">{node.description}</p>}
              </div>
              <NodeList
                nodes={node.children}
                snapshot={snapshot}
                savingPath={savingPath}
                fieldErrors={fieldErrors}
                contextPath={contextPath}
                onSaveField={onSaveField}
                onDeletePath={onDeletePath}
                onAddCollectionItem={onAddCollectionItem}
              />
            </section>
          );
        }

        return (
          <section key={`${contextPath || 'root'}:${node.id}`} className="rounded-[24px] border border-[#2d2f31] bg-[#17181a] px-4 py-4">
            <CollectionEditor
              collection={node}
              snapshot={snapshot}
              savingPath={savingPath}
              fieldErrors={fieldErrors}
              contextPath={contextPath}
              onSaveField={onSaveField}
              onDeletePath={onDeletePath}
              onAddCollectionItem={onAddCollectionItem}
            />
          </section>
        );
      })}
    </div>
  );
}

export default function SettingsRenderer(props: SettingsRendererProps) {
  return (
    <NodeList
      nodes={props.section.children}
      snapshot={props.snapshot}
      savingPath={props.savingPath}
      fieldErrors={props.fieldErrors}
      onSaveField={props.onSaveField}
      onDeletePath={props.onDeletePath}
      onAddCollectionItem={props.onAddCollectionItem}
    />
  );
}
