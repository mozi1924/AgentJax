import { useEffect, useMemo, useState } from 'react';
import {
  closestCenter,
  DndContext,
  PointerSensor,
  useSensor,
  useSensors,
  type DragEndEvent,
} from '@dnd-kit/core';
import {
  arrayMove,
  SortableContext,
  useSortable,
  verticalListSortingStrategy,
} from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import {
  Eye,
  GripVertical,
  Layers2,
  Plus,
  Shield,
  Sparkles,
  Trash2,
  Wrench,
} from 'lucide-react';
import {
  compilePromptComposerPreview,
  createPromptBlock,
  normalizePromptComposer,
  type PromptBlock,
  type PromptBlockRole,
  type PromptComposerConfig,
} from '../../../features/settings/promptComposer';
import { getValueAtPath, resolvePath } from '../../../features/settings/utils';
import type { FieldRendererProps } from './types';

interface SortableBlockItemProps {
  block: PromptBlock;
  selected: boolean;
  onSelect: (id: string) => void;
}

function SortableBlockItem({ block, selected, onSelect }: SortableBlockItemProps) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: block.id,
  });

  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
  };

  return (
    <button
      ref={setNodeRef}
      style={style}
      onClick={() => onSelect(block.id)}
      className={`group flex w-full items-start gap-2 rounded-xl border px-2.5 py-2 text-left transition ${
        selected
          ? 'border-cyan-400/30 bg-cyan-400/10 text-white'
          : 'border-[#2b2b2d] bg-[#1a1b1d]/60 text-neutral-200 hover:border-[#3b3b3f]'
      } ${isDragging ? 'opacity-80 shadow-lg' : ''}`}
    >
      <span
        {...attributes}
        {...listeners}
        className="mt-0.5 inline-flex cursor-grab rounded-md p-1 text-neutral-500 transition hover:bg-[#2b2b2d] hover:text-neutral-200 active:cursor-grabbing"
        title="Drag to reorder"
      >
        <GripVertical className="h-3.5 w-3.5" />
      </span>
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-1.5">
          <span className="truncate text-[12px] font-medium">{block.title}</span>
          {!block.enabled && (
            <span className="rounded-full border border-[#3a3a3d] px-1.5 py-0.5 text-[9px] uppercase tracking-wide text-neutral-500">
              Off
            </span>
          )}
        </span>
        <span className="mt-1 flex items-center gap-1.5 text-[10px] text-neutral-500">
          {block.source === 'builtin' && <Shield className="h-3 w-3" />}
          {block.source === 'plugin' && <Wrench className="h-3 w-3" />}
          {block.source === 'user' && <Sparkles className="h-3 w-3" />}
          <span className="uppercase tracking-wide">{block.source}</span>
        </span>
      </span>
    </button>
  );
}

function PromptPreviewModal({
  markdown,
  onClose,
}: {
  markdown: string;
  onClose: () => void;
}) {
  return (
    <div className="absolute inset-0 z-20 flex items-center justify-center bg-black/70 p-5 backdrop-blur-sm">
      <div className="flex h-[min(70vh,720px)] w-[min(760px,100%)] flex-col overflow-hidden rounded-2xl border border-[#2b2b2d] bg-[#131314] shadow-2xl shadow-black/70">
        <div className="flex items-center justify-between border-b border-[#242426] px-4 py-3">
          <div>
            <h4 className="text-sm font-semibold text-white">Compiled prompt preview</h4>
            <p className="mt-0.5 text-[11px] text-neutral-500">
              This is the Markdown-shaped preview of the final prompt assembly.
            </p>
          </div>
          <button
            onClick={onClose}
            className="rounded-lg border border-[#2b2b2d] px-2.5 py-1 text-xs text-neutral-300 transition hover:bg-[#202022]"
          >
            Close
          </button>
        </div>
        <div className="scrollbar-thin flex-1 overflow-auto px-4 py-4">
          <pre className="whitespace-pre-wrap font-mono text-[12px] leading-6 text-neutral-200">
            {markdown}
          </pre>
        </div>
      </div>
    </div>
  );
}

const laneIcon = (role: PromptBlockRole) =>
  role === 'system' ? <Shield className="h-3.5 w-3.5" /> : <Layers2 className="h-3.5 w-3.5" />;

const laneLabel = (role: PromptBlockRole) =>
  role === 'system' ? 'System / instructions' : 'Developer messages';

const canDeleteBlock = (block: PromptBlock) => block.source === 'user' && !block.locked;

