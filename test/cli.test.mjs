import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { startServer } from '../src/server.mjs';
import { openStore, ROOT } from '../src/store.mjs';

const exec = promisify(execFile);
test(
  'CLI publisher lifecycle: keygen, fork, sign, publish, render, version, install and revoke',
  { timeout: 30000 },
  async () => {
    const directory = mkdtempSync(resolve(tmpdir(), 'w3bs-cli-'));
    const cli = resolve(ROOT, 'src/cli.mjs');
    let instance;
    try {
      await exec(process.execPath, [
        cli,
        'keygen',
        '--out',
        resolve(directory, 'keys'),
        '--namespace',
        'example',
      ]);
      assert.equal(statSync(resolve(directory, 'keys/private.pem')).mode & 0o777, 0o600);
      const trust = JSON.parse(readFileSync(resolve(ROOT, 'fixtures/trust.json'), 'utf8'));
      trust.keys.push(JSON.parse(readFileSync(resolve(directory, 'keys/public.json'), 'utf8')));
      const trustPath = resolve(directory, 'trust.json');
      writeFileSync(trustPath, JSON.stringify(trust));
      const token = 'c'.repeat(32);
      instance = await startServer({
        store: await openStore({ path: ':memory:', trustStore: trust }),
        port: 0,
        publishToken: token,
      });
      const env = {
        PATH: process.env.PATH,
        W3BS_API: `http://127.0.0.1:${instance.server.address().port}`,
        W3BS_TRUST_FILE: trustPath,
        W3BS_PUBLISH_TOKEN: token,
      };
      const run = async (...args) =>
        JSON.parse((await exec(process.execPath, [cli, ...args], { env })).stdout);
      const draft = resolve(directory, 'draft.json'),
        signed = resolve(directory, 'signed.json');
      await run(
        'fork',
        'w3bs://prompt/w3bs/research@1',
        '--namespace',
        'example',
        '--name',
        'research',
        '--key-id',
        'example-1',
        '--out',
        draft,
      );
      await run('sign', draft, '--key', resolve(directory, 'keys/private.pem'), '--out', signed);
      const publication = await run('publish', signed);
      assert.equal(publication.verification.valid, true);
      const uri = publication.canonicalUri,
        inputs = resolve(directory, 'inputs.json');
      writeFileSync(inputs, JSON.stringify({ topic: 'portable behavior' }));
      assert.ok(
        (await run('run', uri, '--inputs', inputs, '--consent')).output.includes(
          'portable behavior',
        ),
      );
      const revision = resolve(directory, 'revision.json'),
        revisionSigned = resolve(directory, 'revision-signed.json');
      await run('version', uri, '--version', '1.1.0', '--out', revision);
      await run(
        'sign',
        revision,
        '--key',
        resolve(directory, 'keys/private.pem'),
        '--out',
        revisionSigned,
      );
      const next = await run('publish', revisionSigned);
      assert.equal(next.manifest.version, '1.1.0');
      const installed = resolve(directory, 'installed.json');
      await run('install', 'w3bs://prompt/example/research@1', '--out', installed);
      assert.deepEqual(JSON.parse(readFileSync(installed, 'utf8')), next.manifest);
      assert.equal(
        (await run('revoke', next.canonicalUri, '--reason', 'Withdrawn example')).verification
          .revoked,
        true,
      );
      await assert.rejects(
        () => run('verify', next.canonicalUri),
        (error) => error.code === 1,
      );
    } finally {
      if (instance) await instance.close();
      rmSync(directory, { recursive: true });
    }
  },
);
