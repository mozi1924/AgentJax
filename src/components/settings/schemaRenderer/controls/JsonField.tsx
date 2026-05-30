import type { FieldControlProps } from '../types';

// JSON field validates object-shaped JSON locally before committing to settings storage.
export function JsonField({ draft, setDraft, setIsDirty, disabled, commit, setLocalError }: FieldControlProps) {
  return (
    <textarea
      rows={5}
      value={draft}
      placeholder={'{\n  "custom": true\n}'}
      disabled={disabled}
      onChange={(event) => {
        setDraft(event.target.value);
        setIsDirty(true);
      }}
      onBlur={() => {
        try {
          const parsed = draft.trim() ? JSON.parse(draft) : {};
          if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
            setLocalError('settings.renderer.json.invalid');
            return;
          }
          void commit(parsed);
        } catch {
          setLocalError('settings.renderer.json.invalid');
        }
      }}
      className="w-full rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 px-2.5 py-2 font-mono text-xs text-neutral-200 outline-none transition focus:border-neutral-500 focus:bg-[#222326]/40 disabled:opacity-50"
    />
  );
}
