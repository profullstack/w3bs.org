import test from 'node:test';
import assert from 'node:assert/strict';
import { generateKeyPairSync } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import {
  parseUri,
  validateManifest,
  signManifest,
  verifyManifest,
  renderPrompt,
} from '../src/core.mjs';
import { openStore } from '../src/store.mjs';
import { dispatch } from '../src/operations.mjs';

const fixture = JSON.parse(
  readFileSync(new URL('../fixtures/resources/research.json', import.meta.url)),
);
const trust = JSON.parse(readFileSync(new URL('../fixtures/trust.json', import.meta.url)));
function publisher() {
  const { privateKey, publicKey } = generateKeyPairSync('ed25519');
  return {
    privateKey,
    trust: {
      keys: [
        {
          id: 'test-key',
          namespace: 'example',
          publicKeyPem: publicKey.export({ type: 'spki', format: 'pem' }),
          revoked: false,
        },
      ],
    },
  };
}
function signed(publisher, version = '1.0.0', mutate = () => {}) {
  const manifest = structuredClone(fixture);
  delete manifest.proof;
  manifest.id = `w3bs://prompt/example/research@${version}`;
  manifest.version = version;
  manifest.publisher = { namespace: 'example', name: 'Example', keyId: 'test-key' };
  mutate(manifest);
  return signManifest(manifest, publisher.privateKey);
}

