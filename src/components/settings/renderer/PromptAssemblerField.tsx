import { useEffect, useMemo, useState, type CSSProperties } from 'react';
import { useI18n } from '../../../features/i18n';
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
import { renderMarkdown } from '../../chat/markdownRenderer';
import { OverlayScrollArea } from '../../OverlayScrollArea';

interface SortableBlockItemProps {
  block: PromptBlock;
  selected: boolean;
  onSelect: (id: string) => void;
  onToggleEnabled: (id: string, enabled: boolean) => void;
  onDelete: (id: string) => void;
}

function SortableBlockItem({
  block,
  selected,
  onSelect,
  onToggleEnabled,
  onDelete,
}: SortableBlockItemProps) {
  const { t } = useI18n();
  const {
    attributes,
    listeners,
    setActivatorNodeRef,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: block.id });

  // Keep the dragged block visually attached to the pointer. The sortable
  // dependency provides smooth transitions for neighboring blocks; the active
  // block itself should not animate its transform while the cursor is moving.
  const style: CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition: isDragging ? 'none' : transition,
    willChange: isDragging ? 'transform' : undefined,
    zIndex: isDragging ? 10 : undefined,
  };

  const isDeletable = block.source === 'user' && !block.locked;

  return (
    <div
      ref={setNodeRef}
      style={style}
      onClick={() => onSelect(block.id)}
      className={`group relative flex w-full items-center gap-2 rounded-lg border border-transparent px-2.5 py-1.5 text-left transition-colors duration-150 cursor-pointer select-none ${
        selected
          ? 'bg-cyan-500/10 text-cyan-200 font-medium'
          : 'bg-transparent text-neutral-400 hover:bg-[#202022]/60 hover:text-neutral-200'
      } ${isDragging ? 'opacity-85 shadow-md bg-cyan-500/15' : ''}`}
    >
      <span
        ref={setActivatorNodeRef}
        {...attributes}
        {...listeners}
        onClick={(e) => e.stopPropagation()}
        className="inline-flex h-7 w-7 cursor-grab touch-none items-center justify-center rounded-md text-neutral-600 transition-colors hover:bg-[#202022] hover:text-neutral-300 active:cursor-grabbing shrink-0"
        title={t('assembler.drag_hint')}
      >
        <GripVertical className="h-3.5 w-3.5" />
      </span>

      {block.locked ? (
        <span
          className="inline-flex h-3.5 w-3.5 items-center justify-center shrink-0 text-neutral-500"
          title={t('assembler.required_block')}
        >
          <Shield className="h-3 w-3" />
        </span>
      ) : (
        <input
          type="checkbox"
          checked={block.enabled}
          onClick={(e) => e.stopPropagation()}
          onChange={(e) => onToggleEnabled(block.id, e.target.checked)}
          className="h-3.5 w-3.5 rounded border-[#2e2e30] bg-[#111112] text-cyan-400 focus:ring-cyan-400/40 transition cursor-pointer shrink-0"
          title={block.enabled ? t('assembler.disable_block') : t('assembler.enable_block')}
        />
      )}

      <div className="min-w-0 flex-1 pl-1">
        <div className="flex items-center gap-1.5">
          <span
            className={`truncate text-[11.5px] font-medium leading-normal transition ${
              block.enabled ? 'text-neutral-200' : 'text-neutral-500 line-through'
            }`}
          >
            {block.title}
          </span>
        </div>
        <div className="mt-0.5 flex items-center gap-1 text-[9px] text-neutral-500 uppercase tracking-wider leading-none">
          {block.source === 'builtin' && <Shield className="h-2.5 w-2.5" />}
          {block.source === 'plugin' && <Wrench className="h-2.5 w-2.5" />}
          {block.source === 'user' && <Sparkles className="h-2.5 w-2.5" />}
          <span>{block.source}</span>
        </div>
      </div>

      {isDeletable && (
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onDelete(block.id);
          }}
          className="opacity-0 group-hover:opacity-100 p-1 text-neutral-500 hover:text-rose-450 hover:bg-rose-500/10 rounded-md transition shrink-0"
          title={t('assembler.delete_block')}
        >
          <Trash2 className="h-3.5 w-3.5" />
        </button>
      )}
    </div>
  );
}

