const joinClassNames = (...values: Array<string | false | null | undefined>) =>
  values.filter(Boolean).join(' ');

// Shared layout class mapping for declarative UI schema containers. Keeping this
// outside data-source renderers lets static settings and provider-backed panels
// use the same visual language.
export const settingsUiLayoutClassName = (layout?: string, variant?: string) => {
  if (variant === 'workbench') {
    return joinClassNames('settings-schema-layout', 'settings-schema-workbench');
  }

  return joinClassNames(
    'settings-schema-layout',
    (layout === 'inline' || layout === 'row') && 'settings-schema-layout-inline',
    layout === 'grid' && 'settings-schema-layout-grid',
    layout !== 'inline' &&
      layout !== 'row' &&
      layout !== 'grid' &&
      'settings-schema-layout-stack'
  );
};

export const settingsUiSplitClassName = (layout?: string) =>
  joinClassNames(
    'settings-schema-split',
    layout === 'two-pane' && 'settings-schema-split-two-pane',
    layout === 'three-pane' && 'settings-schema-split-three-pane',
    layout !== 'two-pane' && layout !== 'three-pane' && 'settings-schema-split-auto'
  );
