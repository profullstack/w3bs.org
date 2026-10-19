import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import express from 'express';
import { mountRegistryFront } from '../src/registry-front.mjs';

const RELEASE = 'https://github.com/profullstack/w3bs.org/releases/download/os-images';
const digest = (s) => `sha256:${createHash('sha256').update(s).digest('hex')}`;
const index = JSON.stringify({
  schemaVersion: 2,
  mediaType: 'application/vnd.oci.image.index.v1+json',
  manifests: [],
});
const indexDigest = digest(index);

async function boot(files) {
  const app = express();
  mountRegistryFront(app, {
    fetchImpl: async (url) => {
      const body = files[url.slice(RELEASE.length + 1)];
      return body === undefined ? new Response('nope', { status: 404 }) : new Response(body);
    },
  });
  const server = await new Promise((r) => {
    const s = app.listen(0, () => r(s));
  });
  const base = `http://localhost:${server.address().port}`;
  return { base, close: () => server.close() };
}

test('registry front: tags and digests serve the exact manifest bytes with the registry headers', async () => {
  const t = await boot({
    'tags.json': JSON.stringify({ server: indexDigest }),
    [indexDigest.replace(':', '-')]: index,
  });
  try {
    const root = await fetch(`${t.base}/v2/`);
    assert.equal(root.headers.get('docker-distribution-api-version'), 'registry/2.0');
    for (const ref of ['server', indexDigest]) {
      const r = await fetch(`${t.base}/v2/os/manifests/${ref}`);
      assert.equal(r.status, 200);
      assert.equal(r.headers.get('content-type'), 'application/vnd.oci.image.index.v1+json');
      assert.equal(r.headers.get('docker-content-digest'), indexDigest);
      assert.equal(await r.text(), index);
    }
    const head = await fetch(`${t.base}/v2/w3bs-os/manifests/server`, { method: 'HEAD' });
    assert.equal(head.status, 200);
    const blob = await fetch(`${t.base}/v2/os/blobs/${indexDigest}`, { redirect: 'manual' });
    assert.equal(blob.status, 307);
    assert.equal(blob.headers.get('location'), `${RELEASE}/${indexDigest.replace(':', '-')}`);
    assert.equal((await fetch(`${t.base}/v2/os/manifests/nope`)).status, 404);
    assert.equal((await fetch(`${t.base}/v2/other/manifests/server`)).status, 404);
    assert.equal(
      (await fetch(`${t.base}/v2/os/blobs/sha256:xyz`, { redirect: 'manual' })).status,
      404,
    );
    assert.equal((await fetch(`${t.base}/v2/os/manifests/server`, { method: 'PUT' })).status, 405);
  } finally {
    t.close();
  }
});

test('registry front: a stored manifest that does not hash to its name is never served', async () => {
  const forged = index.replace('"manifests":[]', '"manifests":[{"x":1}]');
  const t = await boot({
    'tags.json': JSON.stringify({ server: indexDigest }),
    [indexDigest.replace(':', '-')]: forged,
  });
  try {
    const r = await fetch(`${t.base}/v2/os/manifests/server`);
    assert.equal(r.status, 502);
    assert.match(await r.text(), /does not match its digest/);
  } finally {
    t.close();
  }
});
