import { useI18n } from '../../../../features/i18n';
import type { FieldControlProps } from '../types';

// Text-like field control for text, secret, and textarea controls.
export function TextField({ field, draft, setDraft, setIsDirty, disabled, commit }: FieldControlProps) {
  const { t } = useI18n();
  const placeholder = field.placeholder ? t(field.placeholder) : '';

  if (field.control === 'textarea') {
    return (
      <textarea
        rows={field.rows || 3}
        value={draft}
        placeholder={placeholder}
        disabled={disabled}
        onChange={(event) => {
          setDraft(event.target.value);
          setIsDirty(true);
        }}
        onBlur={() => {
          void commit(draft);
        }}
        className="w-full rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 px-2.5 py-2 text-xs text-neutral-200 outline-none transition focus:border-neutral-500 focus:bg-[#222326]/40 disabled:opacity-50"
      />
    );
  }

  return (
    <input
      type={field.control === 'secret' ? 'password' : 'text'}
      value={draft}
      placeholder={placeholder}
      disabled={disabled}
      onChange={(event) => {
        setDraft(event.target.value);
        setIsDirty(true);
      }}
      onBlur={() => {
        void commit(draft);
      }}
      className="w-[240px] rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 px-2.5 py-1 text-xs text-neutral-200 outline-none transition focus:border-neutral-500 focus:bg-[#222326]/40 disabled:opacity-50"
    />
  );
}
