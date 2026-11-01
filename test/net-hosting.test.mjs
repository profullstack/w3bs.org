import test from 'node:test';
import assert from 'node:assert/strict';
import { request as httpRequest } from 'node:http';
import { startServer } from '../src/server.mjs';
import { openStore } from '../src/store.mjs';

function requestHost(port, path, host = 'mint.agent.w3bs.org') {
  return new Promise((resolve, reject) => {
    const request = httpRequest(
      `http://127.0.0.1:${port}${path}`,
      { headers: { Host: host } },
      (response) => {
        const chunks = [];
        response.on('data', (chunk) => chunks.push(chunk));
        response.on('end', () =>
          resolve({
            status: response.statusCode,
            headers: response.headers,
            body: Buffer.concat(chunks).toString('utf8'),
          }),
        );
      },
    );
    request.on('error', reject);
    request.end();
  });
}

test('an active W3BS name page serves its shell assets and links navigation to the primary site', async () => {
  const store = await openStore({ path: ':memory:' });
  store.network.resolveName = async (name) => ({
    name,
    source: 'w3bs',
    status: 'active',
    target: name === 'site.agent' ? 'https://example.org/' : null,
    claimed: true,
  });
  const instance = await startServer({
    store,
    port: 0,
    publicOrigin: 'http://localhost:3000',
    allowedHosts: ['localhost', '127.0.0.1'],
  });
  const port = instance.server.address().port;

  try {
    const page = await requestHost(port, '/');
    assert.equal(page.status, 200);
    assert.match(page.body, /<h1>mint\.agent<\/h1>/);
    assert.match(page.body, /href="http:\/\/localhost:3000\/net"/);
    assert.doesNotMatch(page.body, /<a\b[^>]*href="\/(?!\/)/);

    for (const [path, contentType] of [
      ['/assets/style.css', 'text/css'],
      ['/assets/app.js', 'javascript'],
      ['/manifest.webmanifest', 'application/manifest+json'],
      ['/sw.js', 'javascript'],
    ]) {
      const asset = await requestHost(port, path);
      assert.equal(asset.status, 200, path);
      assert.ok(asset.headers['content-type'].includes(contentType), path);
      assert.doesNotMatch(asset.body, /<html\b/i, path);
    }

    const redirectedAsset = await requestHost(port, '/assets/style.css', 'site.agent.w3bs.org');
    assert.equal(redirectedAsset.status, 302);
    assert.equal(redirectedAsset.headers.location, 'https://example.org/');
  } finally {
    await instance.close();
  }
});
