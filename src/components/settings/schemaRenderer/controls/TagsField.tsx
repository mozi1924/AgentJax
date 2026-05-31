import { parseTagsDraftValue } from '../../../../features/settings/fieldControlDrafts';
import type { FieldControlProps } from '../types';

// Tags are stored as string arrays while the draft remains a comma-delimited text area.
export function TagsField({ draft, setDraft, setIsDirty, disabled, commit }: FieldControlProps) {
  return (
    <textarea
      rows={2}
      value={draft}
      placeholder="Use commas to separate items"
      disabled={disabled}
      onChange={(event) => {
        setDraft(event.target.value);
        setIsDirty(true);
      }}
      onBlur={() => {
        void commit(parseTagsDraftValue(draft));
      }}
      className="w-full rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 px-2.5 py-2 text-xs text-neutral-200 outline-none transition focus:border-neutral-500 focus:bg-[#222326]/40 disabled:opacity-50"
    />
  );
}
