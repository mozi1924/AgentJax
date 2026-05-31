import assert from 'node:assert/strict';
import test from 'node:test';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';

if (!globalThis.navigator) {
  globalThis.navigator = { language: 'en' };
}

const vite = await createServer({
  configFile: false,
  logLevel: 'silent',
  server: {
    middlewareMode: true,
  },
  plugins: [(await import('@vitejs/plugin-react')).default()],
});

try {
  const { SchemaRenderer } = await vite.ssrLoadModule(
    '/src/components/settings/schemaRenderer/SchemaRenderer.tsx'
  );
  const { I18nProvider } = await vite.ssrLoadModule('/src/features/i18n/I18nProvider.tsx');

  const snapshot = {
    configPath: '',
    revision: 'test',
    values: {},
    dynamicOptions: {},
    secretStatuses: {},
  };

  const schemaNodes = [
    {
      kind: 'layout',
      id: 'plugin-demo-workbench',
      dataSource: 'plugin.local.demo.root',
      variant: 'workbench',
      children: [
        {
          kind: 'toolbar',
          id: 'plugin-demo-toolbar',
          dataSource: 'plugin.local.demo.header',
          bindings: {
            title: 'name',
            description: 'description',
            querySource: 'plugin.query',
          },
          actions: [
            {
              id: 'setSearch',
              variant: 'search',
              dataSource: 'plugin.query',
              value: 'search',
              label: 'Search plugins',
            },
          ],
        },
        {
          kind: 'layout',
          id: 'plugin-demo-summary-grid',
          layout: 'grid',
          children: [
            {
              kind: 'metric',
              id: 'plugin-demo-enabled-count',
              dataSource: 'plugin.local.demo.summary',
              bindings: {
                title: 'label',
                value: 'enabledCount',
                description: 'description',
              },
            },
            {
              kind: 'badge',
              id: 'plugin-demo-status',
              dataSource: 'plugin.local.demo.summary',
              bindings: {
                value: 'status',
              },
            },
            {
              kind: 'empty_state',
              id: 'plugin-demo-empty',
              dataSource: 'plugin.local.demo.empty',
              bindings: {
                title: 'title',
                description: 'description',
              },
            },
            {
              kind: 'action',
              id: 'plugin-demo-refresh',
              dataSource: 'plugin.local.demo.summary',
              title: 'Refresh demo',
              icon: 'RefreshCcw',
              variant: 'button',
            },
          ],
        },
        {
          kind: 'split',
          id: 'plugin-demo-split',
          layout: 'three-pane',
          children: [
            {
              kind: 'list',
              id: 'plugin-demo-items',
              dataSource: 'plugin.local.demo.items',
              action: 'selectItem',
              variant: 'content',
              itemTemplate: {
                kind: 'detail',
                id: 'plugin-demo-item-template',
                bindings: {
                  id: 'id',
                  activeKey: 'activeKey',
                  title: 'name',
                  description: 'description',
                },
              },
            },
            {
              kind: 'detail',
              id: 'plugin-demo-detail',
              dataSource: 'plugin.local.demo.items.selected',
              itemTemplate: {
                kind: 'detail',
                id: 'plugin-demo-detail-template',
                defaultExpanded: true,
                bindings: {
                  title: 'name',
                  description: 'description',
                  detailItems: 'params',
                  detailItemsTitle: 'Plugin parameters',
                  detailItemsEmptyText: 'No parameters',
                  detailItemName: 'name',
                  detailItemType: 'type',
                  detailItemDescription: 'description',
                  detailItemRequired: 'required',
                },
                properties: [
                  {
                    id: 'mode',
                    label: 'Mode',
                    value: 'mode',
                    variant: 'badge',
                  },
                ],
              },
            },
          ],
        },
      ],
    },
  ];

  const dataContext = {
    getDataSource: (dataSource) => {
      if (dataSource === 'plugin.local.demo.root') return {};
      if (dataSource === 'plugin.local.demo.header') {
        return {
          name: 'Demo Plugin',
          description: 'Schema rendered plugin panel',
        };
      }
      if (dataSource === 'plugin.query') return { search: 'beta' };
      if (dataSource === 'plugin.local.demo.summary') {
        return {
          label: 'Enabled items',
          enabledCount: 2,
          description: 'Live manifest-backed status',
          status: 'healthy',
        };
      }
      if (dataSource === 'plugin.local.demo.empty') {
        return {
          title: 'No pending work',
          description: 'Everything is configured through schema nodes',
        };
      }
      if (dataSource === 'plugin.local.demo.items') {
        return [
          {
            id: 'alpha',
            activeKey: 'beta',
            name: 'Alpha',
            description: 'Primary item',
          },
          {
            id: 'beta',
            activeKey: 'beta',
            name: 'Beta',
            description: 'Secondary item',
          },
        ];
      }
      if (dataSource === 'plugin.local.demo.items.selected') {
        return {
          id: 'beta',
          name: 'Beta',
          description: 'Secondary item',
          mode: 'enabled',
          params: [
            {
              name: 'query',
              type: 'string',
              required: true,
              description: 'Search query',
            },
          ],
        };
      }
      return undefined;
    },
    dispatch: () => {},
    isSaving: () => false,
  };

  const renderSchema = (nodes) =>
    renderToStaticMarkup(
      React.createElement(
        I18nProvider,
        null,
        React.createElement(SchemaRenderer, {
          nodes,
          snapshot,
          savingPath: null,
          fieldErrors: {},
          actions: {
            saveField: async () => {},
          },
          dataContext,
        })
      )
    );

  test('SchemaRenderer SSR renders a plugin workbench from schema and data context', () => {
    const html = renderSchema(schemaNodes);

    assert.match(html, /settings-schema-workbench/);
    assert.match(html, /settings-schema-layout-grid/);
    assert.match(html, /settings-schema-split-three-pane/);
    assert.match(html, /Demo Plugin/);
    assert.match(html, /Schema rendered plugin panel/);
    assert.match(html, /Enabled items/);
    assert.match(html, />2</);
    assert.match(html, /Live manifest-backed status/);
    assert.match(html, /healthy/);
    assert.match(html, /No pending work/);
    assert.match(html, /Everything is configured through schema nodes/);
    assert.match(html, /Refresh demo/);
    assert.match(html, /Alpha/);
    assert.match(html, /Beta/);
    assert.match(html, /Secondary item/);
    assert.match(html, /Plugin parameters/);
    assert.match(html, /Search query/);
    assert.match(html, /value="beta"/);
  });

  test('SchemaRenderer SSR supports two-pane plugin split layouts', () => {
    const twoPaneNodes = JSON.parse(JSON.stringify(schemaNodes));
    twoPaneNodes[0].children.find((node) => node.id === 'plugin-demo-split').layout = 'two-pane';

    const html = renderSchema(twoPaneNodes);

    assert.match(html, /settings-schema-split-two-pane/);
    assert.match(html, /Demo Plugin/);
    assert.match(html, /Beta/);
    assert.match(html, /Secondary item/);
  });

  test('SchemaRenderer SSR applies shared layout classes to static schema nodes', () => {
    const staticNodes = [
      {
        kind: 'layout',
        id: 'static-summary-grid',
        layout: 'grid',
        children: [
          {
            kind: 'metric',
            id: 'static-total',
            title: 'Total tools',
            value: 4,
            description: 'Static schema metric',
          },
          {
            kind: 'badge',
            id: 'static-status',
            value: 'ready',
          },
        ],
      },
    ];

    const html = renderSchema(staticNodes);

    assert.match(html, /settings-schema-layout-grid/);
    assert.match(html, /Total tools/);
    assert.match(html, />4</);
    assert.match(html, /Static schema metric/);
    assert.match(html, /ready/);
  });
} finally {
  await vite.close();
}
