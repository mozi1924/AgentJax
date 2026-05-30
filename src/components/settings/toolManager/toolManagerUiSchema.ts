import type { SettingsSchemaNode } from '../../../features/settings/types';

// Tool Manager is expressed as UI schema: data comes from the snapshot hooks and
// these nodes only describe the high-level composition and responsive regions.
export const TOOL_MANAGER_UI_SCHEMA: SettingsSchemaNode[] = [
  {
    kind: 'layout',
    id: 'tool-manager-root',
    layout: 'vertical',
    density: 'compact',
    children: [
      {
        kind: 'tabs',
        id: 'tool-source-tabs',
        dataSource: 'toolManager.categories',
      },
      {
        kind: 'split',
        id: 'tool-manager-layout',
        layout: 'three-pane',
        responsive: 'stack',
        children: [
          {
            kind: 'list',
            id: 'tool-source-list',
            dataSource: 'toolManager.sources',
            width: 220,
            scroll: 'y',
          },
          {
            kind: 'panel',
            id: 'tool-list-pane',
            dataSource: 'toolManager.activeSource',
            children: [
              {
                kind: 'toolbar',
                id: 'tool-source-header',
                dataSource: 'toolManager.sourceHeader',
              },
              {
                kind: 'list',
                id: 'tool-list',
                dataSource: 'toolManager.tools',
                scroll: 'y',
              },
            ],
          },
          {
            kind: 'detail',
            id: 'tool-detail',
            dataSource: 'toolManager.selectedTool',
            scroll: 'y',
          },
        ],
      },
    ],
  },
];
