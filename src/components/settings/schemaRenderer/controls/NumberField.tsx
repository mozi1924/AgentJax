import { useI18n } from '../../../../features/i18n';
import { parseNumberDraftValue } from '../../../../features/settings/fieldControlDrafts';
import type { FieldControlProps } from '../types';

// Numeric field control parses according to the schema value type before committing.
export function NumberField({ field, draft, setDraft, setIsDirty, disabled, commit }: FieldControlProps) {
  const { t } = useI18n();

  return (
    <input
      type="number"
      value={draft}
      placeholder={field.placeholder ? t(field.placeholder) : ''}
      min={field.min}
      max={field.max}
      step={field.step || (field.valueType === 'integer' ? 1 : 0.1)}
      disabled={disabled}
      onChange={(event) => {
        setDraft(event.target.value);
        setIsDirty(true);
      }}
      onBlur={() => {
        void commit(parseNumberDraftValue(field, draft));
      }}
      className="w-[100px] rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 px-2.5 py-1 text-right text-xs text-neutral-200 outline-none transition focus:border-neutral-500 focus:bg-[#222326]/40 disabled:opacity-50"
    />
  );
}
