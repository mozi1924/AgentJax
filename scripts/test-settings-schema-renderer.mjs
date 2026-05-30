import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import ts from 'typescript';

const sourcePath = new URL('../src/features/settings/schemaRendererView.ts', import.meta.url);
const toolManagerSchemaPath = new URL(
  '../src/components/settings/toolManager/toolManagerUiSchema.ts',
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

const toolManagerSchemaModulePath = join(outDir, `toolManagerUiSchema-${Date.now()}.mjs`);
await writeFile(toolManagerSchemaModulePath, await compile(toolManagerSchemaPath), 'utf8');
const toolManagerSchema = await import(`file://${toolManagerSchemaModulePath}`);
const toolsSection = JSON.parse(await readFile(toolsSectionPath, 'utf8'));

test('dispatches v1 schema nodes and v2 ui nodes through stable render kinds', () => {
  assert.equal(view.getSchemaRenderKind({ kind: 'field', id: 'name' }), 'field');
  assert.equal(view.getSchemaRenderKind({ kind: 'group', id: 'group' }), 'group');
  assert.equal(view.getSchemaRenderKind({ kind: 'collection', id: 'items' }), 'collection');
  assert.equal(view.getSchemaRenderKind({ kind: 'tabs', id: 'tabs' }), 'ui');
  assert.equal(view.getSchemaRenderKind({ kind: 'toolbar', id: 'toolbar' }), 'ui');
});

test('tool manager layout is described with ui schema data source nodes', () => {
  const [root] = toolManagerSchema.TOOL_MANAGER_UI_SCHEMA;
  assert.equal(root.kind, 'layout');
  assert.equal(root.id, 'tool-manager-root');
  assert.equal(root.children[0].kind, 'tabs');
  assert.equal(root.children[0].dataSource, 'toolManager.categories');

  const split = root.children[1];
  assert.equal(split.kind, 'split');
  assert.equal(split.layout, 'three-pane');
  assert.deepEqual(
    split.children.map((node) => [node.kind, node.dataSource || node.id]),
    [
      ['list', 'toolManager.sources'],
      ['panel', 'toolManager.activeSource'],
      ['detail', 'toolManager.selectedTool'],
    ]
  );
});

test('tools settings section uses ui schema instead of a custom control field', () => {
  const [node] = toolsSection.children;
  assert.equal(node.kind, 'layout');
  assert.equal(node.dataSource, 'toolManager');
  assert.equal(node.control, undefined);
  assert.equal(node.children[1].kind, 'split');
});
