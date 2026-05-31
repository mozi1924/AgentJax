import { LoaderCircle, Search } from 'lucide-react';
import type { ReactNode } from 'react';
import type { SettingsUiAction, SettingsUiProperty } from '../../../../features/settings/types';
import { resolveLucideIcon } from '../../../../features/icons/lucide';
import type { SchemaRendererDataContext } from '../types';
import {
  boolItemValue,
  itemConditionsMatch,
  itemDisableConditionsMatch,
  textItemValue,
} from './conditions';

export const classNames = (...values: Array<string | false | null | undefined>) =>
  values.filter(Boolean).join(' ');

export const dataSourceActionVisible = (action: SettingsUiAction, item: unknown) =>
  itemConditionsMatch(action.visibleWhen, item);

export const dataSourceActionDisabled = (action: SettingsUiAction, item: unknown) =>
  itemDisableConditionsMatch(action.disabledWhen, item);

export function DataSourceBadge({
  children,
  tone = 'neutral',
  mono = false,
}: {
  children: ReactNode;
  tone?: 'neutral' | 'warning';
  mono?: boolean;
}) {
  return (
    <span
      className={classNames(
        'min-w-0 truncate rounded px-1.5 py-0.5 text-[10px]',
        mono && 'font-mono',
        tone === 'warning' ? 'bg-amber-500/10 text-amber-200' : 'bg-[#2a2a2c] text-neutral-300'
      )}
    >
      {children}
    </span>
  );
}

function DataSourceSwitch({
  checked,
  loading,
  disabled,
  onChange,
}: {
  checked: boolean;
  loading?: boolean;
  disabled?: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label
      className={classNames(
        'relative inline-flex h-5 w-9 items-center',
        disabled ? 'cursor-not-allowed opacity-60' : 'cursor-pointer'
      )}
      onClick={(event) => event.stopPropagation()}
    >
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
        className="peer sr-only"
      />
      <span className="absolute inset-0 rounded-full bg-[#3e3e42] transition peer-checked:bg-[#007aff]" />
      <span className="absolute left-0.5 top-0.5 flex h-4 w-4 items-center justify-center rounded-full bg-white transition-transform duration-200 peer-checked:translate-x-4">
        {loading && <LoaderCircle className="h-2.5 w-2.5 animate-spin text-neutral-700" />}
      </span>
    </label>
  );
}

