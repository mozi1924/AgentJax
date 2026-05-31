export interface DeclaredPermissions {
  allowNetwork: boolean;
  allowFileRead: boolean;
  allowFileWrite: boolean;
  allowProcessSpawn: boolean;
  allowEnvRead: boolean;
  allowedHosts: string[];
}

export interface EffectivePermissions {
  allowNetwork: boolean;
  allowFileRead: boolean;
  allowFileWrite: boolean;
  allowProcessSpawn: boolean;
  allowEnvRead: boolean;
}

export interface PluginEntryPolicyPaths {
  pluginEnabledPath: string;
  permissionNetworkPath: string;
  permissionFileReadPath: string;
  permissionFileWritePath: string;
  permissionProcessSpawnPath: string;
  permissionEnvReadPath: string;
}

export interface PluginEntrySnapshot {
  id: string;
  name: string;
  version: string;
  description: string;
  isBuiltin: boolean;
  enabled: boolean;
  hasTools: boolean;
  declaredPermissions: DeclaredPermissions;
  effectivePermissions: EffectivePermissions;
  policyPaths: PluginEntryPolicyPaths;
}

export interface PluginManagerSnapshot {
  plugins: PluginEntrySnapshot[];
}

// Permission labels for UI display
export const PERMISSION_LABELS: Record<keyof EffectivePermissions, string> = {
  allowNetwork: 'settings.plugins.permission.network',
  allowFileRead: 'settings.plugins.permission.file_read',
  allowFileWrite: 'settings.plugins.permission.file_write',
  allowProcessSpawn: 'settings.plugins.permission.process_spawn',
  allowEnvRead: 'settings.plugins.permission.env_read',
};

export const PERMISSION_ICONS: Record<keyof EffectivePermissions, string> = {
  allowNetwork: 'Globe',
  allowFileRead: 'FileText',
  allowFileWrite: 'FileEdit',
  allowProcessSpawn: 'Terminal',
  allowEnvRead: 'Variable',
};

export const PERMISSION_POLICY_PATHS: Record<
  keyof EffectivePermissions,
  keyof PluginEntryPolicyPaths
> = {
  allowNetwork: 'permissionNetworkPath',
  allowFileRead: 'permissionFileReadPath',
  allowFileWrite: 'permissionFileWritePath',
  allowProcessSpawn: 'permissionProcessSpawnPath',
  allowEnvRead: 'permissionEnvReadPath',
};
