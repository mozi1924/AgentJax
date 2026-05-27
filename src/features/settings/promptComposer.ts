export type PromptBlockRole = 'system' | 'developer';
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
  developerMessages: string[];
  previewMarkdown: string;
}

const normalizeRole = (value: unknown): PromptBlockRole =>
  value === 'developer' ? 'developer' : 'system';

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

const defaultTitleForRole = (role: PromptBlockRole, fallbackIndex: number) =>
  role === 'system'
    ? `System block ${fallbackIndex + 1}`
    : `Developer block ${fallbackIndex + 1}`;

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

  const systemBlocks = blocks.filter((block) => block.role === 'system');
  const developerBlocks = blocks.filter((block) => block.role === 'developer');

  return {
    blocks: [...systemBlocks, ...developerBlocks],
  };
};

export const createPromptBlock = (role: PromptBlockRole): PromptBlock => ({
  id: `user-${role}-${crypto.randomUUID()}`,
  title: role === 'system' ? 'New system block' : 'New developer block',
  role,
  content: '',
  enabled: true,
  source: 'user',
  source_id: null,
  locked: false,
});

export const compilePromptComposerPreview = (
  composer: PromptComposerConfig
): CompiledPromptPreview => {
  const systemBlocks = composer.blocks.filter(
    (block) => block.role === 'system' && block.enabled && block.content.trim()
  );
  const developerBlocks = composer.blocks.filter(
    (block) => block.role === 'developer' && block.enabled && block.content.trim()
  );

  const instructionsText = systemBlocks.map((block) => block.content.trim()).join('\n\n');
  const developerMessages = developerBlocks.map((block) => block.content.trim());

  const previewSections: string[] = ['## System / instructions'];
  if (systemBlocks.length === 0) {
    previewSections.push('_No active system blocks._');
  } else {
    systemBlocks.forEach((block) => {
      previewSections.push(`### ${block.title}\n\n${block.content.trim()}`);
    });
  }

  previewSections.push('## Developer messages');
  if (developerBlocks.length === 0) {
    previewSections.push('_No active developer blocks._');
  } else {
    developerBlocks.forEach((block, index) => {
      previewSections.push(`### ${index + 1}. ${block.title}\n\n${block.content.trim()}`);
    });
  }

  return {
    instructionsText,
    developerMessages,
    previewMarkdown: previewSections.join('\n\n'),
  };
};
