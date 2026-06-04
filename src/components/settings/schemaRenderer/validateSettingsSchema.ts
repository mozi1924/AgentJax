import type { SettingsSchemaNode } from '../../../features/settings/types';

/** Result of a single schema validation check. */
export interface ValidationResult {
  valid: boolean;
  errors: string[];
}

/**
 * Known node kinds that the renderer can handle.
 * Used to detect unsupported or misspelled kinds.
 */
const KNOWN_NODE_KINDS = new Set([
  'field',
  'group',
  'collection',
  'layout',
  'panel',
  'tabs',
  'split',
  'toolbar',
  'list',
  'detail',
  'collapsible',
  'badge',
  'metric',
  'empty_state',
  'action',
]);

const KNOWN_VALUE_TYPES = new Set([
  'boolean',
  'integer',
  'float',
  'string',
  'enum',
  'secret',
  'string_list',
  'string_map',
  'json_map',
  'object',
  'object_collection',
]);

const KNOWN_CONTROL_TYPES = new Set([
  'switch',
  'select',
  'text',
  'textarea',
  'number',
  'secret',
  'tags',
  'key_value',
  'json',
  'prompt_assembler',
]);

/**
 * Validate a single settings schema node.
 * Returns a list of error messages (empty = valid).
 */
export function validateSettingsNode(node: unknown, path?: string): string[] {
  const errors: string[] = [];
  const label = path ? `node at "${path}"` : 'node';

  if (!node || typeof node !== 'object') {
    errors.push(`${label} is not an object`);
    return errors;
  }

  const obj = node as Record<string, unknown>;

  // Every node must have an id
  if (!obj.id || typeof obj.id !== 'string') {
    errors.push(`${label} is missing a valid "id" (string)`);
  }

  // Every node must have a kind
  if (!obj.kind || typeof obj.kind !== 'string') {
    errors.push(`${label} is missing a valid "kind" (string)`);
  } else if (!KNOWN_NODE_KINDS.has(obj.kind as string)) {
    errors.push(`${label} has unknown kind "${obj.kind}"`);
  }

  const kind = obj.kind as string;

  // Field-specific validation
  if (kind === 'field') {
    if (!obj.path || typeof obj.path !== 'string') {
      errors.push(`${label} (kind=field) is missing a valid "path" (string)`);
    }
    if (!obj.valueType || typeof obj.valueType !== 'string') {
      errors.push(`${label} (kind=field) is missing a valid "valueType" (string)`);
    } else if (!KNOWN_VALUE_TYPES.has(obj.valueType as string)) {
      errors.push(`${label} has unknown valueType "${obj.valueType}"`);
    }
    if (!obj.control || typeof obj.control !== 'string') {
      errors.push(`${label} (kind=field) is missing a valid "control" (string)`);
    } else if (!KNOWN_CONTROL_TYPES.has(obj.control as string)) {
      errors.push(`${label} has unknown control "${obj.control}"`);
    }
  }

  // Collection-specific validation
  if (kind === 'collection') {
    if (!obj.path || typeof obj.path !== 'string') {
      errors.push(`${label} (kind=collection) is missing a valid "path" (string)`);
    }
    if (obj.valueType !== 'object_collection') {
      errors.push(`${label} (kind=collection) must have valueType "object_collection"`);
    }
  }

  // Recursively validate children
  if (obj.children && Array.isArray(obj.children)) {
    obj.children.forEach((child, index) => {
      const childErrors = validateSettingsNode(child, `${path || label}.children[${index}]`);
      errors.push(...childErrors);
    });
  }

  // Validate tabs
  if (obj.tabs && Array.isArray(obj.tabs)) {
    obj.tabs.forEach((tab: unknown, index: number) => {
      const tabObj = tab as Record<string, unknown>;
      if (!tabObj.id || typeof tabObj.id !== 'string') {
        errors.push(`${label}.tabs[${index}] is missing a valid "id" (string)`);
      }
      if (tabObj.children && Array.isArray(tabObj.children)) {
        (tabObj.children as unknown[]).forEach((child, childIndex) => {
          const childErrors = validateSettingsNode(
            child,
            `${path || label}.tabs[${index}].children[${childIndex}]`
          );
          errors.push(...childErrors);
        });
      }
    });
  }

  // Validate itemTemplate
  if (obj.itemTemplate) {
    const templateErrors = validateSettingsNode(obj.itemTemplate, `${path || label}.itemTemplate`);
    errors.push(...templateErrors);
  }

  return errors;
}

/**
 * Validate a settings node and return a sanitized version or null.
 * If validation fails, logs warnings and returns null so the renderer
 * can fall back gracefully instead of crashing.
 */
export function safeValidateNode(node: unknown, contextLabel?: string): SettingsSchemaNode | null {
  const errors = validateSettingsNode(node);
  if (errors.length === 0) {
    return node as SettingsSchemaNode;
  }

  console.warn(
    `[SettingsSchema] ${contextLabel ? `(${contextLabel}) ` : ''}Schema validation failed:`,
    errors
  );
  return null;
}
