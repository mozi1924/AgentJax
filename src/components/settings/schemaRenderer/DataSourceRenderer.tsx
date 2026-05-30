import { LoaderCircle, RefreshCcw, Search } from 'lucide-react';
import type { ReactElement } from 'react';
import { useI18n } from '../../../features/i18n';
import type { SettingsSchemaNode, SettingsUiAction, SettingsUiSchemaNode } from '../../../features/settings/types';
import type { SchemaRendererDataContext, SchemaRendererProps } from './types';
import { OverlayScrollArea } from '../../OverlayScrollArea';

const asRecord = (value: unknown): Record<string, unknown> =>
  value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};

const asArray = (value: unknown): Record<string, unknown>[] =>
  Array.isArray(value) ? value.map(asRecord) : [];

const resolveTemplate = (node: SettingsUiSchemaNode) =>
  node.itemTemplate && 'bindings' in node.itemTemplate
    ? (node.itemTemplate as SettingsUiSchemaNode)
    : undefined;

const getPath = (root: unknown, path?: string): unknown => {
  if (!path) return undefined;
  return path.split('.').reduce<unknown>((current, segment) => {
    if (!current || typeof current !== 'object') return undefined;
    return (current as Record<string, unknown>)[segment];
  }, root);
};

const textValue = (root: unknown, path?: string) => {
  const value = getPath(root, path);
  if (value === null || value === undefined) return '';
  return String(value);
};

const boolValue = (root: unknown, path?: string) => !!getPath(root, path);

const renderMeta = (item: unknown, bindings?: Record<string, string>) => {
  const values = [bindings?.meta, bindings?.secondaryMeta, bindings?.count]
    .map((path) => textValue(item, path))
    .filter(Boolean);
  return values.join(' · ');
};

const classNames = (...values: Array<string | false | null | undefined>) =>
  values.filter(Boolean).join(' ');

