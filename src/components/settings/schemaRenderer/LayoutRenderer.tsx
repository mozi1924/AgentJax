import { useState } from 'react';
import type { ReactElement } from 'react';
import { ChevronRight } from 'lucide-react';
import { useI18n } from '../../../features/i18n';
import type { SettingsGroupSchema, SettingsSchemaNode, SettingsUiSchemaNode } from '../../../features/settings/types';
import { CollectionEditor } from '../renderer/CollectionEditor';
import type { NodeListProps } from '../renderer/types';
import { ActionRenderer } from './ActionRenderer';
import { DataSourceBadge } from './dataSources/ui';
import { settingsUiLayoutClassName } from './layoutClasses';
import type { SchemaRendererProps } from './types';

type RenderChildren = (
  nodes: SettingsSchemaNode[],
  contextPath?: string,
  options?: { container?: SchemaRendererProps['container'] }
) => ReactElement;

export function GroupRenderer({
  node,
  renderChildren,
  contextPath,
}: {
  node: SettingsGroupSchema;
  renderChildren: RenderChildren;
  contextPath?: string;
}) {
  const { t } = useI18n();

  return (
    <section className="flex min-h-0 flex-1 flex-col space-y-2.5 pt-2">
      <div className="mb-1 mt-3 shrink-0 first:mt-0">
        <h5 className="text-[10px] font-semibold uppercase tracking-wider text-neutral-500">
          {t(node.title)}
        </h5>
        {node.description && (
          <p className="mt-0.5 text-[11px] text-neutral-400/70">{t(node.description)}</p>
        )}
      </div>
      <div className="flex min-h-0 flex-1 flex-col border-t border-[#242426]/30 pt-1">
        {renderChildren(node.children, contextPath)}
      </div>
    </section>
  );
}

export function UiLayoutRenderer({
  node,
  actions,
  renderChildren,
  contextPath,
}: {
  node: SettingsUiSchemaNode;
  actions: SchemaRendererProps['actions'];
  renderChildren: RenderChildren;
  contextPath?: string;
}) {
  const { t } = useI18n();
  const [activeTab, setActiveTab] = useState(node.tabs?.[0]?.id || '');
  const [expanded, setExpanded] = useState(node.defaultExpanded ?? true);

  if (node.kind === 'action') {
    return <ActionRenderer node={node} actions={actions} />;
  }

  if (node.kind === 'toolbar') {
    return (
      <div className="flex flex-wrap items-center gap-2 rounded-lg border border-[#242426]/60 bg-[#141516] px-3 py-2">
        {node.title ? <span className="mr-auto text-xs font-medium text-neutral-300">{t(node.title)}</span> : null}
        {node.actions?.map((action) => (
          <ActionRenderer key={action.id} action={action} actions={actions} />
        ))}
        {node.children ? renderChildren(node.children, contextPath) : null}
      </div>
    );
  }

  if (node.kind === 'tabs' && node.tabs?.length) {
    const selectedTab = node.tabs.find((tab) => tab.id === activeTab) || node.tabs[0];
    return (
      <section className="min-h-0 space-y-3">
        <div className="flex flex-wrap items-center gap-1 border-b border-[#242426] pb-2">
          {node.tabs.map((tab) => (
            <button
              key={tab.id}
              type="button"
              onClick={() => setActiveTab(tab.id)}
              className={`rounded-md px-2.5 py-1 text-[12px] transition ${
                selectedTab.id === tab.id
                  ? 'bg-[#2a2a2c] text-white'
                  : 'text-neutral-400 hover:bg-[#202022] hover:text-white'
              }`}
            >
              {t(tab.title)}
            </button>
          ))}
        </div>
        {renderChildren(selectedTab.children, contextPath)}
      </section>
    );
  }

  const children = node.children ? renderChildren(node.children, contextPath) : null;

  if (node.kind === 'layout') {
    return (
      <section className={settingsUiLayoutClassName(node.layout, node.variant)}>
        {children}
      </section>
    );
  }

  if (node.kind === 'collapsible') {
    return (
      <section className="min-h-0 rounded-lg border border-[#242426]/70 bg-[#141516]/60">
        <button
          type="button"
          onClick={() => setExpanded((current) => !current)}
          className="flex w-full items-center gap-2 px-3 py-2 text-left transition hover:bg-[#202022]/70"
        >
          <ChevronRight
            className={`h-3.5 w-3.5 shrink-0 text-neutral-500 transition-transform ${
              expanded ? 'rotate-90' : ''
            }`}
          />
          <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-neutral-100">
            {node.title ? t(node.title) : ''}
          </span>
        </button>
        {node.description && expanded ? (
          <p className="px-3 pb-2 text-[11px] text-neutral-500">{t(node.description)}</p>
        ) : null}
        {expanded ? <div className="border-t border-[#242426]/50 p-3">{children}</div> : null}
      </section>
    );
  }

  if (node.kind === 'panel' || node.kind === 'detail') {
    return (
      <section className="min-h-0 rounded-lg border border-[#242426]/70 bg-[#141516]/60 p-3">
        {node.title ? <h5 className="mb-1 text-[13px] font-medium text-neutral-100">{t(node.title)}</h5> : null}
        {node.description ? <p className="mb-3 text-[11px] text-neutral-500">{t(node.description)}</p> : null}
        {children}
      </section>
    );
  }

  if (node.kind === 'empty_state') {
    return (
      <div className="rounded-lg border border-dashed border-[#242426] px-4 py-6 text-center">
        {node.title ? <div className="text-xs font-medium text-neutral-300">{t(node.title)}</div> : null}
        {node.description ? (
          <p className="mx-auto mt-1 max-w-md text-[11px] leading-relaxed text-neutral-500">
            {t(node.description)}
          </p>
        ) : null}
      </div>
    );
  }

  if (node.kind === 'badge') {
    const value = node.title ? t(node.title) : String(node.value ?? '');
    return value ? <DataSourceBadge mono={node.variant === 'code'}>{value}</DataSourceBadge> : null;
  }

  if (node.kind === 'metric') {
    const value = node.value === undefined || node.value === null ? '' : String(node.value);
    return (
      <div className="min-w-0 rounded-lg border border-[#2b2b2d] bg-[#1a1b1d]/30 px-3 py-2.5">
        {node.title ? (
          <div className="truncate text-[10px] font-medium uppercase tracking-wider text-neutral-500">
            {t(node.title)}
          </div>
        ) : null}
        <div className="mt-1 truncate text-[18px] font-semibold text-neutral-100">
          {value}
        </div>
        {node.description ? (
          <p className="mt-1 line-clamp-2 text-[11px] leading-relaxed text-neutral-500">
            {t(node.description)}
          </p>
        ) : null}
      </div>
    );
  }

  return <section className="min-h-0">{children}</section>;
}

export function CollectionLayoutRenderer({
  node,
  props,
  renderNodeList,
}: {
  node: Extract<SettingsSchemaNode, { kind: 'collection' }>;
  props: Omit<NodeListProps, 'nodes'>;
  renderNodeList: (props: NodeListProps) => ReactElement;
}) {
  return (
    <section className="pt-2">
      <CollectionEditor
        collection={node}
        snapshot={props.snapshot}
        savingPath={props.savingPath}
        fieldErrors={props.fieldErrors}
        contextPath={props.contextPath}
        onSaveField={props.onSaveField}
        onDeletePath={props.onDeletePath}
        onAddCollectionItem={props.onAddCollectionItem}
        renderNodeList={renderNodeList}
      />
    </section>
  );
}
