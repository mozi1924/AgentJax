import type {
  SettingsCollectionSchema,
  SettingsCondition,
  SettingsFieldSchema,
  SettingsSchemaNode,
  SettingsSectionSchema,
  SettingsSnapshot,
} from './types';

export const deepClone = <T>(value: T): T => JSON.parse(JSON.stringify(value));

export const escapePathSegment = (segment: string) =>
  `${segment || ''}`.replace(/\\/g, '\\\\').replace(/\./g, '\\.');

const splitPath = (path: string) => {
  const trimmed = `${path || ''}`.trim();
  if (!trimmed) return [];

  const segments: string[] = [];
  let current = '';
  let escaped = false;

  for (const char of trimmed) {
    if (escaped) {
      current += char;
      escaped = false;
      continue;
    }

    if (char === '\\') {
      escaped = true;
      continue;
    }

    if (char === '.') {
      segments.push(current);
      current = '';
      continue;
    }

    current += char;
  }

  if (escaped) {
    current += '\\';
  }

  segments.push(current);
  return segments;
};

export const appendPathSegment = (basePath: string, segment: string) => {
  const escapedSegment = escapePathSegment(segment);
  if (!basePath.trim()) {
    return escapedSegment;
  }
  return `${basePath}.${escapedSegment}`;
};

export const resolvePath = (path: string, contextPath?: string) => {
  const trimmed = `${path || ''}`.trim();
  if (!trimmed) return contextPath || '';
  if (!contextPath) return trimmed;
  return `${contextPath}.${trimmed}`;
};

/** Read a value from a nested object by dot-separated path.
 *  Callers can specify the expected return type via the type parameter. */
export const getValueAtPath = <T = unknown>(root: unknown, path: string): T | undefined => {
  const segments = splitPath(path).filter(Boolean);
  if (segments.length === 0) return root as T;

  return segments.reduce<unknown>((current, segment) => {
    if (!current || typeof current !== 'object') return undefined;
    return (current as Record<string, unknown>)[segment];
  }, root) as T | undefined;
};

export const setValueAtPath = (root: unknown, path: string, value: unknown) => {
  const nextRoot = deepClone(root);
  const segments = splitPath(path).filter(Boolean);
  if (segments.length === 0) {
    return value;
  }

  let current = nextRoot as Record<string, unknown>;
  segments.forEach((segment, index) => {
    if (index === segments.length - 1) {
      current[segment] = value;
      return;
    }

    const existing = current[segment];
    if (!existing || typeof existing !== 'object' || Array.isArray(existing)) {
      current[segment] = {};
    }
    current = current[segment] as Record<string, unknown>;
  });

  return nextRoot;
};

export const deleteValueAtPath = (root: unknown, path: string) => {
  const nextRoot = deepClone(root);
  const segments = splitPath(path).filter(Boolean);
  if (segments.length === 0) {
    return nextRoot;
  }

  let current = nextRoot as Record<string, unknown>;
  segments.forEach((segment, index) => {
    if (index === segments.length - 1) {
      delete current[segment];
      return;
    }
    const existing = current[segment];
    if (!existing || typeof existing !== 'object' || Array.isArray(existing)) {
      current[segment] = {};
    }
    current = current[segment] as Record<string, unknown>;
  });

  return nextRoot;
};

const matchesCondition = (
  condition: SettingsCondition,
  snapshot: SettingsSnapshot,
  contextPath?: string
) => {
  const value = getValueAtPath(snapshot.values, resolvePath(condition.path, contextPath));

  if (typeof condition.truthy === 'boolean') {
    const isTruthy = !!value;
    if (condition.truthy !== isTruthy) {
      return false;
    }
  }

  if (Object.prototype.hasOwnProperty.call(condition, 'equals') && value !== condition.equals) {
    return false;
  }

  if (
    Object.prototype.hasOwnProperty.call(condition, 'notEquals') &&
    value === condition.notEquals
  ) {
    return false;
  }

  if (condition.includes) {
    if (!Array.isArray(value) || !value.includes(condition.includes)) {
      return false;
    }
  }

  return true;
};

