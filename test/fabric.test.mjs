import test from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { renderPrompt, validateManifest, verifyManifest } from '../src/core.mjs';

const dir = new URL('../fixtures/resources/fabric/', import.meta.url);
const trust = JSON.parse(readFileSync(new URL('../fixtures/trust.json', import.meta.url)));
const load = (name) => JSON.parse(readFileSync(new URL(name, dir)));
const manifests = readdirSync(dir)
  .filter((name) => name.endsWith('.json') && name !== 'catalog.json')
  .map(load);

test('every imported Fabric pattern validates and verifies under the fabric key', () => {
  assert.equal(manifests.length, 261);
  for (const manifest of manifests) {
    validateManifest(manifest);
    assert.equal(manifest.publisher.namespace, 'fabric');
    assert.equal(verifyManifest(manifest, trust).valid, true, manifest.id);
    assert.match(
      manifest.provenance.source,
      /^https:\/\/github\.com\/danielmiessler\/Fabric\/blob\/[0-9a-f]{40}\//,
    );
  }
  assert.deepEqual(
    load('catalog.json')
      .map((item) => item.id)
      .sort(),
    manifests.map((manifest) => manifest.id).sort(),
  );
});

test('a Fabric pattern renders piped text as {{input}} and keeps its own variables', () => {
  const rendered = renderPrompt(load('summarize.json'), { input: 'The text.' }, true);
  assert.ok(rendered.output.endsWith('\n\nThe text.'));
  assert.equal(rendered.modelInvoked, false);
  const translate = load('translate.json');
  assert.equal(translate.content.inputs.lang_code.required, true);
  const out = renderPrompt(translate, { input: 'Hola', lang_code: 'en-us' }, true).output;
  assert.ok(out.includes('language code en-us'));
  assert.throws(() => renderPrompt(translate, { input: 'Hola' }, true), { code: 'MISSING_INPUT' });
});
