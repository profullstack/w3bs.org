import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { buildSuite, NAMED } from '../src/os-suite.mjs';

const catalog = JSON.parse(
  readFileSync(new URL('../catalog/catalog.json', import.meta.url), 'utf8'),
);

test('the suite manifest includes every named inclusion and gives each command one owner', () => {
  const suite = buildSuite(catalog);
  for (const id of Object.keys(NAMED))
    assert.ok(
      suite.entries.some((e) => e.id === id && e.named),
      id,
    );
  assert.deepEqual(suite.summary.missing_named, []);
  const owners = {};
  for (const e of suite.entries)
    for (const c of e.commands) if (!c.shadowed) (owners[c.name] ??= []).push(e.id);
  for (const [name, ids] of Object.entries(owners)) assert.equal(ids.length, 1, `${name}: ${ids}`);
});

test('ownership needs both vendor and publisher, and unverified entries stay out', () => {
  const suite = buildSuite([
    {
      id: 'a.dev/a',
      name: 'a',
      vendor: 'Profullstack',
      publisher: 'profullstack',
      commands: ['a'],
      install: 'curl x | sh',
    },
    { id: 'b.dev/b', name: 'b', vendor: 'Profullstack', publisher: 'someone', commands: ['b'] },
  ]);
  assert.deepEqual(
    suite.entries.map((e) => e.id),
    ['a.dev/a'],
  );
  assert.ok(suite.blocking.some((b) => b.id === 'a.dev/a' && /pin a release/.test(b.task)));
  assert.ok(suite.blocking.some((b) => b.id === 'a.dev/a' && /signed artifact/.test(b.task)));
});
