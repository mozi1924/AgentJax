import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import ts from 'typescript';

const sourcePath = new URL(
  '../src/components/settings/schemaRenderer/dataSources/toolManagerData.ts',
  import.meta.url
);
const outDir = join(tmpdir(), 'agentjax-tool-manager-data-tests');
await mkdir(outDir, { recursive: true });

const sourceText = (await readFile(sourcePath, 'utf8')).replace(
  "import { escapePathSegment } from '../../../../features/settings/utils';",
  "const escapePathSegment = (segment) => `${segment || ''}`.replace(/\\\\/g, '\\\\\\\\').replace(/\\./g, '\\\\.');"
);
const compiled = ts.transpileModule(sourceText, {
  compilerOptions: {
    module: ts.ModuleKind.ES2022,
    target: ts.ScriptTarget.ES2022,
    strict: true,
  },
}).outputText;

const modulePath = join(outDir, `toolManagerData-${Date.now()}.mjs`);
await writeFile(modulePath, compiled, 'utf8');

const view = await import(`file://${modulePath}`);

const source = (overrides) => ({
  sourceType: 'native',
  sourceId: 'native',
  sourceName: 'Native Tools',
  enabled: true,
  status: 'ready',
  exposureMode: 'always',
  tools: [],
  ...overrides,
});

const tool = (overrides) => ({
  id: 'calculator',
  friendlyName: 'Calculator',
  modelName: 'calculator',
  description: 'Evaluate expressions',
  icon: null,
  enabled: true,
  availability: 'available',
  schemaSummary: { parameterCount: 1, required: [], properties: ['expression'] },
  inputSchema: {
    type: 'object',
    properties: {
      expression: { type: 'string' },
    },
  },
  schemaFormat: 'json_schema',
  policyPaths: {},
  ...overrides,
});

const SourceList = ({ sources, category }) => {
  const categorySources = view.sourcesForCategory(sources, category);
  if (categorySources.length === 0) {
    return React.createElement('p', { 'data-testid': 'empty-sources' }, 'No sources');
  }
  return React.createElement(
    'div',
    { 'data-testid': 'source-list' },
    categorySources.map((item) =>
      React.createElement(
        'button',
        {
          key: `${item.sourceType}-${item.sourceId}`,
          'data-source-id': item.sourceId,
        },
        item.sourceName
      )
    )
  );
};

test('groups dynamic and control sources into the expected visible categories', () => {
  const sources = [
    source({ sourceType: 'native', sourceId: 'native' }),
    source({ sourceType: 'mcp', sourceId: 'docs' }),
    source({ sourceType: 'control', sourceId: 'mcp_controls' }),
    source({ sourceType: 'dynamic', sourceId: 'session' }),
    source({ sourceType: 'background', sourceId: 'background' }),
  ];

  assert.deepEqual(
    view.sourcesForCategory(sources, 'mcp').map((item) => item.sourceId),
    ['docs', 'mcp_controls']
  );
  assert.deepEqual(
    view.sourcesForCategory(sources, 'session').map((item) => item.sourceId),
    ['session']
  );
  assert.deepEqual(view.sourcesForCategory(sources, 'plugin'), []);
});

test('selects a stable empty state when a category has no sources', () => {
  assert.equal(view.selectToolManagerSource([], 'missing'), null);
  const markup = renderToStaticMarkup(
    React.createElement(SourceList, { sources: [], category: 'plugin' })
  );
  assert.match(markup, /data-testid="empty-sources"/);
  assert.match(markup, /No sources/);
});

test('renders grouped source list state for the active category', () => {
  const sources = [
    source({ sourceType: 'mcp', sourceId: 'docs', sourceName: 'Docs Server' }),
    source({ sourceType: 'control', sourceId: 'mcp_controls', sourceName: 'MCP Controls' }),
    source({ sourceType: 'dynamic', sourceId: 'session', sourceName: 'Session Tools' }),
  ];
  const markup = renderToStaticMarkup(
    React.createElement(SourceList, { sources, category: 'mcp' })
  );

  assert.match(markup, /data-testid="source-list"/);
  assert.match(markup, /data-source-id="docs"/);
  assert.match(markup, /data-source-id="mcp_controls"/);
  assert.doesNotMatch(markup, /data-source-id="session"/);
});

