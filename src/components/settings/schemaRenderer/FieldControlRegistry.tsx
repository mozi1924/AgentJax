import { useEffect, useState } from 'react';
import type { ReactElement } from 'react';
import type { SettingsControlType } from '../../../features/settings/types';
import {
  getFieldOptions,
  getValueAtPath,
  isNodeEnabled,
  resolvePath,
  validateFieldValue,
} from '../../../features/settings/utils';
import { renderField } from '../renderer/customFields';
import { isFullWidthControl, normalizeFieldValueForDraft } from '../renderer/utils';
import { FieldShell } from './FieldShell';
import { JsonField } from './controls/JsonField';
import { KeyValueField } from './controls/KeyValueField';
import { NumberField } from './controls/NumberField';
import { SelectField } from './controls/SelectField';
import { SwitchField } from './controls/SwitchField';
import { TagsField } from './controls/TagsField';
import { TextField } from './controls/TextField';
import type { FieldControlProps, SchemaRendererProps } from './types';
import { useI18n } from '../../../features/i18n';
import type { SettingsFieldSchema } from '../../../features/settings/types';

type FieldControlComponent = (props: FieldControlProps) => ReactElement;

const FIELD_CONTROL_REGISTRY: Partial<Record<SettingsControlType, FieldControlComponent>> = {
  switch: SwitchField,
  select: SelectField,
  text: TextField,
  secret: TextField,
  textarea: TextField,
  number: NumberField,
  tags: TagsField,
  key_value: KeyValueField,
  json: JsonField,
};

export const getRegisteredFieldControl = (control: SettingsControlType) =>
  FIELD_CONTROL_REGISTRY[control] || null;

const CUSTOM_FIELD_CONTROLS: SettingsControlType[] = ['prompt_assembler', 'tool_manager'];

// Renders a v1 field through the v2 registry, keeping custom controls on the compatibility path.
export function FieldRenderer({
  field,
  snapshot,
  savingPath,
  fieldErrors,
  contextPath,
  actions,
}: Pick<
  SchemaRendererProps,
  'snapshot' | 'savingPath' | 'fieldErrors' | 'contextPath' | 'actions'
> & {
  field: SettingsFieldSchema;
}) {
  const { t } = useI18n();
  const resolvedPath = resolvePath(field.path, contextPath);
  const value = getValueAtPath(snapshot.values, resolvedPath);
  const secretStatus = snapshot.secretStatuses[resolvedPath];
  const [draft, setDraft] = useState(normalizeFieldValueForDraft(field, value, secretStatus));
  const [isDirty, setIsDirty] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);
  const isSaving = savingPath === resolvedPath;
  const disabled = !isNodeEnabled(field, snapshot, contextPath) || isSaving;
  const options = getFieldOptions(field, snapshot, contextPath);

  useEffect(() => {
    setDraft(normalizeFieldValueForDraft(field, value, secretStatus));
    setIsDirty(false);
    setLocalError(null);
  }, [field, value, secretStatus, snapshot.revision]);

  if (CUSTOM_FIELD_CONTROLS.includes(field.control)) {
    return renderField({
      field,
      snapshot,
      savingPath,
      fieldErrors,
      contextPath,
      onSaveField: actions.saveField,
    });
  }

  const Control = getRegisteredFieldControl(field.control);
  if (!Control) {
    return null;
  }

  const commit = async (nextValue: unknown) => {
    const validationError = validateFieldValue(field, nextValue);
    if (validationError) {
      setLocalError(validationError);
      return;
    }

    setLocalError(null);
    if (!isDirty && field.control === 'secret' && `${draft}`.trim() === '') {
      return;
    }
    if (!isDirty && field.control !== 'switch' && field.control !== 'select') {
      return;
    }

    await actions.saveField(resolvedPath, nextValue);
  };

  const rawHelper = fieldErrors[resolvedPath] || localError || field.helpText;
  const helperText = rawHelper ? t(rawHelper) : '';
  const hasError = !!(fieldErrors[resolvedPath] || localError);

  return (
    <FieldShell
      field={field}
      resolvedPath={resolvedPath}
      secretStatus={secretStatus}
      helperText={helperText}
      hasError={hasError}
      fullWidth={isFullWidthControl(field.control)}
    >
      <Control
        field={field}
        resolvedPath={resolvedPath}
        value={value}
        draft={draft}
        setDraft={setDraft}
        isDirty={isDirty}
        setIsDirty={setIsDirty}
        disabled={disabled}
        options={options}
        secretStatus={secretStatus}
        commit={commit}
        onSaveField={actions.saveField}
        setLocalError={setLocalError}
      />
    </FieldShell>
  );
}
