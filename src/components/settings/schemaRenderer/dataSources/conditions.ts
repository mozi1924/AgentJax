import type { SettingsCondition } from '../../../../features/settings/types';

export const getItemPath = (root: unknown, path?: string): unknown => {
  if (!path) return undefined;
  return path.split('.').reduce<unknown>((current, segment) => {
    if (!current || typeof current !== 'object') return undefined;
    return (current as Record<string, unknown>)[segment];
  }, root);
};

export const textItemValue = (root: unknown, path?: string) => {
  const value = getItemPath(root, path);
  if (value === null || value === undefined) return '';
  return String(value);
};

export const boolItemValue = (root: unknown, path?: string) => !!getItemPath(root, path);

export const itemConditionMatches = (condition: SettingsCondition, item: unknown) => {
  const value = getItemPath(item, condition.path);
  if (typeof condition.truthy === 'boolean' && !!value !== condition.truthy) return false;
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
    return Array.isArray(value) && value.includes(condition.includes);
  }
  return true;
};

export const itemConditionsMatch = (
  conditions: SettingsCondition[] | undefined,
  item: unknown
) => !conditions?.length || conditions.every((condition) => itemConditionMatches(condition, item));

export const itemDisableConditionsMatch = (
  conditions: SettingsCondition[] | undefined,
  item: unknown
) => !!conditions?.length && conditions.every((condition) => itemConditionMatches(condition, item));
