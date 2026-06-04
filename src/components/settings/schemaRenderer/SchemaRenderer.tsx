import { Fragment, useMemo } from 'react';
import { AlertTriangle } from 'lucide-react';
import { isNodeVisible } from '../../../features/settings/utils';
import type { SettingsSchemaNode, SettingsUiSchemaNode } from '../../../features/settings/types';
import {
  getSchemaRenderKind,
  shouldUseDataContextRenderer,
} from '../../../features/settings/schemaRendererView';
import { FieldRenderer } from './FieldControlRegistry';
import { DataSourceRenderer } from './DataSourceRenderer';
import { CollectionLayoutRenderer, GroupRenderer, UiLayoutRenderer } from './LayoutRenderer';
import { SettingsErrorBoundary } from './SettingsErrorBoundary';
import { safeValidateNode } from './validateSettingsSchema';
import type { SchemaRendererProps } from './types';
import type { NodeListProps } from '../renderer/types';

/** Fallback shown when a single schema node fails validation. */
function InvalidNodeFallback({ nodeId, kind }: { nodeId: string; kind: string }) {
  return (
    <div className="rounded-lg border border-amber-500/15 bg-amber-950/5 px-3 py-2">
      <div className="flex items-start gap-2">
        <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0 text-amber-400" />
        <div className="min-w-0">
          <p className="text-[11px] font-medium text-amber-300">Invalid schema node</p>
          <p className="mt-0.5 truncate text-[10px] text-amber-400/60">
            id=&ldquo;{nodeId}&rdquo; kind=&ldquo;{kind}&rdquo;
          </p>
        </div>
      </div>
    </div>
  );
}

/** Wrap a single rendered node so a crash in one does not break the whole section. */
function SafeNodeRenderer({
  node,
  contextLabel,
  children,
}: {
  node: SettingsSchemaNode;
  contextLabel?: string;
  children: React.ReactNode;
}) {
  return (
    <SettingsErrorBoundary contextLabel={`${contextLabel ? `${contextLabel} > ` : ''}${node.kind} "${node.id}"`}>
      {children}
    </SettingsErrorBoundary>
  );
}

// Recursive UI schema dispatcher. V1 field/group/collection nodes are handled as compatibility nodes.
export function SchemaRenderer(props: SchemaRendererProps) {
  const visibleNodes = useMemo(
    () => props.nodes.filter((node) => isNodeVisible(node, props.snapshot, props.contextPath)),
    [props.contextPath, props.nodes, props.snapshot]
  );

  const renderChildren = (
    nodes: SettingsSchemaNode[],
    contextPath?: string,
    options?: { container?: SchemaRendererProps['container'] }
  ) => (
    <SchemaRenderer
      {...props}
      nodes={nodes}
      contextPath={contextPath ?? props.contextPath}
      container={options?.container ?? 'stack'}
    />
  );

  const renderNodeList = (nodeListProps: NodeListProps) => (
    <SchemaRenderer
      nodes={nodeListProps.nodes}
      snapshot={nodeListProps.snapshot}
      savingPath={nodeListProps.savingPath}
      fieldErrors={nodeListProps.fieldErrors}
      contextPath={nodeListProps.contextPath}
      actions={{
        saveField: nodeListProps.onSaveField,
        deletePath: nodeListProps.onDeletePath,
        addCollectionItem: nodeListProps.onAddCollectionItem,
      }}
      queryState={props.queryState}
      dataContext={props.dataContext}
    />
  );

  const renderedNodes = visibleNodes.map((node) => {
        const key = `${props.contextPath || 'root'}:${node.kind}:${node.id}`;
        const contextLabel = props.contextPath || 'root';

        // Validate node before rendering — catch structural issues early.
        const validated = safeValidateNode(node, contextLabel);
        if (!validated) {
          return (
            <div key={key} className="flex flex-col">
              <InvalidNodeFallback nodeId={node.id} kind={node.kind} />
            </div>
          );
        }

        const renderKind = getSchemaRenderKind(validated);

        if (renderKind === 'field' && validated.kind === 'field') {
          const content = (
            <div key={key} className="flex flex-col">
              <FieldRenderer
                field={validated}
                snapshot={props.snapshot}
                savingPath={props.savingPath}
                fieldErrors={props.fieldErrors}
                contextPath={props.contextPath}
                actions={props.actions}
              />
            </div>
          );
          return <SafeNodeRenderer key={key} node={validated} contextLabel={contextLabel}>{content}</SafeNodeRenderer>;
        }

        if (renderKind === 'group' && validated.kind === 'group') {
          const content = (
            <GroupRenderer
              key={key}
              node={validated}
              contextPath={props.contextPath}
              renderChildren={renderChildren}
            />
          );
          return <SafeNodeRenderer key={key} node={validated} contextLabel={contextLabel}>{content}</SafeNodeRenderer>;
        }

        if (renderKind === 'collection' && validated.kind === 'collection') {
          const content = (
            <CollectionLayoutRenderer
              key={key}
              node={validated}
              props={{
                snapshot: props.snapshot,
                savingPath: props.savingPath,
                fieldErrors: props.fieldErrors,
                contextPath: props.contextPath,
                onSaveField: props.actions.saveField,
                onDeletePath: props.actions.deletePath || (() => Promise.resolve()),
                onAddCollectionItem:
                  props.actions.addCollectionItem || (() => Promise.resolve()),
              }}
              renderNodeList={renderNodeList}
            />
          );
          return <SafeNodeRenderer key={key} node={validated} contextLabel={contextLabel}>{content}</SafeNodeRenderer>;
        }

        const uiNode = validated as SettingsUiSchemaNode;
        if (props.dataContext && shouldUseDataContextRenderer(uiNode)) {
          const content = (
            <Fragment key={key}>
              <DataSourceRenderer
                node={uiNode}
                dataContext={props.dataContext}
                renderChildren={renderChildren}
              />
            </Fragment>
          );
          return <SafeNodeRenderer key={key} node={validated} contextLabel={contextLabel}>{content}</SafeNodeRenderer>;
        }

        const content = (
          <Fragment key={key}>
            <UiLayoutRenderer
              node={uiNode}
              actions={props.actions}
              contextPath={props.contextPath}
              renderChildren={renderChildren}
            />
          </Fragment>
        );
        return <SafeNodeRenderer key={key} node={validated} contextLabel={contextLabel}>{content}</SafeNodeRenderer>;
      });

  if (props.container === 'fragment') {
    return <>{renderedNodes}</>;
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col space-y-4">
      {renderedNodes}
    </div>
  );
}
