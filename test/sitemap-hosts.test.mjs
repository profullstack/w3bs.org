// browse., prompt. and specs. served the apex's sitemap and robots Sitemap line, so
// each section host advertised w3bs.org URLs. These pin the per-host answers.
import test from 'node:test';
import assert from 'node:assert/strict';
import { request } from 'node:http';
import { startServer } from '../src/server.mjs';
import { openStore } from '../src/store.mjs';

function get(port, path, host) {
  return new Promise((resolve, reject) => {
    const req = request({ host: '127.0.0.1', port, path, headers: { host } }, (res) => {
      let body = '';
      res.setEncoding('utf8');
      res.on('data', (chunk) => (body += chunk));
      res.on('end', () => resolve({ status: res.statusCode, type: res.headers['content-type'] ?? '', body }));
    });
    req.on('error', reject);
    req.end();
  });
}

test('section hosts list their own home; the apex keeps its full sitemap', async () => {
  const store = await openStore({ path: ':memory:' });
  const instance = await startServer({
    store,
    port: 0,
    publicOrigin: 'https://w3bs.example',
    allowedHosts: ['w3bs.example', 'browse.w3bs.example', 'prompt.w3bs.example', 'specs.w3bs.example'],
    mailer: { send: async () => ({ sent: true }) },
  });
  const port = instance.server.address().port;
  try {
    for (const section of ['browse', 'prompt', 'specs']) {
      const host = `${section}.w3bs.example`;
      const sitemap = await get(port, '/sitemap.xml', host);
      assert.equal(sitemap.status, 200);
      assert.match(sitemap.type, /application\/xml/);
      const locs = [...sitemap.body.matchAll(/<loc>([^<]+)<\/loc>/g)].map((m) => m[1]);
      assert.deepEqual(locs, [`https://${host}/`]);
      const robots = await get(port, '/robots.txt', host);
      assert.match(robots.body, new RegExp(`^Sitemap: https://${host.replaceAll('.', '\\.')}/sitemap\\.xml$`, 'm'));
    }
    const apex = await get(port, '/sitemap.xml', 'w3bs.example');
    assert.ok(apex.body.includes('<loc>https://w3bs.example/pricing</loc>'));
    const robots = await get(port, '/robots.txt', 'w3bs.example');
    assert.match(robots.body, /^Sitemap: https:\/\/w3bs\.example\/sitemap\.xml$/m);
  } finally {
    await instance.close();
  }
});