test('filters only the currently selected source tools', () => {
  const selected = source({
    sourceId: 'native',
    tools: [
      tool({ id: 'calculator', friendlyName: 'Calculator', modelName: 'calculator' }),
      tool({ id: 'system_time', friendlyName: 'System Time', modelName: 'system_time' }),
    ],
  });
  const unselected = source({
    sourceId: 'plugin.demo',
    sourceType: 'plugin',
    tools: [tool({ id: 'say_hello', friendlyName: 'Say Hello', modelName: 'plugin__demo__say_hello' })],
  });

  const active = view.selectToolManagerSource([selected, unselected], 'native');
  assert.equal(active.sourceId, 'native');
  assert.deepEqual(
    view.filterToolsForQuery(active.tools, 'hello').map((item) => item.id),
    []
  );
  assert.deepEqual(
    view.filterToolsForQuery(active.tools, 'time').map((item) => item.id),
    ['system_time']
  );
});

test('matches tool manager snapshots for global settings search discovery', () => {
  const snapshot = {
    sources: [
      source({
        sourceType: 'mcp',
        sourceId: 'docs',
        sourceName: 'Docs Server',
        tools: [
          tool({
            id: 'search_docs',
            friendlyName: 'Search Docs',
            description: 'Search developer documentation',
          }),
        ],
      }),
    ],
  };

  assert.equal(view.toolManagerSnapshotMatchesQuery(snapshot, 'developer documentation'), true);
  assert.equal(view.toolManagerSnapshotMatchesQuery(snapshot, 'docs server'), true);
  assert.equal(view.toolManagerSnapshotMatchesQuery(snapshot, 'missing query'), false);
});

test('selects sources by typed identity when source ids collide across categories', () => {
  const sources = [
    source({ sourceType: 'mcp', sourceId: 'shared', sourceName: 'MCP Shared' }),
    source({ sourceType: 'plugin', sourceId: 'shared', sourceName: 'Plugin Shared' }),
  ];

  assert.equal(view.sourceIdentityKey(sources[0]), 'mcp:shared');
  assert.equal(
    view.selectToolManagerSource(sources, 'plugin:shared').sourceName,
    'Plugin Shared'
  );
});

test('builds escaped policy paths for source and tool switches', () => {
  const pluginSource = source({
    sourceType: 'plugin',
    sourceId: 'local.demo',
    sourceName: 'Local Demo',
  });
  const mcpSource = source({
    sourceType: 'mcp',
    sourceId: 'openai.docs',
    sourceName: 'OpenAI Docs',
  });

  assert.equal(
    view.sourcePolicyEnabledPath(pluginSource),
    'tool_manager.plugin_tools.local\\.demo.enabled'
  );
  assert.equal(
    view.toolPolicyEnabledPath(pluginSource, tool({ id: 'say.hello' })),
    'tool_manager.plugin_tools.local\\.demo.tools.say\\.hello.enabled'
  );
  assert.equal(
    view.toolPolicyEnabledPath(mcpSource, tool({ id: 'search.docs' })),
    'tool_manager.mcp_tools.openai\\.docs.tools.search\\.docs.enabled'
  );
  assert.equal(
    view.mcpExposurePolicyPath(mcpSource),
    'tool_manager.mcp_tools.openai\\.docs.exposure'
  );

  assert.equal(
    view.sourcePolicyEnabledPath(
      source({
        sourceType: 'mcp',
        sourceId: 'ignored',
        policyPaths: { sourceEnabledPath: 'tool_manager.mcp_tools.from_backend.enabled' },
      })
    ),
    'tool_manager.mcp_tools.from_backend.enabled'
  );
  assert.equal(
    view.toolPolicyEnabledPath(
      mcpSource,
      tool({ policyPaths: { toolEnabledPath: 'tool_manager.mcp_tools.from_backend.tools.t.enabled' } })
    ),
    'tool_manager.mcp_tools.from_backend.tools.t.enabled'
  );
});

test('marks only global policy surfaces editable', () => {
  assert.equal(view.isSourcePolicyEditable(source({ sourceType: 'plugin' })), true);
  assert.equal(view.isSourcePolicyEditable(source({ sourceType: 'mcp' })), true);
  assert.equal(view.isSourcePolicyEditable(source({ sourceType: 'native' })), false);
  assert.equal(view.isSourcePolicyEditable(source({ sourceType: 'dynamic' })), false);
  assert.equal(view.isToolPolicyEditable(source({ sourceType: 'native' })), true);
  assert.equal(view.isToolPolicyEditable(source({ sourceType: 'background' })), false);
});
