import { useMemo } from 'react';
import { isNodeVisible } from '../../../features/settings/utils';
import { CollectionEditor } from './CollectionEditor';
import { renderField } from './customFields';
import type { NodeListProps } from './types';

export function NodeList({
  nodes,
  snapshot,
  savingPath,
  fieldErrors,
  contextPath,
  onSaveField,
  onDeletePath,
  onAddCollectionItem,
}: NodeListProps) {
  const visibleNodes = useMemo(
    () => nodes.filter((node) => isNodeVisible(node, snapshot, contextPath)),
    [contextPath, nodes, snapshot]
  );

  return (
    <div className="space-y-4">
      {visibleNodes.map((node) => {
        if (node.kind === 'field') {
          return (
            <div key={`${contextPath || 'root'}:${node.id}`}>
              {renderField({
                field: node,
                snapshot,
                savingPath,
                fieldErrors,
                contextPath,
                onSaveField,
              })}
            </div>
          );
        }

        if (node.kind === 'group') {
          return (
            <section key={`${contextPath || 'root'}:${node.id}`} className="space-y-2.5 pt-2">
              <div className="mt-3 mb-1 first:mt-0">
                <h5 className="text-[10px] font-semibold text-neutral-500 uppercase tracking-wider">
                  {node.title}
                </h5>
                {node.description && (
                  <p className="mt-0.5 text-[11px] text-neutral-400/70">{node.description}</p>
                )}
              </div>
              <div className="border-t border-[#242426]/30 pt-1">
                <NodeList
                  nodes={node.children}
                  snapshot={snapshot}
                  savingPath={savingPath}
                  fieldErrors={fieldErrors}
                  contextPath={contextPath}
                  onSaveField={onSaveField}
                  onDeletePath={onDeletePath}
                  onAddCollectionItem={onAddCollectionItem}
                />
              </div>
            </section>
          );
        }

        return (
          <section key={`${contextPath || 'root'}:${node.id}`} className="pt-2">
            <CollectionEditor
              collection={node}
              snapshot={snapshot}
              savingPath={savingPath}
              fieldErrors={fieldErrors}
              contextPath={contextPath}
              onSaveField={onSaveField}
              onDeletePath={onDeletePath}
              onAddCollectionItem={onAddCollectionItem}
              renderNodeList={(props) => <NodeList {...props} />}
            />
          </section>
        );
      })}
    </div>
  );
}