export function PromptAssemblerField({
  field,
  snapshot,
  savingPath,
  fieldErrors,
  contextPath,
  onSaveField,
}: FieldRendererProps) {
  const resolvedPath = resolvePath(field.path, contextPath);
  const value = getValueAtPath(snapshot.values, resolvedPath);
  const [composer, setComposer] = useState<PromptComposerConfig>(() =>
    normalizePromptComposer(value)
  );
  const [selectedBlockId, setSelectedBlockId] = useState<string>('');
  const [previewOpen, setPreviewOpen] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);
  const isSaving = savingPath === resolvedPath;
  const sensors = useSensors(
    useSensor(PointerSensor, {
      activationConstraint: { distance: 4 },
    })
  );

  useEffect(() => {
    const nextComposer = normalizePromptComposer(value);
    setComposer(nextComposer);
    setSelectedBlockId((current) => {
      if (current && nextComposer.blocks.some((block) => block.id === current)) {
        return current;
      }
      return nextComposer.blocks[0]?.id || '';
    });
    setLocalError(null);
  }, [snapshot.revision, value]);

  const selectedBlock =
    composer.blocks.find((block) => block.id === selectedBlockId) || composer.blocks[0] || null;

  const preview = useMemo(() => compilePromptComposerPreview(composer), [composer]);
  const helperText = fieldErrors[resolvedPath] || localError || field.helpText;
  const systemBlocks = composer.blocks.filter((block) => block.role === 'system');
  const developerBlocks = composer.blocks.filter((block) => block.role === 'developer');

  const persistComposer = async (nextComposer: PromptComposerConfig) => {
    setComposer(nextComposer);
    setLocalError(null);
    await onSaveField(resolvedPath, nextComposer);
  };

  const updateBlock = (blockId: string, updates: Partial<PromptBlock>) => {
    setComposer((current) => ({
      blocks: current.blocks.map((block) =>
        block.id === blockId ? { ...block, ...updates } : block
      ),
    }));
  };

  const saveCurrentComposer = async () => {
    const normalized = normalizePromptComposer(composer);
    await persistComposer(normalized);
  };

  const handleAddBlock = async (role: PromptBlockRole) => {
    const newBlock = createPromptBlock(role);
    const nextComposer = normalizePromptComposer({
      blocks: [...composer.blocks, newBlock],
    });
    setSelectedBlockId(newBlock.id);
    await persistComposer(nextComposer);
  };

  const handleDeleteBlock = async (blockId: string) => {
    const block = composer.blocks.find((entry) => entry.id === blockId);
    if (!block || !canDeleteBlock(block)) {
      return;
    }

    const nextComposer = normalizePromptComposer({
      blocks: composer.blocks.filter((entry) => entry.id !== blockId),
    });
    setSelectedBlockId(nextComposer.blocks[0]?.id || '');
    await persistComposer(nextComposer);
  };

  const handleToggleEnabled = async (blockId: string, enabled: boolean) => {
    const nextComposer = normalizePromptComposer({
      blocks: composer.blocks.map((block) => (block.id === blockId ? { ...block, enabled } : block)),
    });
    await persistComposer(nextComposer);
  };

  const handleRoleChange = async (blockId: string, role: PromptBlockRole) => {
    const block = composer.blocks.find((entry) => entry.id === blockId);
    if (!block || block.locked) {
      return;
    }

    const nextComposer = normalizePromptComposer({
      blocks: composer.blocks.map((entry) => (entry.id === blockId ? { ...entry, role } : entry)),
    });
    await persistComposer(nextComposer);
  };

  const handleDragEnd = async (event: DragEndEvent, role: PromptBlockRole) => {
    const { active, over } = event;
    if (!over || active.id === over.id) {
      return;
    }

    const laneBlocks = composer.blocks.filter((block) => block.role === role);
    const oldIndex = laneBlocks.findIndex((block) => block.id === active.id);
    const newIndex = laneBlocks.findIndex((block) => block.id === over.id);
    if (oldIndex === -1 || newIndex === -1) {
      return;
    }

    const movedLane = arrayMove(laneBlocks, oldIndex, newIndex);
    const otherLane = composer.blocks.filter((block) => block.role !== role);
    const nextComposer =
      role === 'system'
        ? { blocks: [...movedLane, ...otherLane] }
        : { blocks: [...otherLane, ...movedLane] };
    await persistComposer(nextComposer);
  };

  return (
    <div className="relative space-y-3">
      <div className="flex items-center justify-between gap-3">
        <div>
          <h4 className="text-[13.5px] font-medium text-neutral-200">{field.title}</h4>
          {field.description && (
            <p className="mt-0.5 text-[11.5px] leading-relaxed text-neutral-400/80">
              {field.description}
            </p>
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
        <div className="flex items-center gap-2">
          <button
            type="button"
            disabled={isSaving}
            onClick={() => {
              void handleAddBlock('system');
            }}
            className="inline-flex items-center gap-1.5 rounded-lg border border-[#2b2b2d] bg-[#202022] px-2.5 py-1.5 text-[11px] text-neutral-200 transition hover:bg-[#2a2a2d] disabled:opacity-50"
          >
            <Plus className="h-3.5 w-3.5" />
            Add system
          </button>
          <button
            type="button"
            disabled={isSaving}
            onClick={() => {
              void handleAddBlock('developer');
            }}
            className="inline-flex items-center gap-1.5 rounded-lg border border-[#2b2b2d] bg-[#202022] px-2.5 py-1.5 text-[11px] text-neutral-200 transition hover:bg-[#2a2a2d] disabled:opacity-50"
          >
            <Plus className="h-3.5 w-3.5" />
            Add developer
          </button>
          <button
            type="button"
            onClick={() => setPreviewOpen(true)}
            className="inline-flex items-center gap-1.5 rounded-lg bg-neutral-200 px-2.5 py-1.5 text-[11px] font-medium text-neutral-900 transition hover:bg-white"
          >
            <Eye className="h-3.5 w-3.5" />
            Preview
          </button>
        </div>
      </div>

      <div className="grid min-h-[420px] grid-cols-[240px_minmax(0,1fr)_220px] gap-3 rounded-2xl border border-[#242426] bg-[#171718]/70 p-3">
        <div className="space-y-3 rounded-2xl border border-[#242426] bg-[#141415]/80 p-3">
          {(['system', 'developer'] as const).map((role) => {
            const blocks = role === 'system' ? systemBlocks : developerBlocks;
            return (
              <div key={role} className="space-y-2">
                <div className="flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-[0.16em] text-neutral-500">
                  {laneIcon(role)}
                  <span>{laneLabel(role)}</span>
                </div>
                <DndContext
                  sensors={sensors}
                  collisionDetection={closestCenter}
                  onDragEnd={(event) => {
                    void handleDragEnd(event, role);
                  }}
                >
                  <SortableContext
                    items={blocks.map((block) => block.id)}
                    strategy={verticalListSortingStrategy}
                  >
                    <div className="space-y-2">
                      {blocks.length === 0 && (
                        <div className="rounded-xl border border-dashed border-[#2b2b2d] px-3 py-4 text-[11px] text-neutral-500">
                          No {role} blocks yet.
                        </div>
                      )}
                      {blocks.map((block) => (
                        <SortableBlockItem
                          key={block.id}
                          block={block}
                          selected={block.id === selectedBlock?.id}
                          onSelect={setSelectedBlockId}
                        />
                      ))}
                    </div>
                  </SortableContext>
                </DndContext>
              </div>
            );
          })}
        </div>

        <div className="flex min-w-0 flex-col rounded-2xl border border-[#242426] bg-[#141415]/70">
          {selectedBlock ? (
            <>
              <div className="border-b border-[#242426] px-4 py-3">
                <div className="flex items-center justify-between gap-3">
                  <input
                    value={selectedBlock.title}
                    disabled={selectedBlock.locked || isSaving}
                    onChange={(event) => updateBlock(selectedBlock.id, { title: event.target.value })}
                    onBlur={() => {
                      void saveCurrentComposer();
                    }}
                    className="min-w-0 flex-1 rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/50 px-3 py-2 text-sm font-medium text-white outline-none transition focus:border-neutral-500 disabled:opacity-60"
                  />
                  <span className="rounded-full border border-[#2b2b2d] px-2 py-1 text-[10px] uppercase tracking-[0.16em] text-neutral-500">
                    {selectedBlock.role}
                  </span>
                </div>
              </div>
              <div className="flex-1 p-4">
                <textarea
                  value={selectedBlock.content}
                  disabled={selectedBlock.locked || isSaving}
                  onChange={(event) => updateBlock(selectedBlock.id, { content: event.target.value })}
                  onBlur={() => {
                    void saveCurrentComposer();
                  }}
                  placeholder={
                    selectedBlock.role === 'system'
                      ? 'Write high-priority instructions here…'
                      : 'Write a developer message block here…'
                  }
                  className="h-full min-h-[260px] w-full resize-none rounded-xl border border-[#2b2b2d] bg-[#101011] px-3 py-3 font-mono text-[12px] leading-6 text-neutral-200 outline-none transition focus:border-neutral-500 disabled:opacity-60"
                />
              </div>
            </>
          ) : (
            <div className="flex h-full items-center justify-center text-sm text-neutral-500">
              Select a block to edit it.
            </div>
          )}
        </div>

        <div className="space-y-3 rounded-2xl border border-[#242426] bg-[#141415]/80 p-3">
          {selectedBlock ? (
            <>
              <div>
                <p className="text-[11px] font-semibold uppercase tracking-[0.16em] text-neutral-500">
                  Block metadata
                </p>
                <div className="mt-2 rounded-xl border border-[#242426] bg-[#111112]/80 p-3 text-[12px] text-neutral-300">
                  <div className="space-y-2">
                    <label className="block">
                      <span className="mb-1 block text-[10px] uppercase tracking-[0.16em] text-neutral-500">
                        Role
                      </span>
                      <select
                        value={selectedBlock.role}
                        disabled={selectedBlock.locked || isSaving}
                        onChange={(event) => {
                          void handleRoleChange(
                            selectedBlock.id,
                            event.target.value === 'developer' ? 'developer' : 'system'
                          );
                        }}
                        className="w-full rounded-lg border border-[#2b2b2d] bg-[#18191b] px-2.5 py-2 text-[12px] text-neutral-200 outline-none transition focus:border-neutral-500 disabled:opacity-60"
                      >
                        <option value="system">system</option>
                        <option value="developer">developer</option>
                      </select>
                    </label>

                    <div>
                      <span className="mb-1 block text-[10px] uppercase tracking-[0.16em] text-neutral-500">
                        Source
                      </span>
                      <div className="rounded-lg border border-[#2b2b2d] bg-[#18191b] px-2.5 py-2 text-[12px] text-neutral-200">
                        {selectedBlock.source}
                      </div>
                    </div>

                    <div>
                      <span className="mb-1 block text-[10px] uppercase tracking-[0.16em] text-neutral-500">
                        Source ID
                      </span>
                      <div className="rounded-lg border border-[#2b2b2d] bg-[#18191b] px-2.5 py-2 text-[12px] text-neutral-400">
                        {selectedBlock.source_id || 'User-defined'}
                      </div>
                    </div>

                    <label className="flex items-center justify-between rounded-lg border border-[#2b2b2d] bg-[#18191b] px-2.5 py-2">
                      <span className="text-[12px] text-neutral-200">Enabled</span>
                      <input
                        type="checkbox"
                        checked={selectedBlock.enabled}
                        disabled={isSaving}
                        onChange={(event) => {
                          void handleToggleEnabled(selectedBlock.id, event.target.checked);
                        }}
                        className="h-4 w-4 rounded border-[#3c3c40] bg-[#111112] text-cyan-400 focus:ring-cyan-400/40"
                      />
                    </label>

                    <div className="rounded-lg border border-[#2b2b2d] bg-[#18191b] px-2.5 py-2 text-[11px] leading-5 text-neutral-400">
                      {selectedBlock.locked
                        ? 'This block is managed by the framework or a plugin. You can reorder it and toggle it, but not edit its content.'
                        : 'This user block is fully editable and can be removed.'}
                    </div>

                    <button
                      type="button"
                      disabled={!canDeleteBlock(selectedBlock) || isSaving}
                      onClick={() => {
                        void handleDeleteBlock(selectedBlock.id);
                      }}
                      className="inline-flex w-full items-center justify-center gap-1.5 rounded-lg border border-rose-500/20 bg-rose-500/10 px-2.5 py-2 text-[12px] text-rose-200 transition hover:bg-rose-500/15 disabled:opacity-40"
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                      Delete block
                    </button>
                  </div>
                </div>
              </div>

              <div className="rounded-xl border border-[#242426] bg-[#111112]/80 p-3">
                <p className="text-[10px] uppercase tracking-[0.16em] text-neutral-500">
                  Current assembly
                </p>
                <p className="mt-2 text-[11px] leading-5 text-neutral-400">
                  {preview.instructionsText
                    ? `${preview.instructionsText.split(/\s+/).length} instruction words across ${systemBlocks.filter((block) => block.enabled).length} active system blocks.`
                    : 'No active system instructions yet.'}
                </p>
                <p className="mt-1 text-[11px] leading-5 text-neutral-400">
                  {preview.developerMessages.length} active developer message
                  {preview.developerMessages.length === 1 ? '' : 's'}.
                </p>
              </div>
            </>
          ) : (
            <div className="text-[12px] text-neutral-500">Select a block to inspect its metadata.</div>
          )}
        </div>
      </div>

      {previewOpen && (
        <PromptPreviewModal
          markdown={preview.previewMarkdown}
          onClose={() => setPreviewOpen(false)}
        />
      )}
    </div>
  );
}
