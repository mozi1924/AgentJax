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

const isFullWidthControl = (control: string) => {
  return ['textarea', 'tags', 'key_value', 'json'].includes(control);
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
  const fullWidth = isFullWidthControl(field.control);

  return (
    <div className="border-b border-[#242426]/30 py-3 first:pt-0 last:border-b-0">
      <div className={`flex ${fullWidth ? 'flex-col gap-2' : 'flex-row items-center justify-between gap-4'}`}>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5 flex-wrap">
            <h4 className="text-[13.5px] font-medium text-neutral-200">{field.title}</h4>
            {field.advanced && (
              <span className="rounded px-1.5 py-0.5 text-[9px] font-semibold bg-[#2e2e30] text-neutral-400 uppercase tracking-wider">
                Advanced
              </span>
            )}
          </div>
          {field.description && (
            <p className="mt-0.5 text-[11.5px] leading-relaxed text-neutral-400/80 max-w-[95%]">
              {field.description}
            </p>
          )}
          {field.control === 'secret' && secretStatus && (
            <p className="mt-1 text-[11px] text-neutral-500">
              {secretStatus.configured
                ? `Current secret stored via ${secretStatus.source}. Leave blank to keep it unchanged.`
                : 'No secret configured yet.'}
            </p>
          )}
          {field.warningText && <p className="mt-1 text-[11px] text-amber-500/80">{field.warningText}</p>}
          {helperText && (
            <p className={`mt-1 text-[11px] ${fieldErrors[resolvedPath] || localError ? 'text-rose-400' : 'text-neutral-500'}`}>
              {helperText}
            </p>
          )}
        </div>

        <div className={fullWidth ? 'w-full mt-1.5' : 'shrink-0 flex items-center justify-end'}>
          {field.control === 'switch' && (
            <span className="relative inline-flex h-5 w-9 items-center cursor-pointer">
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
            </span>
          )}

          {field.control === 'select' && (
            <div className="relative inline-flex items-center">
              {field.id === 'accent-color' && (
                <span 
                  className="w-2.5 h-2.5 rounded-full mr-1 transition-colors shrink-0"
                  style={{
                    backgroundColor: 
                      draft === 'green' ? '#10a37f' :
                      draft === 'blue' ? '#007aff' :
                      draft === 'purple' ? '#a855f7' :
                      draft === 'orange' ? '#f97316' :
                      '#737373'
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
                  <option key={`${field.id}-${option.value}`} value={option.value} className="bg-[#171717] text-neutral-200 text-left">
                    {option.label}
                  </option>
                ))}
              </select>
              <ChevronDown className="absolute right-0.5 h-3.5 w-3.5 pointer-events-none text-neutral-400" />
            </div>
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
              className="w-[180px] text-right rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 px-2.5 py-1 text-xs text-neutral-200 outline-none transition focus:border-neutral-500 focus:bg-[#222326]/40 disabled:opacity-50"
            />
          )}

          {field.control === 'textarea' && (
            <textarea
              rows={field.rows || 3}
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
              className="w-full rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 px-2.5 py-2 text-xs text-neutral-200 outline-none transition focus:border-neutral-500 focus:bg-[#222326]/40 disabled:opacity-50"
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
            <textarea
              rows={4}
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
              className="w-full rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 px-2.5 py-2 font-mono text-xs text-neutral-200 outline-none transition focus:border-neutral-500 focus:bg-[#222326]/40 disabled:opacity-50"
            />
          )}

          {field.control === 'json' && (
            <textarea
              rows={5}
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
              className="w-full rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 px-2.5 py-2 font-mono text-xs text-neutral-200 outline-none transition focus:border-neutral-500 focus:bg-[#222326]/40 disabled:opacity-50"
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
    <div className="space-y-3.5">
      <div className="flex items-end justify-between gap-4">
        <div>
          <h4 className="text-[13px] font-semibold text-neutral-200">{collection.title}</h4>
          {collection.description && <p className="mt-0.5 text-[11px] text-neutral-500">{collection.description}</p>}
        </div>
        <button
          onClick={() => setAdding((current) => !current)}
          className="inline-flex items-center gap-1.5 rounded-lg border border-[#2b2b2d] bg-[#2e2e30]/80 px-2.5 py-1 text-xs text-[#e3e3e3] hover:bg-[#3e3e40] transition"
        >
          <Plus className="h-3.5 w-3.5" />
          {collection.addLabel}
        </button>
      </div>

      {adding && (
        <div className="rounded-xl border border-[#242426] bg-[#1a1b1d]/40 p-3">
          <div className="flex items-center gap-3">
            <input
              value={newKey}
              onChange={(event) => {
                setNewKey(event.target.value);
                setNewKeyError('');
              }}
              placeholder={collection.keyLabel}
              className="flex-1 rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 px-2.5 py-1.5 text-xs text-neutral-200 outline-none transition focus:border-neutral-500"
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
              className="rounded-lg bg-neutral-200 px-3 py-1.5 text-xs font-medium text-neutral-900 transition hover:bg-white"
            >
              Create
            </button>
          </div>
          {newKeyError && <p className="mt-1.5 text-xs text-rose-300">{newKeyError}</p>}
        </div>
      )}

      <div className="space-y-2">
        {items.length === 0 && (
          <div className="rounded-xl border border-dashed border-[#242426] px-4 py-6 text-center text-xs text-neutral-500">
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
            <div key={itemPath} className="overflow-hidden rounded-xl border border-[#242426]/50 bg-[#1c1c1e]/40">
              <div className="flex items-center justify-between gap-2 px-3 py-2">
                <button
                  onClick={() => setExpandedKeys((current) => ({ ...current, [itemKey]: !current[itemKey] }))}
                  className="flex min-w-0 flex-1 items-center gap-2 text-left"
                >
                  <span className="flex h-6 w-6 items-center justify-center rounded-lg bg-[#2e2e30]/30 text-neutral-300">
                    {isExpanded ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-xs font-semibold text-neutral-200">{itemKey}</span>
                    {subtitle && <span className="mt-0.5 block truncate text-[10px] text-neutral-500">{subtitle}</span>}
                  </span>
                </button>
                <button
                  onClick={() => {
                    void onDeletePath(itemPath);
                  }}
                  className="rounded-lg p-1.5 text-neutral-500 transition hover:bg-rose-500/10 hover:text-rose-300"
                  title={`Delete ${collection.itemLabel}`}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </button>
              </div>

              {isExpanded && (
                <div className="border-t border-[#242426]/50 px-3 py-3 bg-[#171717]/30">
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
    <div className="space-y-4">
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
            <section key={`${contextPath || 'root'}:${node.id}`} className="space-y-2.5 pt-2">
              <div className="mt-3 mb-1 first:mt-0">
                <h5 className="text-[10px] font-semibold text-neutral-500 uppercase tracking-wider">{node.title}</h5>
                {node.description && <p className="mt-0.5 text-[11px] text-neutral-400/70">{node.description}</p>}
              </div>
              <div className="border-t border-[#242426]/30 pt-1">
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
              </div>
            </section>
          );
        }

        return (
          <section key={`${contextPath || 'root'}:${node.id}`} className="pt-2">
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
