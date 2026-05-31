import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import test from 'node:test';
import ts from 'typescript';

const sourcePath = new URL('../src/features/settings/schemaRendererView.ts', import.meta.url);
const schemaRendererComponentPath = new URL(
  '../src/components/settings/schemaRenderer/SchemaRenderer.tsx',
  import.meta.url
);
const runtimePath = new URL(
  '../src/components/settings/schemaRenderer/dataSources/runtime.ts',
  import.meta.url
);
const conditionsPath = new URL(
  '../src/components/settings/schemaRenderer/dataSources/conditions.ts',
  import.meta.url
);
const dataSourceUiPath = new URL(
  '../src/components/settings/schemaRenderer/dataSources/ui.tsx',
  import.meta.url
);
const pluginSettingsDataPath = new URL(
  '../src/components/settings/schemaRenderer/dataSources/pluginSettingsData.ts',
  import.meta.url
);
const toolsSectionPath = new URL(
  '../src-tauri/src/config/settings_ui_sections/tools.json',
  import.meta.url
);
const repoRoot = fileURLToPath(new URL('..', import.meta.url));
const outDir = join(repoRoot, 'node_modules', '.cache', 'agentjax-settings-schema-renderer-tests');
await mkdir(outDir, { recursive: true });

const compile = async (path) =>
  ts.transpileModule(await readFile(path, 'utf8'), {
  compilerOptions: {
    module: ts.ModuleKind.ES2022,
    target: ts.ScriptTarget.ES2022,
    jsx: ts.JsxEmit.ReactJSX,
    strict: true,
  },
  }).outputText;

const compileDataSourceUi = async (conditionsModulePath) => {
  let source = await readFile(dataSourceUiPath, 'utf8');
  source = source
    .replace(
      "import { LoaderCircle, Search } from 'lucide-react';",
      'const LoaderCircle = () => null;\nconst Search = () => null;'
    )
    .replace(
      "import { resolveLucideIcon } from '../../../../features/icons/lucide';",
      'const resolveLucideIcon = () => null;'
    )
    .replace(
      "} from './conditions';",
      `} from '${pathToFileURL(conditionsModulePath).href}';`
    );
  return ts.transpileModule(source, {
    compilerOptions: {
      module: ts.ModuleKind.ES2022,
      target: ts.ScriptTarget.ES2022,
      jsx: ts.JsxEmit.ReactJSX,
      strict: true,
    },
  }).outputText;
};

const modulePath = join(outDir, `schemaRendererView-${Date.now()}.mjs`);
await writeFile(modulePath, await compile(sourcePath), 'utf8');
const view = await import(`file://${modulePath}`);

const runtimeModulePath = join(outDir, `schemaDataRuntime-${Date.now()}.mjs`);
await writeFile(runtimeModulePath, await compile(runtimePath), 'utf8');
const runtime = await import(`file://${runtimeModulePath}`);

const conditionsModulePath = join(outDir, `schemaDataConditions-${Date.now()}.mjs`);
await writeFile(conditionsModulePath, await compile(conditionsPath), 'utf8');
const conditions = await import(`file://${conditionsModulePath}`);

const dataSourceUiModulePath = join(outDir, `schemaDataUi-${Date.now()}.mjs`);
await writeFile(dataSourceUiModulePath, await compileDataSourceUi(conditionsModulePath), 'utf8');
const dataSourceUi = await import(`file://${dataSourceUiModulePath}`);

const pluginSettingsDataModulePath = join(outDir, `pluginSettingsData-${Date.now()}.mjs`);
await writeFile(pluginSettingsDataModulePath, await compile(pluginSettingsDataPath), 'utf8');
const pluginSettingsData = await import(`file://${pluginSettingsDataModulePath}`);

const toolsSection = JSON.parse(await readFile(toolsSectionPath, 'utf8'));

test('SchemaRenderer does not expose a custom ui render escape hatch', async () => {
  const source = await readFile(schemaRendererComponentPath, 'utf8');
  assert.doesNotMatch(source, /renderUiNode/);
});

test('dispatches v1 schema nodes and v2 ui nodes through stable render kinds', () => {
  assert.equal(view.getSchemaRenderKind({ kind: 'field', id: 'name' }), 'field');
  assert.equal(view.getSchemaRenderKind({ kind: 'group', id: 'group' }), 'group');
  assert.equal(view.getSchemaRenderKind({ kind: 'collection', id: 'items' }), 'collection');
  assert.equal(view.getSchemaRenderKind({ kind: 'tabs', id: 'tabs' }), 'ui');
  assert.equal(view.getSchemaRenderKind({ kind: 'toolbar', id: 'toolbar' }), 'ui');
  assert.equal(view.getSchemaRenderKind({ kind: 'collapsible', id: 'advanced' }), 'ui');
});