export const isNodeVisible = (
  node: SettingsSchemaNode,
  snapshot: SettingsSnapshot,
  contextPath?: string
) => {
  if (!node.visibleWhen || node.visibleWhen.length === 0) return true;
  return node.visibleWhen.every((condition) => matchesCondition(condition, snapshot, contextPath));
};

export const isNodeEnabled = (
  node: SettingsSchemaNode,
  snapshot: SettingsSnapshot,
  contextPath?: string
) => {
  if (!node.enabledWhen || node.enabledWhen.length === 0) return true;
  return node.enabledWhen.every((condition) => matchesCondition(condition, snapshot, contextPath));
};

export const getFieldOptions = (
  field: SettingsFieldSchema,
  snapshot: SettingsSnapshot,
  contextPath?: string
) => {
  if (field.optionSourceKey) {
    if (contextPath) {
      const scopedKey = `${field.optionSourceKey}@${contextPath}`;
      const scoped = snapshot.dynamicOptions[scopedKey];
      if (Array.isArray(scoped) && scoped.length > 0) {
        return scoped;
      }
    }

    return snapshot.dynamicOptions[field.optionSourceKey] || [];
  }
  return field.options || [];
};

export const asStringArray = (value: unknown) =>
  Array.isArray(value) ? value.map((entry) => `${entry ?? ''}`.trim()).filter(Boolean) : [];

export const asRecord = (value: unknown) =>
  value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};

export const getCollectionItems = (
  collection: SettingsCollectionSchema,
  snapshot: SettingsSnapshot,
  contextPath?: string
) => {
  const value = getValueAtPath(snapshot.values, resolvePath(collection.path, contextPath));
  const objectValue = asRecord(value);
  return Object.entries(objectValue).sort(([left], [right]) => left.localeCompare(right));
};

export const validateFieldValue = (
  field: SettingsFieldSchema,
  value: unknown,
  t?: (key: string, replacements?: Record<string, string>) => string
) => {
  if (field.control === 'text' || field.control === 'textarea' || field.control === 'secret') {
    const text = `${value ?? ''}`;
    if (typeof field.minLength === 'number' && text.length < field.minLength) {
      return t
        ? t('settings.validation.min_length', { count: String(field.minLength) })
        : `至少输入 ${field.minLength} 个字符`;
    }
    if (typeof field.maxLength === 'number' && text.length > field.maxLength) {
      return t
        ? t('settings.validation.max_length', { count: String(field.maxLength) })
        : `最多输入 ${field.maxLength} 个字符`;
    }
    if (field.pattern) {
      const regex = new RegExp(field.pattern);
      if (text && !regex.test(text)) {
        return t ? t('settings.validation.pattern') : '输入格式不符合要求';
      }
    }
  }

  if (field.control === 'number' && value !== null && value !== '' && value !== undefined) {
    const numericValue = Number(value);
    if (!Number.isFinite(numericValue)) {
      return t ? t('settings.validation.number') : '请输入合法数字';
    }
    if (typeof field.min === 'number' && numericValue < field.min) {
      return t
        ? t('settings.validation.min', { count: String(field.min) })
        : `最小值为 ${field.min}`;
    }
    if (typeof field.max === 'number' && numericValue > field.max) {
      return t
        ? t('settings.validation.max', { count: String(field.max) })
        : `最大值为 ${field.max}`;
    }
  }

  return null;
};

export const buildOptimisticSnapshot = (
  snapshot: SettingsSnapshot,
  path: string,
  value: unknown,
  operation: 'set' | 'delete' = 'set'
): SettingsSnapshot => ({
  ...snapshot,
  values:
    operation === 'delete'
      ? (deleteValueAtPath(snapshot.values, path) as Record<string, unknown>)
      : (setValueAtPath(snapshot.values, path, value) as Record<string, unknown>),
});

export const findFirstSection = (sections: SettingsSectionSchema[]) => sections[0]?.id || '';
