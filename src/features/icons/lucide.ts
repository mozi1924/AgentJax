import * as LucideIcons from 'lucide-react';
import { Wrench } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';

const BUILT_IN_TOOL_ICON_NAMES: Record<string, string> = {
  calculator: 'Calculator',
  get_system_time: 'Clock3',
  read_file: 'FileSearch',
  write_file: 'FilePenLine',
};

const resolveFallbackToolIconName = (toolName?: string | null) => {
  const normalizedToolName = (toolName || '').trim();
  if (!normalizedToolName) {
    return null;
  }

  if (
    normalizedToolName.startsWith('mcp__') ||
    normalizedToolName.startsWith('mcp_server__')
  ) {
    return 'LayoutGrid';
  }

  return BUILT_IN_TOOL_ICON_NAMES[normalizedToolName] || null;
};

/**
 * Centralized Lucide icon lookup keeps schema-driven icon strings reusable
 * across settings, chat transcript tool cards, and future tool management UI.
 */
export function resolveLucideIcon(
  iconName?: string | null,
  fallbackIcon: LucideIcon = Wrench
): LucideIcon {
  if (!iconName) {
    return fallbackIcon;
  }

  const candidate = (LucideIcons as Record<string, unknown>)[iconName];
  return candidate && (typeof candidate === 'function' || typeof candidate === 'object')
    ? (candidate as LucideIcon)
    : fallbackIcon;
}

export function resolveToolLucideIcon(
  toolName?: string | null,
  iconName?: string | null,
  fallbackIcon: LucideIcon = Wrench
): LucideIcon {
  const toolFallbackIconName = resolveFallbackToolIconName(toolName);
  const toolFallbackIcon = toolFallbackIconName
    ? resolveLucideIcon(toolFallbackIconName, fallbackIcon)
    : fallbackIcon;

  return resolveLucideIcon(iconName, toolFallbackIcon);
}
