import type { ModelOption } from '../conversations/types';

export const DEFAULT_MODEL_PROFILE = 'openai/gpt-5-mini';
export const DEFAULT_REASONING_MODE = '__default__';

interface ProfileSplit {
  providerKey: string;
  modelKey: string;
}

const splitProfileKey = (value: string | null | undefined): ProfileSplit | null => {
  const raw = `${value || ''}`.trim();
  if (!raw) return null;
  const [providerKey, modelKey] = raw.split('/');
  if (!providerKey || !modelKey) return null;
  return {
    providerKey: providerKey.trim(),
    modelKey: modelKey.trim(),
  };
};

export const buildFallbackModelOption = (profileKey: string): ModelOption => {
  const normalizedProfileKey = `${profileKey || ''}`.trim();
  const split = splitProfileKey(normalizedProfileKey);
  return {
    profileKey: normalizedProfileKey,
    providerKey: split?.providerKey || '',
    modelId: split?.modelKey || normalizedProfileKey,
    supportsReasoning: false,
    supportedReasoningLevels: [],
    configuredReasoningEffort: null,
  };
};

export const normalizeModelOption = (
  option: Partial<ModelOption> | null | undefined
): ModelOption | null => {
  const profileKey = (option?.profileKey || option?.modelId || '').trim();
  if (!profileKey) {
    return null;
  }

  const providerFromProfile = splitProfileKey(profileKey)?.providerKey || '';
  const modelFromProfile = splitProfileKey(profileKey)?.modelKey || '';
  const providedProviderKey = (option?.providerKey || '').trim();
  const normalizedProviderKey = providedProviderKey || providerFromProfile;
  const normalizedProfileKey =
    normalizedProviderKey && !profileKey.includes('/')
      ? `${normalizedProviderKey}/${profileKey}`
      : profileKey;

  const configuredReasoningEffort = (option?.configuredReasoningEffort || '')
    .trim()
    .toLowerCase();
  return {
    profileKey: normalizedProfileKey,
    providerKey: normalizedProviderKey,
    modelId: (option?.modelId || modelFromProfile || profileKey).trim(),
    supportsReasoning: !!option?.supportsReasoning,
    supportedReasoningLevels: Array.isArray(option?.supportedReasoningLevels)
      ? option.supportedReasoningLevels
          .map((level) => `${level || ''}`.trim().toLowerCase())
          .filter(Boolean)
      : [],
    configuredReasoningEffort: configuredReasoningEffort || null,
  };
};

export const resolveConfiguredDefaultOptionProfileKey = (
  configuredDefault: string | null | undefined,
  options: ModelOption[]
): string | null => {
  const normalizedDefault = `${configuredDefault || ''}`.trim();
  if (!normalizedDefault || options.length === 0) {
    return null;
  }

  const direct = options.find((option) => option.profileKey === normalizedDefault);
  if (direct) return direct.profileKey;

  const split = splitProfileKey(normalizedDefault);
  if (!split) return null;
  const byProviderAndModelId = options.find(
    (option) =>
      option.providerKey === split.providerKey && option.modelId === split.modelKey
  );
  return byProviderAndModelId?.profileKey || null;
};
