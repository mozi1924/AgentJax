// ── Type-safe config value access ───────────────────────────────────────────
//
// Re-exports generated Rust-backed types and provides typed accessor
// helpers so callers don't have to reach into `snapshot.values` directly.
//
// The types in `__generated__/config-types.ts` are auto-generated from
// Rust structs via `schemars` (run `pnpm gen:schemas` to update).
//
// Usage:
//   import { getAppConfigValue, AppConfig } from './configAccess';
//   const timeout = getAppConfigValue(snapshot.values, 'request_timeout_seconds');

export type {
  AppConfig,
  ProviderConfig,
  ProviderModelConfig,
  ModelRequestConfig,
  McpConfig,
  McpServerConfig,
  McpTransportKind,
  MemoryConfig,
  ContextManagementConfig,
  SubAgentConfig,
  RagConfig,
  EmbeddingProviderConfig,
  ToolManagerConfig,
  PluginManagerConfig,
  SettingsSnapshot as SettingsSnapshotSchema,
  SettingsOption as SettingsOptionSchema,
  PromptComposerConfig,
  PromptBlock,
  PromptBlockRole,
  PromptBlockSource,
} from './__generated__/config-types';

import { getValueAtPath } from './utils';

/** Read a typed config value by its dot-separated path.
 *  Returns `undefined` when the path doesn't exist. */
export function getAppConfigValue<T>(
  values: import('./types').SettingsSnapshot['values'],
  path: string,
): T | undefined {
  return getValueAtPath<T>(values, path);
}

/** Read a value from a settings collection item.
 *  Usage: `getCollectionItemValue(snapshot.values, 'providers', 'openai', 'kind')` */
export function getCollectionItemValue<T>(
  values: import('./types').SettingsSnapshot['values'],
  collectionPath: string,
  itemKey: string,
  fieldPath: string,
): T | undefined {
  return getValueAtPath<T>(values, `${collectionPath}.${itemKey}.${fieldPath}`);
}
