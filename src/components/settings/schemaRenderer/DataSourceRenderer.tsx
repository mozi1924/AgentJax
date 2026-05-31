import { useState } from 'react';
import { AlertCircle, ChevronRight, LoaderCircle } from 'lucide-react';
import type { ReactElement } from 'react';
import { useI18n } from '../../../features/i18n';
import type {
  SettingsSchemaNode,
  SettingsUiAction,
  SettingsUiSchemaNode,
} from '../../../features/settings/types';
import type { SchemaRendererDataContext, SchemaRendererProps } from './types';
import { OverlayScrollArea } from '../../OverlayScrollArea';
import {
  boolItemValue,
  getItemPath,
  textItemValue,
} from './dataSources/conditions';
import {
  settingsUiLayoutClassName,
  settingsUiSplitClassName,
} from './layoutClasses';
import {
  classNames,
  dataSourceActionDisabled,
  dataSourceActionVisible,
  DataSourceActionControl,
  DataSourceBadge,
  DataSourceProperties,
  DataSourceSearchInput,
} from './dataSources/ui';

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

const renderMeta = (item: unknown, bindings?: Record<string, string>) => {
  const values = [bindings?.meta, bindings?.secondaryMeta, bindings?.count]
    .map((path) => textItemValue(item, path))
    .filter(Boolean);
  return values.join(' · ');
};

const nodeBoundText = (
  item: unknown,
  path: string | undefined,
  fallback: unknown,
  translate: (label?: string) => string
) => {
  const value = textItemValue(item, path);
  if (value) return value;
  if (typeof fallback === 'string') return translate(fallback);
  if (fallback === undefined || fallback === null) return '';
  return String(fallback);
};