test('URI grammar accepts PRD examples and rejects normalization ambiguities', () => {
  for (const uri of [
    'w3bs://prompt/open/researcher@2',
    'w3bs://behavior/open/code-review@4',
    'w3bs://agent/acme/support',
    'w3bs://mcp/example/filesystem',
    'w3bs://schema/logicsrc/task@1',
    'w3bs://eval/security/prompt-injection@1',
    'w3bs://device/example/camera-17',
  ])
    assert.ok(parseUri(uri));
  for (const uri of [
    'https://prompt/w3bs/research',
    'w3bs://PROMPT/w3bs/research',
    'w3bs://prompt/w3bs/../research',
    'w3bs://prompt/w3bs/%72esearch',
    'w3bs://prompt/w3bs/research@01',
    'w3bs://prompt/w3bs/research@1.2',
    'w3bs://prompt/w3bs/research@1.0.0-beta',
    'w3bs://prompt/w3bs/research?x=1',
    'w3bs://prompt/w3bs/research#x',
    'w3bs://prompt/w3bs/research@1000000000',
    'w3bs://prompt/user@host/research',
    null,
  ])
    assert.throws(() => parseUri(uri));
});
test('all ten examples verify and permission/content tampering fails', () => {
  assert.equal(verifyManifest(fixture, trust).valid, true);
  for (const mutate of [
    (m) => {
      m.content.instructions += ' changed';
    },
    (m) => {
      m.permissions = ['network'];
    },
    (m) => {
      m.provenance.source = 'https://attacker.example';
    },
  ]) {
    const bad = structuredClone(fixture);
    mutate(bad);
    assert.equal(verifyManifest(bad, trust).valid, false);
  }
});
test('namespace binding rejects a signature by an unapproved publisher key', () => {
  const p = publisher(),
    manifest = signed(p);
  const wrongNamespace = structuredClone(p.trust);
  wrongNamespace.keys[0].namespace = 'attacker';
  const result = verifyManifest(manifest, wrongNamespace);
  assert.equal(result.signatureValid, true);
  assert.equal(result.publisherTrusted, false);
  assert.equal(result.valid, false);
  assert.equal(verifyManifest(manifest, { keys: [] }).valid, false);
});
test('schema rejects unsigned, inconsistent and extended manifests', () => {
  const missing = structuredClone(fixture);
  delete missing.proof;
  assert.throws(() => validateManifest(missing), { code: 'SIGNATURE_REQUIRED' });
  assert.throws(() => validateManifest({ ...fixture, version: '2.0.0' }), {
    code: 'IDENTITY_MISMATCH',
  });
  assert.throws(() => validateManifest({ ...fixture, extra: 'unknown' }), {
    code: 'INVALID_MANIFEST',
  });
  assert.throws(
    () => validateManifest({ ...fixture, dependencies: ['w3bs://tool/example/read@1'] }),
    { code: 'UNPINNED_DEPENDENCY' },
  );
});
test('signature and declared SYSTEM_POLICY cannot grant instruction authority', () => {
  const p = publisher(),
    manifest = signed(p, '1.0.0', (m) => {
      m.trust.classification = 'SYSTEM_POLICY';
    });
  const result = verifyManifest(manifest, p.trust);
  assert.equal(result.valid, true);
  assert.equal(result.authority, 'DATA');
  assert.throws(() => renderPrompt(manifest, { topic: 'test' }), { code: 'CONSENT_REQUIRED' });
});
test('template execution requires consent and treats inserted template syntax as data', () => {
  assert.throws(() => renderPrompt(fixture, {}, true), { code: 'MISSING_INPUT' });
  assert.throws(() => renderPrompt(fixture, { topic: 'x', surprise: 'y' }, true), {
    code: 'INVALID_INPUT',
  });
  const result = renderPrompt(fixture, { topic: '{{topic}} and $& are untrusted input' }, true);
  assert.ok(result.output.includes('{{topic}} and $& are untrusted input'));
  assert.equal(result.modelInvoked, false);
  assert.deepEqual(result.toolsInvoked, []);
  const withTools = structuredClone(fixture);
  withTools.content.tools = ['w3bs://tool/example/read@1.0.0'];
  assert.throws(() => renderPrompt(withTools, { topic: 'x' }, true), {
    code: 'UNSUPPORTED_CAPABILITY',
  });
});
test('numeric aliases, immutable publication and revoked latest versions are preserved', () => {
  const p = publisher(),
    store = openStore({ path: ':memory:', trustStore: p.trust, seed: false });
  try {
    store.publish(signed(p, '1.2.0'));
    store.publish(signed(p, '1.10.0'));
    store.publish(signed(p, '2.0.0'));
    assert.equal(store.inspect('w3bs://prompt/example/research@1').manifest.version, '1.10.0');
    assert.equal(store.inspect('w3bs://prompt/example/research').manifest.version, '2.0.0');
    assert.throws(
      () =>
        store.publish(
          signed(p, '1.2.0', (m) => {
            m.name = 'Changed';
          }),
        ),
      { code: 'IMMUTABLE_VERSION' },
    );
    store.revoke('w3bs://prompt/example/research@1.10.0', 'Withdraw unsafe example');
    const result = store.inspect('w3bs://prompt/example/research@1');
    assert.equal(result.manifest.version, '1.10.0');
    assert.equal(result.verification.revoked, true);
    assert.throws(
      () =>
        dispatch(store, 'run', { uri: result.canonicalUri, inputs: { topic: 'x' }, consent: true }),
      { code: 'VERIFICATION_FAILED' },
    );
  } finally {
    store.close();
  }
});
test('publication and revocation survive reopening a persistent registry', () => {
  const directory = mkdtempSync(resolve(tmpdir(), 'w3bs-store-')),
    p = publisher();
  const config = { path: resolve(directory, 'test.sqlite'), trustStore: p.trust, seed: false };
  let store = openStore(config);
  try {
    store.publish(signed(p));
    store.revoke('w3bs://prompt/example/research@1.0.0', 'Withdrawn');
    store.close();
    store = openStore(config);
    assert.equal(store.inspect('w3bs://prompt/example/research@1').verification.revoked, true);
  } finally {
    store.close();
    rmSync(directory, { recursive: true });
  }
});
test('administrative operations fail closed when authorization is missing', () => {
  const store = openStore({ path: ':memory:' });
  try {
    assert.equal(store.search().length, 10);
    for (const result of store.search()) assert.equal(result.verification.valid, true);
    assert.throws(() => dispatch(store, 'publish', { manifest: fixture }, { publishToken: '' }), {
      code: 'PUBLISHING_DISABLED',
    });
    assert.throws(
      () =>
        dispatch(
          store,
          'publish',
          { manifest: fixture },
          { publishToken: 'a'.repeat(32), token: 'wrong' },
        ),
      { code: 'UNAUTHORIZED' },
    );
    assert.equal(
      dispatch(
        store,
        'publish',
        { manifest: fixture },
        { publishToken: 'a'.repeat(32), token: 'a'.repeat(32) },
      ).verification.valid,
      true,
    );
  } finally {
    store.close();
  }
});