test('routes structural ui nodes through the data-context renderer', () => {
  assert.equal(view.shouldUseDataContextRenderer({ kind: 'split', id: 'split' }), true);
  assert.equal(view.shouldUseDataContextRenderer({ kind: 'toolbar', id: 'toolbar' }), true);
  assert.equal(
    view.shouldUseDataContextRenderer({
      kind: 'metric',
      id: 'plugin-health',
      dataSource: 'plugin.demo.summary',
    }),
    true
  );
  assert.equal(
    view.shouldUseDataContextRenderer({
      kind: 'action',
      id: 'plugin-refresh',
      actions: [{ id: 'refreshPlugin', variant: 'button', dataSource: 'plugin.demo.summary' }],
    }),
    true
  );
  assert.equal(view.shouldUseDataContextRenderer({ kind: 'badge', id: 'static-badge' }), false);
  assert.equal(view.shouldUseDataContextRenderer({ kind: 'collapsible', id: 'advanced' }), false);
  assert.equal(view.shouldUseDataContextRenderer({ kind: 'field', id: 'field' }), false);
});

test('tool manager layout is described with ui schema data source nodes', () => {
  const [root] = toolsSection.children;
  assert.equal(root.kind, 'layout');
  assert.equal(root.id, 'tools-manager');
  assert.equal(root.dataSource, 'toolManager');
  assert.equal(root.variant, 'workbench');
  assert.equal(root.children[0].kind, 'tabs');
  assert.equal(root.children[0].dataSource, 'toolManager.categories');
  assert.equal(root.children[0].bindings.activeSource, 'toolManager.query');
  assert.equal(root.children[1].kind, 'toolbar');
  assert.equal(root.children[1].dataSource, 'toolManager.activeSource');
  assert.equal(
    root.children[1].actions.some((action) => action.variant === 'search'),
    false
  );
  assert.deepEqual(root.children[1].actions[0].visibleWhen, [
    { path: 'policyPaths.sourceEnabledPath', truthy: true },
  ]);
  assert.deepEqual(root.children[1].actions[1].visibleWhen, [
    { path: 'policyPaths.exposurePath', truthy: true },
  ]);
  assert.deepEqual(root.children[1].actions[2].visibleWhen, [
    { path: 'sourceType', equals: 'mcp' },
  ]);

  const split = root.children[2];
  assert.equal(split.kind, 'split');
  assert.equal(split.layout, 'three-pane');
  assert.deepEqual(
    split.children.map((node) => [node.kind, node.dataSource || node.id]),
    [
      ['list', 'toolManager.sources'],
      ['list', 'toolManager.tools'],
      ['detail', 'toolManager.selectedTool'],
    ]
  );
});

test('tools settings section uses ui schema instead of a custom control field', () => {
  const [node] = toolsSection.children;
  assert.equal(node.kind, 'layout');
  assert.equal(node.dataSource, 'toolManager');
  assert.equal(node.control, undefined);
  assert.equal(node.children[2].kind, 'split');
  const detailTemplate = node.children[2].children[2].itemTemplate;
  assert.equal(detailTemplate.bindings.detailItems, 'schemaProperties');
  assert.equal(detailTemplate.bindings.detailItemsTitle, 'settings.tools.parameters');
  assert.equal(detailTemplate.defaultExpanded, true);
  assert.equal(detailTemplate.bindings.schemaProperties, undefined);
  assert.deepEqual(
    detailTemplate.properties.map((property) => [property.id, property.value, property.variant]),
    [
      ['tool-id', 'id', 'code'],
      ['model-name', 'modelName', 'code'],
      ['schema-format', 'schemaFormat', 'badge'],
    ]
  );
  assert.equal(view.schemaUsesDataSource([node], 'toolManager'), true);
  assert.equal(view.schemaUsesDataSource([node], 'toolManager.selectedTool'), true);
  assert.equal(view.schemaUsesDataSource([node], 'toolManager.unknown'), false);
  assert.deepEqual(view.collectSchemaDataSources([node]), [
    'toolManager',
    'toolManager.activeSource',
    'toolManager.categories',
    'toolManager.selectedTool',
    'toolManager.sources',
    'toolManager.tools',
  ]);
  assert.deepEqual(view.collectSchemaDataSourceNamespaces([node]), ['toolManager']);
  assert.equal(view.schemaUsesDataSource([node], 'plugin.example'), false);
});

