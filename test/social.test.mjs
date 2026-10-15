import test from 'node:test';
import assert from 'node:assert/strict';
import { startServer } from '../src/server.mjs';
import { openStore } from '../src/store.mjs';

const ORIGIN = 'http://localhost:3000';
const PDS = 'https://social.example';

// A stand-in PDS: describeServer, and createInviteCode behind admin Basic auth.
function fakePds({ online = true, codes = [] } = {}) {
  const calls = [];
  let n = 0;
  const fetchImpl = async (url, init = {}) => {
    calls.push({ url: String(url), init });
    if (!online) throw new Error('offline');
    if (String(url).endsWith('/xrpc/com.atproto.server.describeServer'))
      return Response.json({
        did: 'did:web:social.example',
        availableUserDomains: ['.social.example'],
        inviteCodeRequired: true,
      });
    if (String(url).includes('/xrpc/com.atproto.admin.getInviteCodes'))
      return Response.json({ codes });
    if (String(url).endsWith('/xrpc/com.atproto.server.createInviteCode')) {
      if (init.headers?.Authorization !== `Basic ${Buffer.from('admin:secret').toString('base64')}`)
        return new Response('{}', { status: 401 });
      return Response.json({ code: `social-example-${++n}` });
    }
    return new Response('{}', { status: 404 });
  };
  return { fetchImpl, calls };
}

async function boot({ adminPassword = 'secret', online = true, publicInvite = '', codes } = {}) {
  const pds = fakePds({ online, codes });
  const store = await openStore({ path: ':memory:' });
  const instance = await startServer({
    store,
    port: 0,
    publicOrigin: ORIGIN,
    mailer: { send: async () => ({ sent: true }) },
    social: { pds: PDS, adminPassword, publicInvite },
    socialFetch: pds.fetchImpl,
  });
  const base = `http://localhost:${instance.server.address().port}`;
  const accounts = store.accounts(ORIGIN);
  const user = async (username) => {
    const signup = await accounts.startSignup({ username, email: `${username}@example.com` });
    const { user: created } = await accounts.consumeLink(signup.link);
    return `w3bs_session=${await accounts.createSession(created.id)}`;
  };
  const http = (path, { cookie, method } = {}) =>
    fetch(`${base}${path}`, {
      redirect: 'manual',
      method: method ?? 'GET',
      headers: cookie ? { Cookie: cookie } : {},
    });
  return { http, user, pds, close: () => instance.close() };
}

test('social: public page and API describe the PDS', async () => {
  const t = await boot();
  try {
    const page = await t.http('/social');
    assert.equal(page.status, 200);
    const html = await page.text();
    assert.match(html, /social\.example/);
    assert.match(html, /Server online/);
    assert.match(html, /id="policy"/);
    assert.match(html, /Sign in for an invite/);
    assert.match(html, /href="\/social" aria-current="page"/);
    const api = await (await t.http('/api/social')).json();
    assert.equal(api.pds, PDS);
    assert.equal(api.did, 'did:web:social.example');
    assert.deepEqual(api.handleDomains, ['.social.example']);
    assert.equal(api.online, true);
    assert.equal(api.invitesFromW3bs, true);
    // describeServer is cached: two reads, one call.
    assert.equal(t.pds.calls.filter((c) => c.url.includes('describeServer')).length, 1);
  } finally {
    await t.close();
  }
});

test('social: one invite per account, kept across requests', async () => {
  const t = await boot();
  try {
    assert.equal((await t.http('/api/social/invite', { method: 'POST' })).status, 401);
    const alice = await t.user('alice');
    const first = await t.http('/api/social/invite', { method: 'POST', cookie: alice });
    assert.equal(first.status, 200);
    const body = await first.json();
    assert.equal(body.code, 'social-example-1');
    assert.equal(body.handle, 'alice.social.example');
    const again = await (
      await t.http('/api/social/invite', { method: 'POST', cookie: alice })
    ).json();
    assert.equal(again.code, 'social-example-1');
    const bob = await t.user('bob');
    const bobs = await (await t.http('/api/social/invite', { method: 'POST', cookie: bob })).json();
    assert.equal(bobs.code, 'social-example-2');
    const html = await (await t.http('/social', { cookie: alice })).text();
    assert.match(html, /social-example-1/);
    assert.equal(t.pds.calls.filter((c) => c.url.includes('createInviteCode')).length, 2);
  } finally {
    await t.close();
  }
});

test('social: without an admin password or a reachable PDS, the page still renders', async () => {
  const t = await boot({ adminPassword: '', online: false });
  try {
    const html = await (await t.http('/social')).text();
    assert.match(html, /Server not answering/);
    assert.match(html, /Invites are not open yet/);
    const alice = await t.user('alice');
    const res = await t.http('/api/social/invite', { method: 'POST', cookie: alice });
    assert.equal(res.status, 503);
    assert.equal((await res.json()).error.code, 'SOCIAL_UNAVAILABLE');
  } finally {
    await t.close();
  }
});

test('social: a shared invite is shown to everyone with its uses left', async () => {
  const codes = [{ code: 'shared-code', available: 1000, disabled: false, uses: [{}, {}] }];
  const t = await boot({ publicInvite: 'shared-code', codes });
  try {
    const html = await (await t.http('/social')).text();
    assert.match(html, /id="social-code">shared-code</);
    assert.match(html, /998 sign-ups left/);
    assert.doesNotMatch(html, /Sign in for an invite/);
    const api = await (await t.http('/api/social')).json();
    assert.deepEqual(api.publicInvite, { code: 'shared-code', remaining: 998 });
  } finally {
    await t.close();
  }
});

test('social: a used-up or disabled shared invite falls back to per-account invites', async () => {
  const codes = [{ code: 'shared-code', available: 2, disabled: false, uses: [{}, {}] }];
  const t = await boot({ publicInvite: 'shared-code', codes });
  try {
    const html = await (await t.http('/social')).text();
    assert.doesNotMatch(html, /shared-code/);
    assert.match(html, /Sign in for an invite/);
    assert.equal((await (await t.http('/api/social')).json()).publicInvite, null);
  } finally {
    await t.close();
  }
});
