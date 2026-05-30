import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import ts from 'typescript';

const sourcePath = new URL('../src/features/settings/fieldControlDrafts.ts', import.meta.url);
const outDir = join(tmpdir(), 'agentjax-settings-field-control-tests');
await mkdir(outDir, { recursive: true });

const compiled = ts.transpileModule(await readFile(sourcePath, 'utf8'), {
  compilerOptions: {
    module: ts.ModuleKind.ES2022,
    target: ts.ScriptTarget.ES2022,
    strict: true,
  },
}).outputText;

const modulePath = join(outDir, `fieldControlDrafts-${Date.now()}.mjs`);
await writeFile(modulePath, compiled, 'utf8');
const drafts = await import(`file://${modulePath}`);

test('parses field control drafts into commit values', () => {
  assert.equal(drafts.parseNumberDraftValue({ valueType: 'integer' }, '42'), 42);
  assert.equal(drafts.parseNumberDraftValue({ valueType: 'float' }, '4.25'), 4.25);
  assert.equal(drafts.parseNumberDraftValue({ valueType: 'integer' }, ''), null);
  assert.deepEqual(drafts.parseTagsDraftValue('alpha, beta,, gamma '), [
    'alpha',
    'beta',
    'gamma',
  ]);
});

test('applies key_value patches to the latest draft before blur save', () => {
  const stale = [{ id: 'kv-1', key: 'OLD_KEY', value: 'old' }];
  const latest = drafts.applyKeyValueEntryPatch(stale, 'kv-1', { key: 'NEW_KEY' });
  const committed = drafts.applyKeyValueEntryPatch(latest, 'kv-1', { value: 'new' });

  assert.deepEqual(committed, [{ id: 'kv-1', key: 'NEW_KEY', value: 'new' }]);
  assert.deepEqual(stale, [{ id: 'kv-1', key: 'OLD_KEY', value: 'old' }]);
});
