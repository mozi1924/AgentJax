import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import ts from 'typescript';

const sourcePath = new URL('../src/features/settings/schemaRendererView.ts', import.meta.url);
const runtimePath = new URL(
  '../src/components/settings/schemaRenderer/dataSources/runtime.ts',
  import.meta.url
);
const toolsSectionPath = new URL(
  '../src-tauri/src/config/settings_ui_sections/tools.json',
  import.meta.url
);
const outDir = join(tmpdir(), 'agentjax-settings-schema-renderer-tests');
await mkdir(outDir, { recursive: true });

const compile = async (path) =>
  ts.transpileModule(await readFile(path, 'utf8'), {
  compilerOptions: {
    module: ts.ModuleKind.ES2022,
    target: ts.ScriptTarget.ES2022,
    strict: true,
  },
  }).outputText;

const modulePath = join(outDir, `schemaRendererView-${Date.now()}.mjs`);
await writeFile(modulePath, await compile(sourcePath), 'utf8');
const view = await import(`file://${modulePath}`);

const runtimeModulePath = join(outDir, `schemaDataRuntime-${Date.now()}.mjs`);
await writeFile(runtimeModulePath, await compile(runtimePath), 'utf8');
const runtime = await import(`file://${runtimeModulePath}`);

const toolsSection = JSON.parse(await readFile(toolsSectionPath, 'utf8'));

test('dispatches v1 schema nodes and v2 ui nodes through stable render kinds', () => {
  assert.equal(view.getSchemaRenderKind({ kind: 'field', id: 'name' }), 'field');
  assert.equal(view.getSchemaRenderKind({ kind: 'group', id: 'group' }), 'group');
  assert.equal(view.getSchemaRenderKind({ kind: 'collection', id: 'items' }), 'collection');
  assert.equal(view.getSchemaRenderKind({ kind: 'tabs', id: 'tabs' }), 'ui');
  assert.equal(view.getSchemaRenderKind({ kind: 'toolbar', id: 'toolbar' }), 'ui');
});

test('routes structural ui nodes through the data-context renderer', () => {
  assert.equal(view.shouldUseDataContextRenderer({ kind: 'split', id: 'split' }), true);
  assert.equal(view.shouldUseDataContextRenderer({ kind: 'toolbar', id: 'toolbar' }), true);
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
  assert.equal(detailTemplate.bindings.schemaProperties, undefined);
  assert.equal(view.schemaUsesDataSource([node], 'toolManager'), true);
  assert.equal(view.schemaUsesDataSource([node], 'plugin.example'), false);
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
