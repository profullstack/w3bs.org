import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { openStore } from '../src/store.mjs';
import { startServer } from '../src/server.mjs';

for (const databaseUrl of [null, ...(process.env.W3BS_TEST_DATABASE_URL ? [process.env.W3BS_TEST_DATABASE_URL] : [])]) {
  test(`name requests queue a missing namespace safely (${databaseUrl ? 'postgres' : 'sqlite'})`, async () => {
    const store = await openStore({ path: ':memory:', databaseUrl, seed: false });
    const app = await startServer({ store, port: 0, publicOrigin: 'http://localhost:3000' });
    const suffix = randomUUID().slice(0, 8);
    const tld = `new-${suffix}`;
    const name = `dumb.${tld}`;
    const target = 'https://fumb.duck.w3bs.org/';
    const accounts = store.accounts('http://localhost:3000');
    const signup = await accounts.startSignup({ username: `net-${suffix}`, email: `net-${suffix}@example.com` });
    const { user } = await accounts.consumeLink(signup.link);
    const session = await accounts.createSession(user.id);
    const base = `http://localhost:${app.server.address().port}`;
    const post = (body, signedIn = true) => fetch(`${base}/api/net/v1/names`, {
      method: 'POST', headers: { 'content-type': 'application/json', ...(signedIn ? { cookie: `w3bs_session=${session}` } : {}) }, body: JSON.stringify(body),
    });
    try {
      assert.equal((await post({ name, target, requestNamespace: true }, false)).status, 401);
      const missing = await post({ name, target });
      assert.equal(missing.status, 409);
      assert.match((await missing.json()).error.message, new RegExp(`\\.${tld} namespace is not registered`));
      assert.equal((await store.network.pending()).namespaces.some((n) => n.tld === tld), false);
      const invalid = await post({ name, target: 'http://not-https.example', requestNamespace: true });
      assert.equal(invalid.status, 400);
      assert.equal((await store.network.pending()).namespaces.some((n) => n.tld === tld), false);
      const reserved = await post({ name: 'dumb.admin', requestNamespace: true });
      assert.equal(reserved.status, 409);
      assert.equal((await reserved.json()).error.code, 'RESERVED_TLD');
      const queued = await post({ name, target, requestNamespace: true });
      assert.equal(queued.status, 202, await queued.clone().text());
      assert.deepEqual(await queued.json(), { name, status: 'pending_moderation', namespaceRequested: true, namespacePending: true });
      const pending = await store.network.pending();
      assert.equal(pending.namespaces.find((n) => n.tld === tld).owner, user.id);
      assert.equal(pending.namespaces.find((n) => n.tld === tld).policy.open, false);
      assert.equal(pending.names.find((n) => n.name === name).target, target);
      assert.notEqual((await store.network.resolveName(name)).status, 'active');
      assert.equal((await store.network.events()).some((e) => e.subject === name || e.subject === `.${tld}`), false);
      assert.deepEqual(await store.network.moderate({ kind: 'name', subject: name, action: 'approve', actor: user.id }), { error: 'namespace_not_active' });
      assert.equal((await store.network.requestName({ tld, label: 'outsider', ownerId: 'someone-else', requestNamespace: true })).error, 'namespace_unavailable');
      const ownPending = await post({ name: `second.${tld}`, target });
      assert.equal(ownPending.status, 202);
      assert.equal((await ownPending.json()).namespacePending, true);
      await store.network.moderate({ kind: 'namespace', subject: tld, action: 'approve', actor: user.id });
      assert.equal((await store.network.requestName({ tld, label: 'outsider', ownerId: 'someone-else', requestNamespace: true })).error, 'namespace_owner_required');
      await store.network.moderate({ kind: 'name', subject: name, action: 'approve', actor: user.id });
      assert.equal((await store.network.resolveName(name)).target, target);
      assert.equal((await post({ name, target, requestNamespace: true })).status, 409);
      await store.network.moderate({ kind: 'namespace', subject: tld, action: 'suspend', actor: user.id });
      const suspended = await post({ name: `third.${tld}`, requestNamespace: true });
      assert.equal(suspended.status, 409);
      assert.match((await suspended.json()).error.message, /suspended/);
      // Existing open namespaces continue accepting names without the new option.
      const open = await post({ name: `net-${suffix}.agent`, target });
      assert.equal(open.status, 202);
      assert.equal((await open.json()).namespacePending, false);
    } finally {
      await app.close();
    }
  });
}