function DataSwitch({
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
      className={`relative inline-flex h-5 w-9 items-center ${
        disabled ? 'cursor-not-allowed opacity-60' : 'cursor-pointer'
      }`}
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

function renderActionControl({
  action,
  item,
  dataContext,
  translate,
}: {
  action: SettingsUiAction;
  item: unknown;
  dataContext: SchemaRendererDataContext;
  translate: (label?: string) => string;
}) {
  if (action.variant === 'switch') {
    const checked = boolValue(item, action.value);
    const savingKey = textValue(item, action.savingKey);
    const disabled = !textValue(item, action.path);
    return (
      <DataSwitch
        key={action.id}
        checked={checked}
        disabled={disabled || dataContext.isSaving?.(savingKey)}
        loading={dataContext.isSaving?.(savingKey)}
        onChange={(nextChecked) => {
          void dataContext.dispatch(action.id, {
            item,
            path: textValue(item, action.path),
            value: nextChecked,
          });
        }}
      />
    );
  }

  if (action.variant === 'segmented') {
    const currentValue = textValue(item, action.value);
    const savingKey = textValue(item, action.savingKey);
    return (
      <div key={action.id} className="inline-flex h-7 overflow-hidden rounded-md border border-[#2b2b2d] bg-[#171719]">
        {(action.options || []).map((option) => (
          <button
            key={option.value}
            type="button"
            disabled={currentValue === option.value || dataContext.isSaving?.(savingKey)}
            onClick={() => {
              void dataContext.dispatch(action.id, {
                item,
                path: textValue(item, action.path),
                value: option.value,
              });
            }}
            className={`px-2.5 text-[11px] transition ${
              currentValue === option.value
                ? 'bg-[#2a2a2c] text-white'
                : 'text-neutral-400 hover:bg-[#202022] hover:text-white'
            } disabled:cursor-default disabled:opacity-70`}
          >
            {translate(option.label) || option.value}
          </button>
        ))}
      </div>
    );
  }

  if (action.variant === 'button') {
    return (
      <button
        key={action.id}
        type="button"
        onClick={() => {
          void dataContext.dispatch(action.id, { item });
        }}
        className="inline-flex h-7 items-center gap-1.5 rounded-md border border-[#2b2b2d] px-2 text-[12px] text-neutral-300 transition hover:bg-[#202022]"
      >
        {action.icon === 'RefreshCcw' && <RefreshCcw className="h-3.5 w-3.5" />}
        {translate(action.label)}
      </button>
    );
  }

  return null;
}

export function DataSourceRenderer({
  node,
  dataContext,
  renderChildren,
}: {
  node: SettingsUiSchemaNode;
  dataContext: SchemaRendererDataContext;
  renderChildren: (
    nodes: SettingsSchemaNode[],
    contextPath?: string,
    options?: { container?: SchemaRendererProps['container'] }
  ) => ReactElement;
}) {
  const { t } = useI18n();
  const translate = (label?: string) => (label ? t(label) : '');
  const data = dataContext.getDataSource(node.dataSource);
  const template = resolveTemplate(node);
  const bindings = template?.bindings || node.bindings || {};

  if (node.kind === 'layout' && node.dataSource) {
    if (node.variant === 'workbench') {
      return (
        <div className="settings-schema-workbench flex min-h-0 flex-col">
          {node.children ? renderChildren(node.children, undefined, { container: 'fragment' }) : null}
        </div>
      );
    }
    return <>{node.children ? renderChildren(node.children) : null}</>;
  }

  if (node.kind === 'split' && node.layout === 'three-pane') {
    return (
      <div className="settings-schema-split-three-pane">
        {node.children ? renderChildren(node.children, undefined, { container: 'fragment' }) : null}
      </div>
    );
  }

  if (node.kind === 'tabs') {
    const items = asArray(data);
    const activeState = dataContext.getDataSource(node.bindings?.activeSource);
    const activeId = textValue(activeState, node.bindings?.activeKey);
    return (
      <div className="flex flex-wrap items-center gap-1 border-b border-[#242426] px-6 py-2">
        {items.map((item) => {
          const itemId = textValue(item, bindings.id || 'id');
          const label = textValue(item, bindings.label || 'label');
          return (
            <button
              key={itemId}
              type="button"
              onClick={() => void dataContext.dispatch(node.action || 'selectTab', { item, value: itemId })}
              className={`rounded-md px-2.5 py-1 text-[12px] transition ${
                activeId === itemId
                  ? 'bg-[#2a2a2c] text-white'
                  : 'text-neutral-400 hover:bg-[#202022] hover:text-white'
              }`}
            >
              {label ? t(label) : itemId}
            </button>
          );
        })}
      </div>
    );
  }

  if (node.kind === 'toolbar') {
    const record = asRecord(data);
    const title = textValue(record, bindings.title);
    const description = textValue(record, bindings.description);
    const actionError = textValue(record, bindings.error);
    const searchAction = node.actions?.find((action) => action.variant === 'search');
    const search = textValue(
      dataContext.getDataSource(searchAction?.dataSource || node.bindings?.querySource),
      searchAction?.value || 'search'
    );
    return (
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-[#242426]/50 px-6 py-2">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            {title && <h5 className="truncate text-[13px] font-medium text-neutral-100">{title}</h5>}
            {node.actions?.filter((action) => action.variant !== 'search' && action.variant !== 'button').map((action) =>
              renderActionControl({ action, item: record, dataContext, translate })
            )}
          </div>
          {description && <p className="mt-0.5 text-[11px] text-neutral-500">{description}</p>}
          {actionError && <p className="mt-0.5 truncate text-[11px] text-rose-300">{actionError}</p>}
        </div>
        <div className="flex min-w-0 shrink-0 flex-wrap items-center gap-2">
          {node.actions?.filter((action) => action.variant === 'search').map((action) => (
            <div key={action.id} className="relative">
              <Search className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-neutral-500" />
              <input
                value={search}
                onChange={(event) => {
                  void dataContext.dispatch(action.id, { value: event.target.value });
                }}
                placeholder={action.label ? t(action.label) : ''}
                className="h-7 w-48 rounded-md border border-[#2b2b2d] bg-[#171719] pl-7 pr-2 text-[12px] text-neutral-200 outline-none transition placeholder:text-neutral-600 focus:border-neutral-500"
              />
            </div>
          ))}
          {node.actions?.filter((action) => action.variant === 'button').map((action) =>
            renderActionControl({ action, item: record, dataContext, translate })
          )}
        </div>
      </div>
    );
  }

  if (node.kind === 'panel') {
    const record = asRecord(data);
    if (!record || Object.keys(record).length === 0) {
      return (
        <section className="min-w-0 border-t border-[#242426]/50 xl:border-t-0">
          <div className="flex h-full items-center justify-center text-xs text-neutral-500">
            {node.emptyText ? t(node.emptyText) : ''}
          </div>
        </section>
      );
    }
    return (
      <section className="min-w-0 border-t border-[#242426]/50 xl:border-t-0">
        <div className="flex h-full min-h-0 flex-col">
          {node.children ? renderChildren(node.children, undefined, { container: 'fragment' }) : null}
        </div>
      </section>
    );
  }

  if (node.kind === 'list') {
    const items = asArray(data);
    const containerClass = classNames(
      'min-h-0 min-w-0 flex flex-col',
      node.variant === 'sidebar' && 'border-r border-[#242426]/50 bg-[#171719]/40',
      node.variant === 'content' && 'border-r border-[#242426]/50'
    );
    return (
      <section className={containerClass}>
        <OverlayScrollArea
          containerClassName="flex-1 min-h-0"
          className={classNames(
            "min-h-[220px] flex-1 p-2 xl:min-h-0",
            node.variant === 'sidebar' && 'pl-6 pr-2 py-2 xl:pl-6'
          )}
        >
          {items.length === 0 ? (
            <p className="px-2 py-6 text-center text-xs text-neutral-500">
              {node.emptyText ? t(node.emptyText) : ''}
            </p>
          ) : (
            <div className="space-y-1">
              {items.map((item) => {
                const key = textValue(item, bindings.id || 'id');
                const activeKey = textValue(item, bindings.activeKey);
                const active = activeKey && activeKey === key;
                const title = textValue(item, bindings.title);
                const description = textValue(item, bindings.description);
                const meta = renderMeta(item, bindings);
                return (
                  <div
                    key={key}
                    onClick={() => void dataContext.dispatch(node.action || 'selectItem', { item, value: key })}
                    className={`w-full rounded-md border px-2.5 py-2 text-left transition cursor-pointer select-none ${
                      active
                        ? 'border-neutral-500 bg-[#2a2a2c]/60 text-white'
                        : 'border-transparent text-neutral-400 hover:bg-[#202022] hover:text-white'
                    }`}
                  >
                    <div className="grid grid-cols-[minmax(0,1fr)_auto] items-start gap-3">
                      <div className="min-w-0">
                        {title && <div className="truncate text-[12.5px] font-medium text-neutral-100">{title}</div>}
                        {description && <p className="mt-1 line-clamp-2 text-[11.5px] leading-relaxed text-neutral-400">{description}</p>}
                        {meta && <div className="mt-1 truncate text-[10px] text-neutral-500">{meta}</div>}
                      </div>
                      {template?.actions?.length ? (
                        <div className="flex w-10 shrink-0 justify-end pt-0.5">
                          {template.actions.map((action) =>
                            renderActionControl({ action, item, dataContext, translate })
                          )}
                        </div>
                      ) : null}
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </OverlayScrollArea>
      </section>
    );
  }

  if (node.kind === 'detail') {
    const record = asRecord(data);
    if (!record || Object.keys(record).length === 0) {
      return (
        <section className="min-w-0 border-t border-[#242426]/50 bg-[#171719]/20 xl:border-l xl:border-t-0">
          <div className="flex h-40 items-center justify-center text-xs text-neutral-500 xl:h-full">
            {node.emptyText ? t(node.emptyText) : ''}
          </div>
        </section>
      );
    }
    const title = textValue(record, bindings.title);
    const description = textValue(record, bindings.description);
    const meta = renderMeta(record, bindings);
    const schemaProperties = bindings.schemaProperties
      ? asArray(getPath(record, bindings.schemaProperties))
      : [];
    const schemaTitle = bindings.schemaTitle ? t(bindings.schemaTitle) : '';
    const schemaEmptyText = bindings.schemaEmptyText ? t(bindings.schemaEmptyText) : '';
    const requiredLabel = bindings.requiredLabel ? t(bindings.requiredLabel) : 'required';
    return (
      <section className="min-w-0 border-t border-[#242426]/50 bg-[#171719]/20 xl:border-l xl:border-t-0 flex flex-col h-full">
        <OverlayScrollArea
          containerClassName="flex-1 min-h-0"
          className="h-[min(52vh,520px)] xl:h-full p-3 pr-6"
        >
          <div className="space-y-3">
            <div>
              <div className="flex flex-wrap items-center gap-2">
                {title && <h5 className="min-w-0 flex-1 truncate text-[13px] font-medium text-neutral-100">{title}</h5>}
                {textValue(record, bindings.badge) && (
                  <span className="rounded bg-[#2a2a2c] px-1.5 py-0.5 text-[10px] text-neutral-400">
                    {textValue(record, bindings.badge)}
                  </span>
                )}
              </div>
              {description && <p className="mt-1 text-[11.5px] leading-relaxed text-neutral-400">{description}</p>}
              {meta && <div className="mt-2 text-[10.5px] text-neutral-500">{meta}</div>}
            </div>
            {template?.actions?.length ? (
              <div className="rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 px-3 py-2">
                <div className="flex items-center justify-between gap-3">
                  {bindings.actionsTitle && (
                    <span className="text-[12px] font-medium text-neutral-200">
                      {t(bindings.actionsTitle)}
                    </span>
                  )}
                  <div className="flex items-center gap-2">
                    {template.actions.map((action) =>
                      renderActionControl({ action, item: record, dataContext, translate })
                    )}
                  </div>
                </div>
              </div>
            ) : null}
            {bindings.schemaProperties && (
              <div>
                {schemaTitle && (
                  <h6 className="mb-2 text-[11px] font-semibold uppercase tracking-wider text-neutral-500">
                    {schemaTitle}
                  </h6>
                )}
                {schemaProperties.length === 0 ? (
                  <div className="rounded-lg border border-dashed border-[#2b2b2d] px-3 py-4 text-center text-[11px] text-neutral-500">
                    {schemaEmptyText}
                  </div>
                ) : (
                  <div className="space-y-2">
                    {schemaProperties.map((property) => (
                      <div
                        key={textValue(property, 'name')}
                        className="rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/20 px-3 py-2"
                      >
                        <div className="flex flex-wrap items-center gap-2">
                          <span className="font-mono text-[11px] text-neutral-200">
                            {textValue(property, 'name')}
                          </span>
                          <span className="rounded bg-[#2a2a2c] px-1.5 py-0.5 text-[10px] text-neutral-300">
                            {textValue(property, 'type')}
                          </span>
                          {boolValue(property, 'required') && (
                            <span className="rounded bg-amber-500/10 px-1.5 py-0.5 text-[10px] text-amber-200">
                              {requiredLabel}
                            </span>
                          )}
                        </div>
                        {textValue(property, 'description') && (
                          <p className="mt-1 line-clamp-3 text-[11px] leading-relaxed text-neutral-500">
                            {textValue(property, 'description')}
                          </p>
                        )}
                      </div>
                    ))}
                  </div>
                )}
              </div>
            )}
          </div>
        </OverlayScrollArea>
      </section>
    );
  }

  return null;
}
