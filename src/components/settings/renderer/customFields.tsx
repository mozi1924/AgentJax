import type { ReactElement } from 'react';
import { FieldRow } from './FieldRow';
import { PromptAssemblerField } from './PromptAssemblerField';
import type { FieldRendererProps } from './types';

const CUSTOM_FIELD_RENDERERS: Partial<
  Record<string, (props: FieldRendererProps) => ReactElement>
> = {
  prompt_assembler: (props) => <PromptAssemblerField {...props} />,
};

export const renderField = (props: FieldRendererProps) => {
  const customRenderer = CUSTOM_FIELD_RENDERERS[props.field.control];
  if (customRenderer) {
    return customRenderer(props);
  }
  return <FieldRow {...props} />;
};
