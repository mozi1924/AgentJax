export type PromptBlockRole = 'system';
export type PromptBlockSource = 'builtin' | 'user' | 'plugin';

export interface PromptBlock {
  id: string;
  title: string;
  role: PromptBlockRole;
  content: string;
  enabled: boolean;
  source: PromptBlockSource;
  source_id?: string | null;
  locked: boolean;
}

export interface PromptComposerConfig {
  blocks: PromptBlock[];
}

export interface CompiledPromptPreview {
  instructionsText: string;
  systemBlocks: { title: string; content: string }[];
  previewMarkdown: string;
}

const normalizeRole = (_value: unknown): PromptBlockRole => 'system';

const normalizeSource = (value: unknown): PromptBlockSource => {
  if (value === 'builtin' || value === 'plugin') {
    return value;
  }
  return 'user';
};

const sanitizeId = (value: unknown, fallbackIndex: number) => {
  const candidate = `${value ?? ''}`
    .trim()
    .replace(/[^A-Za-z0-9._-]+/g, '-')
    .replace(/^-+|-+$/g, '');
  return candidate || `prompt-block-${fallbackIndex + 1}`;
};

const defaultTitleForRole = (_role: PromptBlockRole, fallbackIndex: number) =>
  `System block ${fallbackIndex + 1}`;

export const normalizePromptComposer = (value: unknown): PromptComposerConfig => {
  const sourceBlocks =
    value && typeof value === 'object' && Array.isArray((value as { blocks?: unknown }).blocks)
      ? (value as { blocks: unknown[] }).blocks
      : [];

  const blocks = sourceBlocks
    .map((entry, index): PromptBlock | null => {
      if (!entry || typeof entry !== 'object' || Array.isArray(entry)) {
        return null;
      }

      const raw = entry as Record<string, unknown>;
      const role = normalizeRole(raw.role);
      const source = normalizeSource(raw.source);
      const title = `${raw.title ?? ''}`.trim() || defaultTitleForRole(role, index);

      return {
        id: sanitizeId(raw.id, index),
        title,
        role,
        content: `${raw.content ?? ''}`.trim(),
        enabled: raw.enabled !== false,
        source,
        source_id:
          source === 'user'
            ? null
            : `${raw.source_id ?? ''}`.trim() || `${raw.sourceId ?? ''}`.trim() || null,
        locked: raw.locked === true,
      };
    })
    .filter((entry): entry is PromptBlock => entry !== null);

  // All blocks are system role — preserve original order.
  return { blocks };
};

export const createPromptBlock = (_role?: PromptBlockRole): PromptBlock => ({
  id: `user-system-${crypto.randomUUID()}`,
  title: 'New system block',
  role: 'system',
  content: '',
  enabled: true,
  source: 'user',
  source_id: null,
  locked: false,
});

export const compilePromptComposerPreview = (
  composer: PromptComposerConfig
): CompiledPromptPreview => {
  const activeBlocks = composer.blocks.filter(
    (block) => block.enabled && block.content.trim()
  );

  const instructionsText = activeBlocks.map((block) => block.content.trim()).join('\n\n');
  const systemBlocks = activeBlocks.map((block) => ({ title: block.title, content: block.content.trim() }));

  const previewSections: string[] = ['## System prompt blocks'];
  if (activeBlocks.length === 0) {
    previewSections.push('_No active blocks._');
  } else {
    activeBlocks.forEach((block, index) => {
      previewSections.push(`### ${index + 1}. ${block.title}\n\n${block.content.trim()}`);
    });
  }

  return {
    instructionsText,
    systemBlocks,
    previewMarkdown: previewSections.join('\n\n'),
  };
};