const actionFromNode = (node: SettingsUiSchemaNode): SettingsUiAction => ({
  id: node.action || node.id,
  label: node.label || node.title,
  icon: node.icon,
  variant: node.variant || 'button',
});

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
  const status = dataContext.getStatus?.(node.dataSource);
  const template = resolveTemplate(node);
  const [detailItemsExpanded, setDetailItemsExpanded] = useState(
    template?.defaultExpanded ?? true
  );
  const bindings = template?.bindings || node.bindings || {};

  if (node.kind === 'layout') {
    if (node.dataSource && status?.loading) {
      return (
        <div className="flex h-48 items-center justify-center gap-2 text-sm text-neutral-400">
          <LoaderCircle className="h-4 w-4 animate-spin" />
          {status.loadingText ? t(status.loadingText) : ''}
        </div>
      );
    }
    if (node.dataSource && status?.error) {
      return (
        <div className="flex h-48 items-center justify-center gap-2 px-4 text-sm text-rose-300">
          <AlertCircle className="h-4 w-4 shrink-0" />
          <span>
            {status.errorText ? t(status.errorText, { message: status.error }) : status.error}
          </span>
        </div>
      );
    }
    return (
      <div className={settingsUiLayoutClassName(node.layout, node.variant)}>
        {node.children ? renderChildren(node.children, undefined, { container: 'fragment' }) : null}
      </div>
    );
  }

  if (node.kind === 'split') {
    return (
      <div className={settingsUiSplitClassName(node.layout)}>
        {node.children ? renderChildren(node.children, undefined, { container: 'fragment' }) : null}
      </div>
    );
  }

  if (node.kind === 'empty_state') {
    const record = asRecord(data);
    const title = nodeBoundText(record, bindings.title, node.title, translate);
    const description = nodeBoundText(record, bindings.description, node.description, translate);
    return (
      <div className="rounded-lg border border-dashed border-[#242426] px-4 py-6 text-center">
        {title ? <div className="text-xs font-medium text-neutral-300">{title}</div> : null}
        {description ? (
          <p className="mx-auto mt-1 max-w-md text-[11px] leading-relaxed text-neutral-500">
            {description}
          </p>
        ) : null}
      </div>
    );
  }

  if (node.kind === 'badge') {
    const record = asRecord(data);
    const value = nodeBoundText(
      record,
      bindings.value || bindings.title,
      node.value ?? node.title,
      translate
    );
    return value ? <DataSourceBadge mono={node.variant === 'code'}>{value}</DataSourceBadge> : null;
  }

  if (node.kind === 'metric') {
    const record = asRecord(data);
    const label = nodeBoundText(
      record,
      bindings.label || bindings.title,
      node.title || node.label,
      translate
    );
    const value = nodeBoundText(record, bindings.value || 'value', node.value, translate);
    const description = nodeBoundText(record, bindings.description, node.description, translate);
    return (
      <div className="min-w-0 rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/30 px-3 py-2.5">
        {label ? (
          <div className="truncate text-[10px] font-medium uppercase tracking-wider text-neutral-500">
            {label}
          </div>
        ) : null}
        <div className="mt-1 truncate text-[18px] font-semibold text-neutral-100">
          {value}
        </div>
        {description ? (
          <p className="mt-1 line-clamp-2 text-[11px] leading-relaxed text-neutral-500">
            {description}
          </p>
        ) : null}
      </div>
    );
  }

  if (node.kind === 'action') {
    const record = asRecord(data);
    const actions = node.actions && node.actions.length > 0 ? node.actions : [actionFromNode(node)];
    return (
      <div className="flex flex-wrap items-center gap-2">
        {actions.map((action) => (
          <DataSourceActionControl
            key={action.id}
            action={action}
            item={record}
            dataSource={node.dataSource}
            dataContext={dataContext}
            translate={translate}
          />
        ))}
      </div>
    );
  }

  if (node.kind === 'tabs') {
    const items = asArray(data);
    const activeState = dataContext.getDataSource(node.bindings?.activeSource);
    const activeId = textItemValue(activeState, node.bindings?.activeKey);
    return (
      <div className="flex flex-wrap items-center gap-1 border-b border-[#242426] px-6 py-2">
        {items.map((item) => {
          const itemId = textItemValue(item, bindings.id || 'id');
          const label = textItemValue(item, bindings.label || 'label');
          return (
            <button
              key={itemId}
              type="button"
              onClick={() => void dataContext.dispatch(node.action || 'selectTab', { dataSource: node.dataSource, item, value: itemId })}
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
    const title = textItemValue(record, bindings.title);
    const description = textItemValue(record, bindings.description);
    const actionError = textItemValue(record, bindings.error);
    const properties = (
      <DataSourceProperties item={record} properties={node.properties} translate={translate} />
    );
    const actions = (node.actions || []).filter((action) => dataSourceActionVisible(action, record));
    const searchAction = actions.find((action) => action.variant === 'search');
    const search = textItemValue(
      dataContext.getDataSource(searchAction?.dataSource || node.bindings?.querySource),
      searchAction?.value || 'search'
    );
    return (
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-[#242426]/50 px-6 py-2">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            {title && <h5 className="truncate text-[13px] font-medium text-neutral-100">{title}</h5>}
            {actions.filter((action) => action.variant !== 'search' && action.variant !== 'button').map((action) =>
              <DataSourceActionControl
                key={action.id}
                action={action}
                item={record}
                dataSource={node.dataSource}
                dataContext={dataContext}
                translate={translate}
              />
            )}
          </div>
          {description && <p className="mt-0.5 text-[11px] text-neutral-500">{description}</p>}
          {properties && <div className="mt-2">{properties}</div>}
          {actionError && <p className="mt-0.5 truncate text-[11px] text-rose-300">{actionError}</p>}
        </div>
        <div className="flex min-w-0 shrink-0 flex-wrap items-center gap-2">
          {actions.filter((action) => action.variant === 'search').map((action) => (
            <DataSourceSearchInput
              key={action.id}
              value={search}
              disabled={dataSourceActionDisabled(action, record)}
              placeholder={action.label ? t(action.label) : ''}
              onChange={(value) => {
                void dataContext.dispatch(action.id, {
                  dataSource: action.dataSource || node.dataSource,
                  value,
                });
              }}
            />
          ))}
          {actions.filter((action) => action.variant === 'button').map((action) =>
            <DataSourceActionControl
              key={action.id}
              action={action}
              item={record}
              dataSource={node.dataSource}
              dataContext={dataContext}
              translate={translate}
            />
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
                const key = textItemValue(item, bindings.id || 'id');
                const activeKey = textItemValue(item, bindings.activeKey);
                const active = activeKey && activeKey === key;
                const title = textItemValue(item, bindings.title);
                const description = textItemValue(item, bindings.description);
                const meta = renderMeta(item, bindings);
                return (
                  <div
                    key={key}
                    onClick={() => void dataContext.dispatch(node.action || 'selectItem', { dataSource: node.dataSource, item, value: key })}
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
                      {template?.actions?.some((action) => dataSourceActionVisible(action, item)) ? (
                        <div className="flex w-10 shrink-0 justify-end pt-0.5">
                          {template.actions.map((action) =>
                            <DataSourceActionControl
                              key={action.id}
                              action={action}
                              item={item}
                              dataSource={node.dataSource}
                              dataContext={dataContext}
                              translate={translate}
                            />
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
    const title = textItemValue(record, bindings.title);
    const description = textItemValue(record, bindings.description);
    const meta = renderMeta(record, bindings);
    const templateProperties = (
      <DataSourceProperties
        item={record}
        properties={template?.properties}
        translate={translate}
      />
    );
    const detailItemsPath = bindings.detailItems || bindings.schemaProperties;
    const detailItems = detailItemsPath
      ? asArray(getItemPath(record, detailItemsPath))
      : [];
    const detailItemsTitle = bindings.detailItemsTitle || bindings.schemaTitle;
    const detailItemsEmptyText = bindings.detailItemsEmptyText || bindings.schemaEmptyText;
    const detailTitle = detailItemsTitle ? t(detailItemsTitle) : '';
    const detailEmptyText = detailItemsEmptyText ? t(detailItemsEmptyText) : '';
    const detailItemName = bindings.detailItemName || 'name';
    const detailItemType = bindings.detailItemType || 'type';
    const detailItemDescription = bindings.detailItemDescription || 'description';
    const detailItemRequired = bindings.detailItemRequired || 'required';
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
                {textItemValue(record, bindings.badge) && (
                  <DataSourceBadge>{textItemValue(record, bindings.badge)}</DataSourceBadge>
                )}
              </div>
              {description && <p className="mt-1 text-[11.5px] leading-relaxed text-neutral-400">{description}</p>}
              {meta && <div className="mt-2 text-[10.5px] text-neutral-500">{meta}</div>}
            </div>
            {templateProperties}
            {template?.actions?.some((action) => dataSourceActionVisible(action, record)) ? (
              <div className="rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/40 px-3 py-2">
                <div className="flex items-center justify-between gap-3">
                  {bindings.actionsTitle && (
                    <span className="text-[12px] font-medium text-neutral-200">
                      {t(bindings.actionsTitle)}
                    </span>
                  )}
                  <div className="flex items-center gap-2">
                    {template.actions.map((action) =>
                      <DataSourceActionControl
                        key={action.id}
                        action={action}
                        item={record}
                        dataSource={node.dataSource}
                        dataContext={dataContext}
                        translate={translate}
                      />
                    )}
                  </div>
                </div>
              </div>
            ) : null}
            {detailItemsPath && (
              <div>
                <button
                  type="button"
                  onClick={() => setDetailItemsExpanded((current) => !current)}
                  className="mb-2 flex w-full items-center gap-1.5 text-left text-[11px] font-semibold uppercase tracking-wider text-neutral-500 transition hover:text-neutral-300"
                >
                  <ChevronRight
                    className={`h-3 w-3 shrink-0 transition-transform ${
                      detailItemsExpanded ? 'rotate-90' : ''
                    }`}
                  />
                  <span className="min-w-0 flex-1 truncate">{detailTitle}</span>
                </button>
                {detailItemsExpanded ? (
                  detailItems.length === 0 ? (
                    <div className="rounded-lg border border-dashed border-[#2b2b2d] px-3 py-4 text-center text-[11px] text-neutral-500">
                      {detailEmptyText}
                    </div>
                  ) : (
                    <div className="space-y-2">
                      {detailItems.map((property) => (
                        <div
                          key={textItemValue(property, detailItemName)}
                          className="rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/20 px-3 py-2"
                        >
                          <div className="flex flex-wrap items-center gap-2">
                            <span className="font-mono text-[11px] text-neutral-200">
                              {textItemValue(property, detailItemName)}
                            </span>
                            <DataSourceBadge>{textItemValue(property, detailItemType)}</DataSourceBadge>
                            {boolItemValue(property, detailItemRequired) && (
                              <DataSourceBadge tone="warning">{requiredLabel}</DataSourceBadge>
                            )}
                          </div>
                          {textItemValue(property, detailItemDescription) && (
                            <p className="mt-1 line-clamp-3 text-[11px] leading-relaxed text-neutral-500">
                              {textItemValue(property, detailItemDescription)}
                            </p>
                          )}
                        </div>
                      ))}
                    </div>
                  )
                ) : null}
              </div>
            )}
          </div>
        </OverlayScrollArea>
      </section>
    );
  }

  return null;
}
