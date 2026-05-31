import { LoaderCircle, RefreshCcw, Search } from 'lucide-react';
import { useI18n } from '../../../features/i18n';
import type { SettingsUiAction, SettingsUiSchemaNode } from '../../../features/settings/types';
import type { SchemaRendererActions } from './types';

const actionIcon = (icon?: string) => {
  if (icon === 'refresh') return RefreshCcw;
  if (icon === 'search') return Search;
  return null;
};

// Action nodes are UI-only schema nodes that delegate behavior to renderer actions.
export function ActionRenderer({
  node,
  action,
  actions,
}: {
  node?: SettingsUiSchemaNode;
  action?: SettingsUiAction;
  actions: SchemaRendererActions;
}) {
  const { t } = useI18n();
  const descriptor = action || {
    id: node?.id || 'action',
    label: node?.label || node?.title,
    action: node?.id,
    icon: node?.icon,
    variant: node?.variant,
  };
  const Icon = actionIcon(descriptor.icon);

  return (
    <button
      type="button"
      onClick={() => {
        void actions.runAction?.(descriptor);
      }}
      className={`inline-flex h-7 items-center gap-1.5 rounded-md border border-[#2b2c30] px-2 text-[12px] transition ${
        descriptor.variant === 'primary'
          ? 'bg-neutral-200 text-neutral-900 hover:bg-white'
          : 'text-neutral-300 hover:bg-[#202124]'
      }`}
    >
      {Icon ? <Icon className="h-3.5 w-3.5" /> : null}
      {descriptor.label ? t(descriptor.label) : null}
      {descriptor.variant === 'loading' ? <LoaderCircle className="h-3.5 w-3.5 animate-spin" /> : null}
    </button>
  );
}