export function DataSourceActionControl({
  action,
  item,
  dataSource,
  dataContext,
  translate,
}: {
  action: SettingsUiAction;
  item: unknown;
  dataSource?: string;
  dataContext: SchemaRendererDataContext;
  translate: (label?: string) => string;
}) {
  if (!dataSourceActionVisible(action, item)) return null;

  const schemaDisabled = dataSourceActionDisabled(action, item);

  if (action.variant === 'switch') {
    const checked = boolItemValue(item, action.value);
    const savingKey = textItemValue(item, action.savingKey);
    const disabled = !textItemValue(item, action.path);
    return (
      <DataSourceSwitch
        key={action.id}
        checked={checked}
        disabled={disabled || schemaDisabled || dataContext.isSaving?.(savingKey)}
        loading={dataContext.isSaving?.(savingKey)}
        onChange={(nextChecked) => {
          void dataContext.dispatch(action.id, {
            dataSource: action.dataSource || dataSource,
            item,
            path: textItemValue(item, action.path),
            value: nextChecked,
          });
        }}
      />
    );
  }

  if (action.variant === 'segmented') {
    const currentValue = textItemValue(item, action.value);
    const savingKey = textItemValue(item, action.savingKey);
    return (
      <div
        key={action.id}
        className="inline-flex h-7 overflow-hidden rounded-md border border-[#2b2b2d] bg-[#171719]"
      >
        {(action.options || []).map((option) => (
          <button
            key={option.value}
            type="button"
            disabled={
              schemaDisabled ||
              currentValue === option.value ||
              dataContext.isSaving?.(savingKey)
            }
            onClick={() => {
              void dataContext.dispatch(action.id, {
                dataSource: action.dataSource || dataSource,
                item,
                path: textItemValue(item, action.path),
                value: option.value,
              });
            }}
            className={classNames(
              'px-2.5 text-[11px] transition disabled:cursor-default disabled:opacity-70',
              currentValue === option.value
                ? 'bg-[#2a2a2c] text-white'
                : 'text-neutral-400 hover:bg-[#202022] hover:text-white'
            )}
          >
            {translate(option.label) || option.value}
          </button>
        ))}
      </div>
    );
  }

  if (action.variant === 'select') {
    const currentValue = textItemValue(item, action.value);
    const savingKey = textItemValue(item, action.savingKey);
    return (
      <select
        key={action.id}
        value={currentValue}
        disabled={schemaDisabled || dataContext.isSaving?.(savingKey)}
        onChange={(event) => {
          void dataContext.dispatch(action.id, {
            dataSource: action.dataSource || dataSource,
            item,
            path: textItemValue(item, action.path),
            value: event.target.value,
          });
        }}
        className="h-7 rounded-md border border-[#2b2b2d] bg-[#171719] px-2 text-[11px] text-neutral-200 outline-none transition focus:border-neutral-500 disabled:cursor-default disabled:opacity-70"
      >
        {(action.options || []).map((option) => (
          <option key={option.value} value={option.value}>
            {translate(option.label) || option.value}
          </option>
        ))}
      </select>
    );
  }

  if (action.variant === 'button') {
    const Icon = action.icon ? resolveLucideIcon(action.icon) : null;
    return (
      <button
        key={action.id}
        type="button"
        disabled={schemaDisabled}
        onClick={() => {
          void dataContext.dispatch(action.id, {
            dataSource: action.dataSource || dataSource,
            item,
          });
        }}
        className="inline-flex h-7 items-center gap-1.5 rounded-md border border-[#2b2b2d] px-2 text-[12px] text-neutral-300 transition hover:bg-[#202022] disabled:cursor-default disabled:opacity-50 disabled:hover:bg-transparent"
      >
        {Icon && <Icon className="h-3.5 w-3.5" />}
        {translate(action.label)}
      </button>
    );
  }

  return null;
}

const renderPropertyValue = (item: unknown, property: SettingsUiProperty) => {
  const value = textItemValue(item, property.value);
  if (!value) return null;
  if (property.variant === 'badge' || property.variant === 'status') {
    return <DataSourceBadge>{value}</DataSourceBadge>;
  }
  if (property.variant === 'code') {
    return <span className="min-w-0 truncate font-mono text-[11px] text-neutral-200">{value}</span>;
  }
  return <span className="min-w-0 truncate text-[11px] text-neutral-300">{value}</span>;
};

export function DataSourceProperties({
  item,
  properties,
  translate,
}: {
  item: unknown;
  properties?: SettingsUiProperty[];
  translate: (label?: string) => string;
}) {
  const visibleProperties = (properties || []).filter((property) =>
    itemConditionsMatch(property.visibleWhen, item)
  );
  if (visibleProperties.length === 0) return null;
  return (
    <dl className="grid grid-cols-1 gap-2 sm:grid-cols-2">
      {visibleProperties.map((property) => {
        const value = renderPropertyValue(item, property);
        if (!value) return null;
        return (
          <div
            key={property.id}
            className="min-w-0 rounded-md border border-[#2b2b2d] bg-[#1a1b1d]/30 px-2.5 py-2"
          >
            {property.label && (
              <dt className="mb-1 truncate text-[10px] font-medium uppercase tracking-wider text-neutral-500">
                {translate(property.label)}
              </dt>
            )}
            <dd className="min-w-0">{value}</dd>
          </div>
        );
      })}
    </dl>
  );
}

export function DataSourceSearchInput({
  value,
  disabled,
  placeholder,
  onChange,
}: {
  value: string;
  disabled?: boolean;
  placeholder?: string;
  onChange: (value: string) => void;
}) {
  return (
    <div className="relative">
      <Search className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-neutral-500" />
      <input
        value={value}
        disabled={disabled}
        onChange={(event) => onChange(event.target.value)}
        placeholder={placeholder}
        className="h-7 w-48 rounded-md border border-[#2b2b2d] bg-[#171719] pl-7 pr-2 text-[12px] text-neutral-200 outline-none transition placeholder:text-neutral-600 focus:border-neutral-500 disabled:cursor-default disabled:opacity-50"
      />
    </div>
  );
}
