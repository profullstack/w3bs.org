import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { readFileSync, writeFileSync, readdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { startServer } from '../src/server.mjs';
import { openStore, ROOT } from '../src/store.mjs';

const exec = promisify(execFile);
function sourceFingerprint() {
  const hash = createHash('sha256');
  for (const directory of ['src', 'native', 'public']) {
    for (const filename of readdirSync(resolve(ROOT, directory)).sort()) {
      hash.update(`${directory}/${filename}\0`);
      hash.update(readFileSync(resolve(ROOT, directory, filename)));
    }
  }
  hash.update(readFileSync(resolve(ROOT, 'package-lock.json')));
  return hash.digest('hex');
}
export async function conformance() {
  const instance = await startServer({
    store: await openStore({ path: ':memory:' }),
    port: 0,
    host: '127.0.0.1',
  });
  const origin = `http://127.0.0.1:${instance.server.address().port}`;
  const uri = 'w3bs://prompt/w3bs/research@1';
  const clients = [];
  try {
    const api = await (await fetch(`${origin}/api/resolve?uri=${encodeURIComponent(uri)}`)).json();
    const env = {
      PATH: process.env.PATH,
      W3BS_API: origin,
      W3BS_TRUST_FILE: resolve(ROOT, 'fixtures/trust.json'),
    };
    const cli = JSON.parse(
      (await exec(process.execPath, [resolve(ROOT, 'src/cli.mjs'), 'resolve', uri], { env }))
        .stdout,
    );
    const native = JSON.parse(
      (await exec(process.execPath, [resolve(ROOT, 'native/client.mjs'), uri, '--json'], { env }))
        .stdout,
    );
    const html = await (await fetch(`${origin}/browse?uri=${encodeURIComponent(uri)}`)).text();
    const web = JSON.parse(html.match(/id="w3bs-resource">([\s\S]*?)<\/script>/)[1]);
    const httpClient = new Client({ name: 'w3bs-conformance-http', version: '1.0.0' });
    clients.push(httpClient);
    await httpClient.connect(new StreamableHTTPClientTransport(new URL(`${origin}/mcp`)));
    const mcpHttp = (await httpClient.callTool({ name: 'w3bs_resolve', arguments: { uri } }))
      .structuredContent;
    const stdioClient = new Client({ name: 'w3bs-conformance-stdio', version: '1.0.0' });
    clients.push(stdioClient);
    await stdioClient.connect(
      new StdioClientTransport({
        command: process.execPath,
        args: [resolve(ROOT, 'src/mcp-stdio.mjs')],
        env,
        stderr: 'pipe',
      }),
    );
    const mcpStdio = (await stdioClient.callTool({ name: 'w3bs_resolve', arguments: { uri } }))
      .structuredContent;
    const expectedManifest = JSON.parse(
      readFileSync(resolve(ROOT, 'fixtures/resources/research.json'), 'utf8'),
    );
    assert.deepEqual(api.manifest, expectedManifest);
    assert.equal(api.verification.valid, true);
    const surfaces = { api, cli, web, mcpHttp, mcpStdio, native };
    for (const [name, result] of Object.entries(surfaces))
      assert.deepEqual(result, api, `${name} disagreed about the canonical resource`);
    for (const [name, client] of [
      ['http', httpClient],
      ['stdio', stdioClient],
    ]) {
      const tools = await client.listTools();
      assert.equal(tools.tools.length, 8, `${name} missing shared operations`);
      const denied = await client.callTool({
        name: 'w3bs_run',
        arguments: { uri, inputs: { topic: 'open web' }, consent: false },
      });
      assert.equal(denied.isError, true);
      const rendered = await client.callTool({
        name: 'w3bs_run',
        arguments: { uri, inputs: { topic: 'open web' }, consent: true },
      });
      assert.equal(rendered.structuredContent.modelInvoked, false);
      assert.ok(rendered.structuredContent.output.includes('open web'));
    }
    return {
      status: 'passed',
      testedAt: new Date().toISOString(),
      implementationVersion: '0.1.0',
      sourceFingerprint: sourceFingerprint(),
      nodeVersion: process.version,
      environment: 'isolated loopback registry, not production',
      resource: uri,
      canonicalUri: api.canonicalUri,
      digest: api.verification.digest,
      surfaces: Object.keys(surfaces).map((name) => ({
        name,
        status: 'passed',
        checks: [
          'identity',
          'version',
          'full manifest',
          'permissions',
          'provenance',
          'signature',
          'revocation',
          'effective authority',
        ],
      })),
      limitations: [
        'One reference stack with an independently implemented native verifier; no claim of independent organizational implementations.',
        'Web envelope conformance is supplemented by Playwright browser tests.',
        'No production availability or Recommendation status is implied.',
      ],
    };
  } finally {
    for (const client of clients) await client.close();
    await instance.close();
  }
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const report = await conformance();
  writeFileSync(
    resolve(ROOT, 'fixtures/conformance-report.json'),
    JSON.stringify(report, null, 2) + '\n',
  );
  process.stdout.write(JSON.stringify(report, null, 2) + '\n');
}
