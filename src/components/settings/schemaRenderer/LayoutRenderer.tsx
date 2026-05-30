import { useState } from 'react';
import type { ReactElement } from 'react';
import { useI18n } from '../../../features/i18n';
import type { SettingsGroupSchema, SettingsSchemaNode, SettingsUiSchemaNode } from '../../../features/settings/types';
import { CollectionEditor } from '../renderer/CollectionEditor';
import type { NodeListProps } from '../renderer/types';
import { ActionRenderer } from './ActionRenderer';
import type { SchemaRendererProps } from './types';

type RenderChildren = (nodes: SettingsSchemaNode[], contextPath?: string) => ReactElement;

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
      <div className="rounded-lg border border-dashed border-[#242426] px-4 py-6 text-center text-xs text-neutral-500">
        {node.title ? t(node.title) : null}
      </div>
    );
  }

  if (node.kind === 'badge' || node.kind === 'metric') {
    return (
      <span className="inline-flex w-fit rounded bg-[#24262a] px-1.5 py-0.5 text-[10px] text-neutral-400">
        {node.title ? t(node.title) : String(node.value ?? '')}
      </span>
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
