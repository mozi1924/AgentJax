import type { SettingsRendererProps } from './types';
import { SchemaRenderer } from '../schemaRenderer';
import { useSchemaDataContext } from '../schemaRenderer/dataSources/useSchemaDataContext';
import { filterSchemaNodesForSearch } from '../../../features/settings/schemaRendererView';
import { useI18n } from '../../../features/i18n';

export default function SettingsRenderer(props: SettingsRendererProps) {
  const { t } = useI18n();
  const dataContext = useSchemaDataContext({
    nodes: props.section.children,
    queryState: props.queryState,
    onSaveField: props.onSaveField,
  });
  const nodes = filterSchemaNodesForSearch(
    props.section.children,
    props.queryState?.search || '',
    t
  );

  if (props.queryState?.search && nodes.length === 0) {
    return (
      <div className="flex min-h-[220px] items-center justify-center rounded-lg border border-dashed border-[#2b2b2d] px-4 text-center text-xs text-neutral-500">
        {t('settings.modal.no_results')}
      </div>
    );
  }

  return (
    <SchemaRenderer
      nodes={nodes}
      snapshot={props.snapshot}
      savingPath={props.savingPath}
      fieldErrors={props.fieldErrors}
      queryState={props.queryState}
      actions={{
        saveField: props.onSaveField,
        deletePath: props.onDeletePath,
        addCollectionItem: props.onAddCollectionItem,
      }}
      dataContext={dataContext}
    />
  );
}