test('schema search indexes collapsible nodes and property declarations', () => {
  const nodes = [
    {
      kind: 'collapsible',
      id: 'advanced-panel',
      title: 'Advanced',
      defaultExpanded: false,
      properties: [
        {
          id: 'runtime-mode',
          label: 'Runtime Mode',
          value: 'mode',
          variant: 'badge',
        },
      ],
      children: [
        {
          kind: 'field',
          id: 'timeout',
          title: 'Timeout',
          path: 'runtime.timeout',
          valueType: 'integer',
          control: 'number',
        },
      ],
    },
  ];

  const propertyMatch = view.filterSchemaNodesForSearch(nodes, 'runtime mode');
  assert.equal(propertyMatch.length, 1);
  assert.equal(propertyMatch[0].id, 'advanced-panel');
  assert.equal(propertyMatch[0].defaultExpanded, false);

  const childMatch = view.filterSchemaNodesForSearch(nodes, 'timeout');
  assert.equal(childMatch.length, 1);
  assert.equal(childMatch[0].children.length, 1);
});

test('schema search keeps matching branches and preserves dynamic data-source surfaces', () => {
  const nodes = [
    {
      kind: 'group',
      id: 'provider-group',
      title: 'Provider Settings',
      children: [
        {
          kind: 'field',
          id: 'base-url',
          title: 'Base URL',
          path: 'providers.openai.base_url',
          valueType: 'string',
          control: 'text',
        },
      ],
    },
    {
      kind: 'layout',
      id: 'dynamic-tools',
      dataSource: 'toolManager',
      children: [],
    },
  ];

  const filtered = view.filterSchemaNodesForSearch(nodes, 'base');
  assert.equal(filtered.length, 2);
  assert.equal(filtered[0].id, 'provider-group');
  assert.equal(filtered[0].children.length, 1);
  assert.equal(filtered[1].id, 'dynamic-tools');
});

test('schema search can disable dynamic surface preservation for section matching', () => {
  const nodes = [
    {
      kind: 'layout',
      id: 'dynamic-tools',
      dataSource: 'toolManager',
      children: [],
    },
  ];

  const renderFiltered = view.filterSchemaNodesForSearch(nodes, 'definitely missing');
  assert.equal(renderFiltered.length, 1);
  assert.equal(renderFiltered[0].id, 'dynamic-tools');

  const sectionFiltered = view.filterSchemaNodesForSearch(
    nodes,
    'definitely missing',
    (key) => key,
    { preserveDataSourceNodes: false }
  );
  assert.equal(sectionFiltered.length, 0);
});

test('schema search indexes item templates as first-class schema declarations', () => {
  const nodes = [
    {
      kind: 'list',
      id: 'plugin-list',
      itemTemplate: {
        kind: 'detail',
        id: 'plugin-list-item-template',
        bindings: {
          title: 'name',
        },
        properties: [
          {
            id: 'provider-scope',
            label: 'Provider Scope',
            value: 'scope',
            variant: 'badge',
          },
        ],
      },
    },
  ];

  const filtered = view.filterSchemaNodesForSearch(nodes, 'provider scope');
  assert.equal(filtered.length, 1);
  assert.equal(filtered[0].id, 'plugin-list');
  assert.equal(filtered[0].itemTemplate.id, 'plugin-list-item-template');
});

test('schema data-source discovery scans nested templates and action sources', () => {
  const nodes = [
    {
      kind: 'layout',
      id: 'plugin-root',
      dataSource: 'plugin.demo.root',
      children: [
        {
          kind: 'toolbar',
          id: 'plugin-toolbar',
          actions: [
            {
              id: 'setQuery',
              variant: 'search',
              dataSource: 'plugin.demo.query',
            },
          ],
        },
        {
          kind: 'list',
          id: 'tools',
          dataSource: 'toolManager.tools',
          itemTemplate: {
            kind: 'detail',
            id: 'tool-template',
            dataSource: 'toolManager.selectedTool',
          },
        },
      ],
    },
  ];

  assert.deepEqual(view.collectSchemaDataSourceNamespaces(nodes), ['plugin', 'toolManager']);
  assert.equal(view.schemaUsesDataSource(nodes, 'plugin'), true);
  assert.equal(view.schemaUsesDataSource(nodes, 'toolManager'), true);
  assert.equal(view.schemaUsesDataSource(nodes, 'mcp'), false);
});

