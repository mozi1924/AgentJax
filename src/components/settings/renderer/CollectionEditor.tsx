import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { ChevronDown, ChevronRight, Plus, Trash2 } from 'lucide-react';
import type { SettingsCollectionSchema, SettingsSnapshot } from '../../../features/settings/types';
import { appendPathSegment, asRecord, getCollectionItems, resolvePath } from '../../../features/settings/utils';
import type { NodeListProps } from './types';
import { createDefaultItem } from './utils';

export function CollectionEditor({
  collection,
  snapshot,
  savingPath,
  fieldErrors,
  contextPath,
  onSaveField,
  onDeletePath,
  onAddCollectionItem,
  renderNodeList,
}: {
  collection: SettingsCollectionSchema;
  snapshot: SettingsSnapshot;
  savingPath: string | null;
  fieldErrors: Record<string, string>;
  contextPath?: string;
  onSaveField: (path: string, value: unknown) => Promise<void>;
  onDeletePath: (path: string) => Promise<void>;
  onAddCollectionItem: (path: string, key: string, value: Record<string, unknown>) => Promise<void>;
  renderNodeList: (props: NodeListProps) => ReactNode;
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
          {collection.description && (
            <p className="mt-0.5 text-[11px] text-neutral-500">{collection.description}</p>
          )}
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
                void onAddCollectionItem(
                  resolvedPath,
                  candidate,
                  createDefaultItem(collection, candidate)
                ).then(() => {
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
          const itemPath = appendPathSegment(resolvedPath, itemKey);
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
            <div
              key={itemPath}
              className="overflow-hidden rounded-xl border border-[#242426]/50 bg-[#1c1c1e]/40"
            >
              <div className="flex items-center justify-between gap-2 px-3 py-2">
                <button
                  onClick={() =>
                    setExpandedKeys((current) => ({ ...current, [itemKey]: !current[itemKey] }))
                  }
                  className="flex min-w-0 flex-1 items-center gap-2 text-left"
                >
                  <span className="flex h-6 w-6 items-center justify-center rounded-lg bg-[#2e2e30]/30 text-neutral-300">
                    {isExpanded ? (
                      <ChevronDown className="h-3.5 w-3.5" />
                    ) : (
                      <ChevronRight className="h-3.5 w-3.5" />
                    )}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-xs font-semibold text-neutral-200">
                      {itemKey}
                    </span>
                    {subtitle && (
                      <span className="mt-0.5 block truncate text-[10px] text-neutral-500">
                        {subtitle}
                      </span>
                    )}
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
                  {renderNodeList({
                    nodes: collection.children,
                    snapshot,
                    savingPath,
                    fieldErrors,
                    contextPath: itemPath,
                    onSaveField,
                    onDeletePath,
                    onAddCollectionItem,
                  })}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
