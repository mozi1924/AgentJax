import { useI18n } from '../../../../features/i18n';
import { coerceSelectValue } from '../../renderer/utils';
import type { FieldControlProps } from '../types';

const accentColor = (value: string) => {
  if (value === 'green') return '#10a37f';
  if (value === 'blue') return '#007aff';
  if (value === 'purple') return '#a855f7';
  if (value === 'orange') return '#f97316';
  return '#737373';
};

// Select persists on change so option-driven settings do not require a blur event.
export function SelectField({
  field,
  draft,
  setDraft,
  setIsDirty,
  disabled,
  options,
  resolvedPath,
  onSaveField,
}: FieldControlProps) {
  const { t } = useI18n();

  return (
    <div className="relative inline-flex items-center">
      {field.id === 'accent-color' && (
        <span
          className="mr-1 h-2.5 w-2.5 shrink-0 rounded-full transition-colors"
          style={{ backgroundColor: accentColor(draft) }}
        />
      )}
      <select
        value={draft}
        disabled={disabled}
        onChange={(event) => {
          const rawValue = event.target.value;
          setDraft(rawValue);
          setIsDirty(true);
          void onSaveField(resolvedPath, coerceSelectValue(field, rawValue));
        }}
        className="cursor-pointer appearance-none rounded-md bg-transparent py-0.5 pl-2 pr-5 text-right text-[13px] font-normal text-neutral-200 outline-none transition hover:bg-neutral-800/40 disabled:opacity-50"
      >
        {options.map((option) => (
          <option key={`${field.id}-${option.value}`} value={option.value} className="bg-[#1a1b1d] text-neutral-200">
            {t(option.label)}
          </option>
        ))}
      </select>
      <span className="pointer-events-none absolute right-1 top-1/2 -translate-y-1/2 text-neutral-500">
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none">
          <path
            d="M6 9l6 6 6-6"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      </span>
    </div>
  );
}
