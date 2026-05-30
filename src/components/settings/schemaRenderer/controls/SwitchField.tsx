import type { FieldControlProps } from '../types';

// Boolean field control. Persists immediately because there is no intermediate draft state.
export function SwitchField({ value, disabled, resolvedPath, onSaveField }: FieldControlProps) {
  return (
    <label className="relative inline-flex h-5 w-9 cursor-pointer items-center">
      <input
        type="checkbox"
        checked={!!value}
        disabled={disabled}
        onChange={(event) => {
          void onSaveField(resolvedPath, event.target.checked);
        }}
        className="peer sr-only"
      />
      <span className="absolute inset-0 rounded-full bg-[#3e3e42] transition peer-checked:bg-[#007aff] peer-disabled:opacity-50" />
      <span className="absolute left-0.5 top-0.5 h-4 w-4 rounded-full bg-white transition-transform duration-200 peer-checked:translate-x-4" />
    </label>
  );
}
