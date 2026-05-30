import { LoaderCircle } from 'lucide-react';
import { useI18n } from '../../../features/i18n';
import {
  isToolPolicyEditable,
  type ToolManagerSourceSnapshot,
  type ToolManagerToolSnapshot,
} from '../../../features/settings/toolManagerView';

export function ToolPolicySwitch({
  checked,
  disabled,
  loading,
  title,
  onChange,
}: {
  checked: boolean;
  disabled?: boolean;
  loading?: boolean;
  title: string;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label
      className={`relative inline-flex h-5 w-9 items-center ${
        disabled ? 'cursor-not-allowed opacity-60' : 'cursor-pointer'
      }`}
      title={title}
    >
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
        className="peer sr-only"
      />
      <span className="absolute inset-0 rounded-full bg-[#3e3e42] transition peer-checked:bg-cyan-500" />
      <span className="absolute left-0.5 top-0.5 flex h-4 w-4 items-center justify-center rounded-full bg-white transition-transform duration-200 peer-checked:translate-x-4">
        {loading && <LoaderCircle className="h-2.5 w-2.5 animate-spin text-neutral-700" />}
      </span>
    </label>
  );
}

// Shared tool policy surface used by both the list and detail panel.
export function ToolPolicyControls({
  source,
  tool,
  saving,
  onSaveToolEnabled,
}: {
  source: ToolManagerSourceSnapshot;
  tool: ToolManagerToolSnapshot;
  saving: boolean;
  onSaveToolEnabled: (
    source: ToolManagerSourceSnapshot,
    tool: ToolManagerToolSnapshot,
    enabled: boolean
  ) => void;
}) {
  const { t } = useI18n();

  if (isToolPolicyEditable(source)) {
    return (
      <ToolPolicySwitch
        checked={tool.enabled}
        loading={saving}
        disabled={saving}
        title={tool.enabled ? t('settings.tools.disable_tool') : t('settings.tools.enable_tool')}
        onChange={(nextEnabled) => onSaveToolEnabled(source, tool, nextEnabled)}
      />
    );
  }

  return (
    <span
      className={`shrink-0 rounded px-1.5 py-0.5 text-[10px] ${
        tool.enabled ? 'bg-emerald-500/10 text-emerald-200' : 'bg-rose-500/10 text-rose-200'
      }`}
    >
      {tool.enabled ? t('settings.tools.status.enabled') : t('settings.tools.status.disabled')}
    </span>
  );
}