function PromptPreviewModal({
  markdown,
  onClose,
}: {
  markdown: string;
  onClose: () => void;
}) {
  const { t } = useI18n();
  return (
    <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/70 p-5 backdrop-blur-sm">
      <div className="flex h-[min(70vh,720px)] w-[min(760px,100%)] flex-col overflow-hidden rounded-2xl border border-[#25282d] bg-[#131314] shadow-2xl shadow-black/80">
        <div className="flex items-center justify-between border-b border-[#242426] px-4 py-3 bg-[#17181a]/55">
          <div>
            <h4 className="text-sm font-semibold text-white">{t('assembler.preview.title')}</h4>
            <p className="mt-0.5 text-[11px] text-neutral-500">
              {t('assembler.preview.description')}
            </p>
          </div>
          <button
            onClick={onClose}
            className="rounded-lg border border-[#2b2b2d] px-2.5 py-1 text-xs text-neutral-300 transition hover:bg-[#202022]"
          >
            {t('settings.modal.close')}
          </button>
        </div>
        <OverlayScrollArea
          axis="both"
          containerClassName="flex-1"
          className="h-full bg-[#0c0d0e]/30 px-6 py-5 select-text"
        >
          <div className="prose prose-invert prose-sm max-w-none text-slate-350">
            {renderMarkdown(markdown, undefined, t)}
          </div>
        </OverlayScrollArea>
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
  const { t } = useI18n();
  const laneLabel = (role: PromptBlockRole) => t(`settings.prompt_composer.lane.${role}`);
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
    const block = composer.blocks.find((entry) => entry.id === blockId);
    if (block?.locked) return;
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
    <div className="relative flex-1 flex flex-col min-h-0 space-y-3">
      {(fieldErrors[resolvedPath] || localError) && (
        <div className="rounded-xl border border-rose-500/20 bg-rose-500/10 px-3.5 py-2 text-xs text-rose-200">
          {t(fieldErrors[resolvedPath] || localError || '')}
        </div>
      )}

      <div className="grid flex-1 min-h-[520px] grid-cols-[280px_minmax(0,1fr)] gap-6">
        {/* Left Column: Sidebar with blocks and summary stats */}
        <div className="flex flex-col gap-4 min-w-0">
          <OverlayScrollArea containerClassName="flex-1" className="h-full space-y-4 pr-0.5">
            {(['system', 'developer'] as const).map((role) => {
              const blocks = role === 'system' ? systemBlocks : developerBlocks;
              return (
                <div key={role} className="space-y-2">
                  <div className="flex items-center justify-between gap-2 text-[11px] font-semibold uppercase tracking-[0.14em] text-neutral-500">
                    <div className="flex items-center gap-1.5">
                      {laneIcon(role)}
                      <span>{laneLabel(role)}</span>
                    </div>
                    <button
                      type="button"
                      disabled={isSaving}
                      onClick={() => {
                        void handleAddBlock(role);
                      }}
                      className="inline-flex items-center gap-1 rounded-md border border-[#2b2b2d] bg-[#1d1d1f] hover:bg-[#28282b] px-1.5 py-0.5 text-[10px] text-neutral-350 transition disabled:opacity-50"
                      title={role === 'system' ? t('assembler.enable_block') : t('assembler.add')}
                    >
                      <Plus className="h-3 w-3" />
                      {t('assembler.add')}
                    </button>
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
                          <div className="rounded-xl border border-dashed border-[#242426] bg-neutral-900/10 px-3 py-4 text-center text-[11px] text-neutral-500">
                            {t('assembler.no_blocks', { role })}
                          </div>
                        )}
                        {blocks.map((block) => (
                          <SortableBlockItem
                            key={block.id}
                            block={block}
                            selected={block.id === selectedBlock?.id}
                            onSelect={setSelectedBlockId}
                            onToggleEnabled={handleToggleEnabled}
                            onDelete={handleDeleteBlock}
                          />
                        ))}
                      </div>
                    </SortableContext>
                  </DndContext>
                </div>
              );
            })}
          </OverlayScrollArea>

          {/* Bottom Summary & Stats Panel */}
          <div className="mt-auto border-t border-[#242426]/50 pt-4 space-y-2.5 shrink-0">
            <div className="flex items-center justify-between text-[10px] uppercase tracking-wider text-neutral-500 font-semibold">
              <span>{t('assembler.stats.title')}</span>
              <span className="h-1.5 w-1.5 rounded-full bg-cyan-400 animate-pulse" />
            </div>
            <div className="text-[11px] leading-relaxed text-neutral-400 space-y-1">
              <div className="flex justify-between">
                <span>{t('assembler.stats.active_instructions')}</span>
                <span className="font-mono text-neutral-200">
                  {t('assembler.stats.blocks', { count: String(systemBlocks.filter((b) => b.enabled).length) })}
                </span>
              </div>
              <div className="flex justify-between">
                <span>{t('assembler.stats.developer_messages')}</span>
                <span className="font-mono text-neutral-200">
                  {t('assembler.stats.blocks', { count: String(developerBlocks.filter((b) => b.enabled).length) })}
                </span>
              </div>
              <div className="flex justify-between border-t border-[#242426] pt-1 mt-1 font-medium">
                <span>{t('assembler.stats.estimated_size')}</span>
                <span className="font-mono text-cyan-400">
                  {t('assembler.stats.words', { count: String(preview.instructionsText ? preview.instructionsText.split(/\s+/).filter(Boolean).length : 0) })}
                </span>
              </div>
            </div>
            <button
              type="button"
              onClick={() => setPreviewOpen(true)}
              className="mt-2 inline-flex w-full items-center justify-center gap-1.5 rounded-lg border border-[#2b2b2d] bg-[#1d1d1f] hover:bg-[#28282b] hover:text-white px-2.5 py-1.5 text-[11px] font-medium text-neutral-350 transition"
            >
              <Eye className="h-3.5 w-3.5" />
              {t('assembler.preview_btn')}
            </button>
          </div>
        </div>

        {/* Right Column: Unified Editor & Inspector workspace */}
        <div className="flex min-w-0 flex-col rounded-xl border border-[#242426]/50 bg-[#121213]/30 overflow-hidden">
          {selectedBlock ? (
            <div className="flex h-full flex-col">
              {/* Editor Header: Title Input + Role Selector + Metadata & Actions */}
              <div className="border-b border-[#242426]/40 bg-[#161617]/25 px-4 py-3 space-y-2">
                <div className="flex items-center justify-between gap-3">
                  <input
                    value={selectedBlock.title}
                    disabled={selectedBlock.locked || isSaving}
                    onChange={(event) => updateBlock(selectedBlock.id, { title: event.target.value })}
                    onBlur={() => {
                      void saveCurrentComposer();
                    }}
                    placeholder={t('assembler.block_title_placeholder')}
                    className="min-w-0 flex-1 rounded-lg border border-transparent bg-transparent hover:bg-[#1a1b1d]/40 focus:bg-[#1a1b1d]/75 focus:border-[#2b2b2d] px-2.5 py-1.5 text-[14px] font-semibold text-white outline-none transition disabled:opacity-60"
                  />
                  <div className="flex items-center gap-2 shrink-0">
                    <span className="flex items-center gap-1 rounded-full border border-[#242426] bg-[#18191b] px-2 py-0.5 text-[9px] font-semibold uppercase tracking-wider text-neutral-400">
                      {selectedBlock.source === 'builtin' && <Shield className="h-2.5 w-2.5 text-blue-400" />}
                      {selectedBlock.source === 'plugin' && <Wrench className="h-2.5 w-2.5 text-amber-400" />}
                      {selectedBlock.source === 'user' && <Sparkles className="h-2.5 w-2.5 text-cyan-400" />}
                      <span>{selectedBlock.source}</span>
                    </span>
                  </div>
                </div>

                {/* Sub-toolbar with role & direct details */}
                <div className="flex flex-wrap items-center justify-between gap-2.5 pt-1 border-t border-[#242426]/50">
                  <div className="flex items-center gap-2">
                    <span className="text-[10px] uppercase tracking-wider text-neutral-500 font-medium">{t('assembler.role')}</span>
                    <select
                      value={selectedBlock.role}
                      disabled={selectedBlock.locked || isSaving}
                      onChange={(event) => {
                        void handleRoleChange(
                          selectedBlock.id,
                          event.target.value === 'developer' ? 'developer' : 'system'
                        );
                      }}
                      className="rounded-lg border border-[#2b2b2d] bg-[#18191b] px-2 py-1 text-[11px] font-medium text-neutral-200 outline-none transition hover:border-[#3c3c40] focus:border-neutral-500 disabled:opacity-60"
                    >
                      <option value="system">system</option>
                      <option value="developer">developer</option>
                    </select>
                  </div>

                  {selectedBlock.source_id && (
                    <div className="flex items-center gap-1.5 text-[11px] text-neutral-500">
                      <span className="text-[10px] uppercase tracking-wider">Source ID:</span>
                      <span className="font-mono rounded border border-[#2b2b2d] bg-[#18191b]/80 px-1.5 py-0.5 text-[10px] text-neutral-400">
                        {selectedBlock.source_id}
                      </span>
                    </div>
                  )}

                  <div className="flex items-center gap-2">
                    <label className="inline-flex items-center gap-1.5 rounded-lg border border-[#2b2b2d] bg-[#18191b] px-2 py-1 cursor-pointer select-none text-[11px] text-neutral-350 hover:bg-[#202022] transition">
                      <span>{t('assembler.active')}</span>
                      <input
                        type="checkbox"
                        checked={selectedBlock.enabled}
                        disabled={isSaving || selectedBlock.locked}
                        onChange={(event) => {
                          void handleToggleEnabled(selectedBlock.id, event.target.checked);
                        }}
                        className="h-3.5 w-3.5 rounded border-[#2e2e30] bg-[#111112] text-cyan-400 focus:ring-cyan-400/40 transition cursor-pointer"
                      />
                    </label>

                    {canDeleteBlock(selectedBlock) && (
                      <button
                        type="button"
                        disabled={isSaving}
                        onClick={() => {
                          void handleDeleteBlock(selectedBlock.id);
                        }}
                        className="inline-flex items-center justify-center gap-1 rounded-lg border border-rose-500/20 bg-rose-500/10 px-2 py-1 text-[11px] font-medium text-rose-350 hover:bg-rose-500/15 transition disabled:opacity-40"
                      >
                        <Trash2 className="h-3 w-3" />
                        {t('sidebar.action.delete')}
                      </button>
                    )}
                  </div>
                </div>

                {/* Locked Banner / Info Message */}
                {selectedBlock.locked && (
                  <div className="flex items-center gap-1.5 rounded-lg border border-amber-500/10 bg-amber-500/[0.04] px-2.5 py-1.5 text-[11px] text-amber-450 leading-relaxed">
                    <span className="font-semibold text-amber-400 shrink-0">🔒 {t('assembler.managed_block')}</span>
                    <span>{t('assembler.managed_block_hint')}</span>
                  </div>
                )}
              </div>

              {/* Editor area with counting statistics */}
              <div className="relative flex-1 bg-[#0f0f10]/15 flex flex-col min-h-0">
                <textarea
                  value={selectedBlock.content}
                  disabled={selectedBlock.locked || isSaving}
                  onChange={(event) => updateBlock(selectedBlock.id, { content: event.target.value })}
                  onBlur={() => {
                    void saveCurrentComposer();
                  }}
                  placeholder={
                    selectedBlock.role === 'system'
                      ? t('assembler.placeholder.system')
                      : t('assembler.placeholder.developer')
                  }
                  className="w-full flex-1 resize-none bg-transparent px-4 py-3.5 font-mono text-[11.5px] leading-6 text-neutral-200 outline-none disabled:opacity-60 placeholder-neutral-600"
                />

                {/* Editor Footer Status Bar */}
                <div className="flex items-center justify-end gap-3 border-t border-[#242426]/30 bg-[#161617]/25 px-4 py-1.5 text-[10px] font-mono text-neutral-500 select-none shrink-0">
                  <span>{t('assembler.stats.chars', { count: String(selectedBlock.content.length) })}</span>
                  <span className="h-2 w-px bg-[#242426]/60" />
                  <span>{t('assembler.stats.words', { count: String(selectedBlock.content.split(/\s+/).filter(Boolean).length) })}</span>
                </div>
              </div>
            </div>
          ) : (
            <div className="flex flex-col h-full items-center justify-center text-center p-6 text-neutral-500">
              <Sparkles className="h-8 w-8 text-neutral-600 mb-2 animate-pulse" />
              <p className="text-sm font-medium text-neutral-400">{t('assembler.no_block_selected')}</p>
              <p className="mt-1 text-[11px] text-neutral-500 max-w-[240px]">
                {t('assembler.select_block_hint')}
              </p>
            </div>
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
