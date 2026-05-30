import { Fragment, useMemo } from 'react';
import { isNodeVisible } from '../../../features/settings/utils';
import type { SettingsSchemaNode, SettingsUiSchemaNode } from '../../../features/settings/types';
import { getSchemaRenderKind } from '../../../features/settings/schemaRendererView';
import { FieldRenderer } from './FieldControlRegistry';
import { CollectionLayoutRenderer, GroupRenderer, UiLayoutRenderer } from './LayoutRenderer';
import type { SchemaRendererProps } from './types';
import type { NodeListProps } from '../renderer/types';

// Recursive UI schema dispatcher. V1 field/group/collection nodes are handled as compatibility nodes.
export function SchemaRenderer(props: SchemaRendererProps) {
  const visibleNodes = useMemo(
    () => props.nodes.filter((node) => isNodeVisible(node, props.snapshot, props.contextPath)),
    [props.contextPath, props.nodes, props.snapshot]
  );

  const renderChildren = (nodes: SettingsSchemaNode[], contextPath?: string) => (
    <SchemaRenderer {...props} nodes={nodes} contextPath={contextPath ?? props.contextPath} />
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
      renderUiNode={props.renderUiNode}
    />
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col space-y-4">
      {visibleNodes.map((node) => {
        const key = `${props.contextPath || 'root'}:${node.kind}:${node.id}`;

        const renderKind = getSchemaRenderKind(node);

        if (renderKind === 'field' && node.kind === 'field') {
          return (
            <div key={key} className="flex min-h-0 flex-1 flex-col">
              <FieldRenderer
                field={node}
                snapshot={props.snapshot}
                savingPath={props.savingPath}
                fieldErrors={props.fieldErrors}
                contextPath={props.contextPath}
                actions={props.actions}
              />
            </div>
          );
        }

        if (renderKind === 'group' && node.kind === 'group') {
          return (
            <GroupRenderer
              key={key}
              node={node}
              contextPath={props.contextPath}
              renderChildren={renderChildren}
            />
          );
        }

        if (renderKind === 'collection' && node.kind === 'collection') {
          return (
            <CollectionLayoutRenderer
              key={key}
              node={node}
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
        }

        const uiNode = node as SettingsUiSchemaNode;
        const defaultRender = () => (
          <UiLayoutRenderer
            node={uiNode}
            actions={props.actions}
            contextPath={props.contextPath}
            renderChildren={renderChildren}
          />
        );
        const customRender = props.renderUiNode?.({
          node: uiNode,
          defaultRender,
          renderChildren,
        });
        if (customRender !== undefined && customRender !== null) {
          return <Fragment key={key}>{customRender}</Fragment>;
        }

        return (
          <Fragment key={key}>
            {defaultRender()}
          </Fragment>
        );
      })}
    </div>
  );
}
