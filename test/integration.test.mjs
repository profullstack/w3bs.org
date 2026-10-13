import test from 'node:test';
import assert from 'node:assert/strict';
import { generateKeyPairSync } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { createServer, request as httpRequest } from 'node:http';
import { startServer } from '../src/server.mjs';
import { openStore } from '../src/store.mjs';
import { signManifest } from '../src/core.mjs';
import { nativeResolve } from '../native/client.mjs';
import { request } from '../src/client.mjs';
import { conformance } from '../scripts/conformance.mjs';

test(
  'PRD acceptance: canonical resource agrees over six real surfaces',
  { timeout: 30000 },
  async () => {
    const report = await conformance();
    assert.equal(report.status, 'passed');
    assert.equal(report.surfaces.length, 6);
  },
);
test('HTTP rejects hostile origins, arbitrary hosts, invalid bodies and unauthenticated writes', async () => {
  const instance = await startServer({
    store: await openStore({ path: ':memory:' }),
    port: 0,
    publicOrigin: 'http://localhost:3000',
    publishToken: 'a'.repeat(32),
  });
  const origin = `http://127.0.0.1:${instance.server.address().port}`;
  try {
    assert.equal((await fetch(`${origin}/healthz`)).status, 200);
    assert.equal(
      (await fetch(`${origin}/api/search`, { headers: { Origin: 'https://evil.example' } })).status,
      403,
    );
    // Fetch owns Host; use a raw HTTP request to exercise the actual server boundary.
    const hostileHost = await new Promise((resolve, reject) => {
      const req = httpRequest(
        `${origin}/api/search`,
        { headers: { Host: 'evil.example' } },
        (res) => {
          res.resume();
          resolve(res.statusCode);
        },
      );
      req.on('error', reject);
      req.end();
    });
    assert.equal(hostileHost, 403);
    const healthStatus = await new Promise((resolve, reject) => {
      const req = httpRequest(
        `${origin}/healthz`,
        { headers: { Host: 'healthcheck.railway.app' } },
        (res) => {
          res.resume();
          resolve(res.statusCode);
        },
      );
      req.on('error', reject);
      req.end();
    });
    assert.equal(healthStatus, 200);
    const healthOtherRoute = await new Promise((resolve, reject) => {
      const req = httpRequest(
        `${origin}/api/search`,
        { headers: { Host: 'healthcheck.railway.app' } },
        (res) => {
          res.resume();
          resolve(res.statusCode);
        },
      );
      req.on('error', reject);
      req.end();
    });
    assert.equal(healthOtherRoute, 403);
    assert.equal(
      (
        await fetch(`${origin}/api/search`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: '{broken',
        })
      ).status,
      400,
    );
    assert.equal(
      (
        await fetch(`${origin}/api/search`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ query: 'x'.repeat(300000) }),
        })
      ).status,
      413,
    );
    const fixture = JSON.parse(
      readFileSync(new URL('../fixtures/resources/research.json', import.meta.url)),
    );
    const denied = await fetch(`${origin}/api/publish`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ manifest: fixture }),
    });
    assert.equal(denied.status, 401);
    assert.equal(
      (await fetch(`${origin}/api/resolve?uri=w3bs://prompt/w3bs/missing@1`)).status,
      404,
    );
    const html = await (
      await fetch(`${origin}/prompts?q=${encodeURIComponent('<script>alert(1)</script>')}`)
    ).text();
    assert.ok(!html.includes('<script>alert(1)</script>'));
    assert.ok(html.includes('&lt;script&gt;'));
    const discovery = await (await fetch(`${origin}/.well-known/w3bs.json`)).json();
    assert.ok(discovery.operations.includes('publish'));
    assert.equal(
      (await fetch(`${origin}/api/verify?uri=w3bs://prompt/w3bs/research@1`)).headers.get(
        'cache-control',
      ),
      'no-store',
    );
    for (const route of [
      '/mission',
      '/specs',
      '/prompts',
      '/browse',
      '/governance',
      '/developers',
      '/specs/W3BS-URI-1',
      '/docs/founding-prd.md',
      '/manifest.webmanifest',
      '/sw.js',
    ])
      assert.equal((await fetch(origin + route)).status, 200, route);
  } finally {
    await instance.close();
  }
});
test('a new publisher can publish, render, revoke and inspect through HTTP', async () => {
  const { privateKey, publicKey } = generateKeyPairSync('ed25519');
  const trust = {
    keys: [
      {
        id: 'publisher-1',
        namespace: 'example',
        publicKeyPem: publicKey.export({ type: 'spki', format: 'pem' }),
        revoked: false,
      },
    ],
  };
  const fixture = JSON.parse(
    readFileSync(new URL('../fixtures/resources/research.json', import.meta.url)),
  );
  delete fixture.proof;
  fixture.id = 'w3bs://prompt/example/research@1.0.0';
  fixture.publisher = { namespace: 'example', name: 'Example', keyId: 'publisher-1' };
  const manifest = signManifest(fixture, privateKey),
    token = 'b'.repeat(32);
  const instance = await startServer({
    store: await openStore({ path: ':memory:', trustStore: trust, seed: false }),
    port: 0,
    publishToken: token,
  });
  const origin = `http://127.0.0.1:${instance.server.address().port}`;
  const invoke = (operation, args) =>
    request(operation, args, { origin, token, trustStore: trust });
  try {
    assert.equal((await invoke('publish', { manifest })).verification.valid, true);
    const result = await invoke('run', {
      uri: manifest.id,
      inputs: { topic: 'interoperability' },
      consent: true,
    });
    assert.ok(result.output.includes('interoperability'));
    assert.equal(result.modelInvoked, false);
    assert.equal(
      (await invoke('revoke', { uri: manifest.id, reason: 'Withdraw example' })).verification
        .revoked,
      true,
    );
    await assert.rejects(
      () =>
        invoke('run', { uri: manifest.id, inputs: { topic: 'interoperability' }, consent: true }),
      { code: 'VERIFICATION_FAILED' },
    );
  } finally {
    await instance.close();
  }
});
test('independent clients reject a valid signed artifact returned for the wrong URI', async () => {
  const store = await openStore({ path: ':memory:' }),
    wrong = await store.inspect('w3bs://prompt/w3bs/code-review@1');
  wrong.requestedUri = 'w3bs://prompt/w3bs/research@1';
  const server = createServer((_req, res) => {
    res.setHeader('Content-Type', 'application/json');
    res.end(JSON.stringify(wrong));
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  try {
    await assert.rejects(() => nativeResolve(wrong.requestedUri, { origin }), /identity mismatch/);
    await assert.rejects(() => request('resolve', { uri: wrong.requestedUri }, { origin }), {
      code: 'SURFACE_MISMATCH',
    });
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
    await store.close();
  }
});
test('Agentic Desktop Layer page, typo redirect, raw PRD and navigation', async () => {
  const instance = await startServer({
    store: await openStore({ path: ':memory:' }),
    port: 0,
    publicOrigin: 'http://localhost:3000',
  });
  const origin = `http://127.0.0.1:${instance.server.address().port}`;
  try {
    const page = await fetch(`${origin}/desktop`);
    assert.equal(page.status, 200);
    const html = await page.text();
    for (const text of [
      'Interactive preview - does not control your device',
      'Copy Markdown',
      'Download PRD',
      'Your switch.',
      'No platform is certified yet.',
      'INSTALL AND USE',
      'curl -fsSL https://w3bs.org/desktop/install.sh | sh',
      'Enable agent access',
      'Disable agent access',
      '/assets/desktop.js',
    ])
      assert.ok(html.includes(text), `missing ${text}`);
    assert.match(html, /<a href="\/desktop" aria-current="page">Desktop<\/a>/);
    assert.ok(!html.includes('—Agentic') && !html.includes('Proposed, not yet installable'));
    const installer = await fetch(`${origin}/desktop/install.sh`);
    assert.equal(installer.status, 200);
    assert.match(installer.headers.get('content-type'), /shellscript/);
    const installScript = await installer.text();
    // Nothing runs until the last line, so a truncated download does nothing.
    assert.equal(installScript.trimEnd().split('\n').at(-1), 'adl_main "$@"');
    assert.ok(installScript.includes("SHA256SUMS") && !installScript.includes("enable.confirm"));
    const home = await (await fetch(`${origin}/mission`)).text();
    assert.match(home, /<a href="\/desktop" >Desktop<\/a>/);
    for (const path of ['/desktpo', '/desktpo/', '/desktop/']) {
      const redirect = await fetch(`${origin}${path}`, { redirect: 'manual' });
      assert.equal(redirect.status, 301, path);
      assert.equal(redirect.headers.get('location'), '/desktop', path);
    }
    const withQuery = await fetch(`${origin}/desktpo?ref=a`, { redirect: 'manual' });
    assert.equal(withQuery.headers.get('location'), '/desktop?ref=a');
    const prd = await fetch(`${origin}/docs/agentic-desktop-layer-prd.md`);
    assert.equal(prd.status, 200);
    assert.match(prd.headers.get('content-type'), /^text\/markdown/);
    const markdown = await prd.text();
    assert.ok(
      markdown.startsWith('# Agentic Desktop Layer\n## W3BS Product Requirements Document'),
    );
    assert.ok(markdown.includes('Interactive preview - does not control your device'));
    assert.ok((await (await fetch(`${origin}/sitemap.xml`)).text()).includes('/desktop</loc>'));
    const script = await (await fetch(`${origin}/assets/desktop.js`)).text();
    assert.ok(!/localhost|127\.0\.0\.1|requestPermission|getUserMedia/.test(script));
  } finally {
    await instance.close();
  }
});