test('data providers route by namespace instead of renderer-specific business branches', async () => {
  const calls = [];
  const provider = (namespace, enabled = true) => ({
    namespace,
    enabled,
    getDataSource: (dataSource) => `${namespace}:${dataSource}`,
    dispatch: (action, payload) => {
      calls.push([namespace, action, payload?.dataSource || '']);
    },
    isSaving: (savingKey) => savingKey === `${namespace}:saving`,
    getStatus: (dataSource) => ({ error: `${namespace}:${dataSource}` }),
  });

  assert.equal(runtime.dataSourceMatchesNamespace('plugin.demo.items', 'plugin'), true);
  assert.equal(runtime.dataSourceMatchesNamespace('pluginDemo.items', 'plugin'), false);

  const context = runtime.mergeSchemaDataProviders([
    provider('toolManager'),
    provider('plugin'),
    provider('disabled', false),
  ]);

  assert.equal(context.getDataSource('plugin.demo.items'), 'plugin:plugin.demo.items');
  assert.equal(context.getStatus('toolManager.root').error, 'toolManager:toolManager.root');
  assert.equal(context.isSaving('plugin:saving'), true);

  await context.dispatch('selectItem', { dataSource: 'plugin.demo.items' });
  assert.deepEqual(calls, [['plugin', 'selectItem', 'plugin.demo.items']]);

  await context.dispatch('refreshAll', {});
  assert.deepEqual(calls.slice(1), [
    ['toolManager', 'refreshAll', ''],
    ['plugin', 'refreshAll', ''],
  ]);
});

test('data-source item conditions drive generic visibility and disabled states', () => {
  const item = {
    sourceType: 'mcp',
    policyPaths: {
      sourceEnabledPath: 'tool_manager.mcp_tools.docs.enabled',
    },
    capabilities: ['discover', 'toggle'],
    locked: false,
  };

  assert.equal(
    conditions.itemConditionsMatch([{ path: 'policyPaths.sourceEnabledPath', truthy: true }], item),
    true
  );
  assert.equal(
    conditions.itemConditionsMatch([{ path: 'sourceType', equals: 'plugin' }], item),
    false
  );
  assert.equal(
    conditions.itemConditionsMatch([{ path: 'capabilities', includes: 'discover' }], item),
    true
  );
  assert.equal(
    conditions.itemDisableConditionsMatch([{ path: 'locked', truthy: true }], item),
    false
  );
  assert.equal(
    conditions.itemDisableConditionsMatch([{ path: 'sourceType', notEquals: 'native' }], item),
    true
  );
});

test('data-source ui primitives expose shared class and action condition helpers', () => {
  const item = {
    sourceType: 'mcp',
    locked: true,
  };
  const action = {
    id: 'discover',
    variant: 'button',
    visibleWhen: [{ path: 'sourceType', equals: 'mcp' }],
    disabledWhen: [{ path: 'locked', truthy: true }],
  };

  assert.equal(dataSourceUi.classNames('base', false, null, 'active'), 'base active');
  assert.equal(dataSourceUi.dataSourceActionVisible(action, item), true);
  assert.equal(dataSourceUi.dataSourceActionDisabled(action, item), true);
  assert.equal(
    dataSourceUi.dataSourceActionVisible(
      { ...action, visibleWhen: [{ path: 'sourceType', equals: 'plugin' }] },
      item
    ),
    false
  );
});

test('plugin settings data runtime resolves searchable lists and selected detail items', () => {
  const snapshot = {
    dataSources: {
      'plugin.local.demo.items': [
        {
          id: 'alpha',
          name: 'Alpha',
          description: 'Primary plugin item',
        },
        {
          id: 'beta',
          name: 'Beta',
          description: 'Secondary plugin item',
        },
      ],
    },
  };

  const filtered = pluginSettingsData.resolvePluginSettingsList({
    snapshot,
    dataSource: 'plugin.local.demo.items',
    search: 'secondary',
    selectedIds: {},
  });
  assert.deepEqual(
    filtered.map((item) => [item.id, item.activeKey]),
    [['beta', 'beta']]
  );

  const selected = pluginSettingsData.resolvePluginSelectedItem({
    snapshot,
    dataSource: 'plugin.local.demo.items.selected',
    search: '',
    selectedIds: {
      'plugin.local.demo.items': 'beta',
    },
  });
  assert.equal(selected.id, 'beta');
  assert.equal(
    pluginSettingsData.selectedSourceForPluginDetail('plugin.local.demo.items.selectedItem'),
    'plugin.local.demo.items'
  );
});

test('plugin settings snapshot supports global section search discovery', () => {
  const snapshot = {
    dataSources: {
      'plugin.local.demo.items': [
        {
          id: 'alpha',
          name: 'Alpha',
          description: 'Primary plugin item',
        },
      ],
    },
  };

  assert.equal(
    pluginSettingsData.pluginSettingsSnapshotMatchesQuery(snapshot, 'primary plugin'),
    true
  );
  assert.deepEqual(
    pluginSettingsData.pluginSettingsSnapshotMatchingDataSources(snapshot, 'primary plugin'),
    ['plugin.local.demo.items']
  );
  assert.equal(
    pluginSettingsData.pluginSettingsSnapshotMatchesQuery(snapshot, 'plugin.local.demo.items'),
    true
  );
  assert.equal(
    pluginSettingsData.pluginSettingsSnapshotMatchesQuery(snapshot, 'missing query'),
    false
  );
});
