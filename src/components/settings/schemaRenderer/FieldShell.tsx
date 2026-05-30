import { useI18n } from '../../../features/i18n';
import type { FieldShellProps } from './types';

// Provides the shared field chrome so individual controls only own value input behavior.
export function FieldShell({
  field,
  secretStatus,
  helperText,
  hasError,
  fullWidth,
  children,
}: FieldShellProps) {
  const { t } = useI18n();

  return (
    <div className="border-b border-[#242426]/30 py-3 first:pt-0 last:border-b-0">
      <div
        className={`flex ${
          fullWidth ? 'flex-col gap-2' : 'flex-row items-center justify-between gap-4'
        }`}
      >
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-1.5">
            <h4 className="text-[13.5px] font-medium text-neutral-200">{t(field.title)}</h4>
            {field.advanced && (
              <span className="rounded bg-[#2e2e30] px-1.5 py-0.5 text-[9px] font-semibold uppercase tracking-wider text-neutral-400">
                {t('settings.renderer.advanced')}
              </span>
            )}
          </div>
          {field.description && (
            <p className="mt-0.5 max-w-[95%] text-[11.5px] leading-relaxed text-neutral-400/80">
              {t(field.description)}
            </p>
          )}
          {field.control === 'secret' && secretStatus && (
            <p className="mt-1 text-[11px] text-neutral-500">
              {secretStatus.configured
                ? t('settings.renderer.secret.configured', { source: secretStatus.source })
                : t('settings.renderer.secret.not_configured')}
            </p>
          )}
          {field.warningText && (
            <p className="mt-1 text-[11px] text-amber-500/80">{t(field.warningText)}</p>
          )}
          {helperText && (
            <p className={`mt-1 text-[11px] ${hasError ? 'text-rose-400' : 'text-neutral-500'}`}>
              {helperText}
            </p>
          )}
        </div>

        <div className={fullWidth ? 'mt-1.5 w-full' : 'flex shrink-0 items-center justify-end'}>
          {children}
        </div>
      </div>
    </div>
  );
}
