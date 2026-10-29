// The /fleet "SSH access checks" panel: @profullstack/fleetbar mounted at /api/fleetbar,
// proxying to the crawlproof partner uptime API as the signed-in user.
import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash, randomBytes } from 'node:crypto';
import { startServer } from '../src/server.mjs';
import { openStore } from '../src/store.mjs';
import { FLEETBAR_STYLE_SRC } from '../src/fleet-routes.mjs';
import { STYLES } from '@profullstack/fleetbar/element';

const ORIGIN = 'http://localhost:3000';
const TOKEN = 'cpu_test_partner_token';
const MONITORS = {
  monitors: [
    {
      id: 'mon_1',
      name: 'SSH root@dev2',
      type: 'ssh',
      target: 'dev2.example.com:22',
      enabled: true,
      interval_s: 300,
      current_state: 'up',
      last_checked_at: '2026-10-29T00:00:00Z',
      last_error: null,
      last_response_ms: 120,
      ssh: {
        username: 'root',
        fingerprint: 'SHA256:abc',
        key_comment: 'me',
        key_source: 'keyring',
      },
    },
  ],
};

async function boot({ emailVerified } = {}) {
  const calls = [];
  const fetchStub = async (url, init = {}) => {
    calls.push({ url: String(url), init, headers: new Headers(init.headers) });
    return new Response(JSON.stringify(MONITORS), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    });
  };
  const store = await openStore({ path: ':memory:' });
  const instance = await startServer({
    store,
    port: 0,
    publicOrigin: ORIGIN,
    mailer: { send: async () => ({ sent: true }) },
    fleetbar: {
      token: TOKEN,
      crawlproofUrl: 'https://crawlproof.test',
      fetch: fetchStub,
      ...(emailVerified ? { emailVerified } : {}),
    },
  });
  const base = `http://localhost:${instance.server.address().port}`;
  const accounts = store.accounts(ORIGIN);
  const ids = new Map();
  const user = async (username) => {
    const signup = await accounts.startSignup({ username, email: `${username}@example.com` });
    const { user: created } = await accounts.consumeLink(signup.link);
    const cookie = `w3bs_session=${await accounts.createSession(created.id)}`;
    ids.set(cookie, created.id);
    return cookie;
  };
  const appToken = async (cookie) => {
    const redirect = 'https://app.example/callback';
    const clientId = (
      await accounts.createClient(ids.get(cookie), { name: 'Some app', redirectUris: [redirect] })
    ).id;
    const verifier = randomBytes(32).toString('base64url');
    const request = await accounts.authorizationRequest({
      client_id: clientId,
      redirect_uri: redirect,
      response_type: 'code',
      code_challenge: createHash('sha256').update(verifier).digest('base64url'),
      code_challenge_method: 'S256',
      scope: 'profile',
    });
    const code = await accounts.issueCode(ids.get(cookie), request);
    return (
      await accounts.exchange({
        grant_type: 'authorization_code',
        code,
        redirect_uri: redirect,
        client_id: clientId,
        code_verifier: verifier,
      })
    ).access_token;
  };
  const http = (path, { method = 'GET', body, cookie, bearer, headers = {} } = {}) =>
    fetch(`${base}${path}`, {
      redirect: 'manual',
      method,
      headers: {
        ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
        ...(cookie ? { Cookie: cookie } : {}),
        ...(bearer ? { Authorization: `Bearer ${bearer}` } : {}),
        ...headers,
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
  // An account Sign in with CoinPay created: placeholder email, no verified address.
  const coinpayUser = async (sub) => {
    const { user: created } = await accounts.coinpayLogin({
      sub,
      email: 'cp@example.com',
      name: null,
    });
    return `w3bs_session=${await accounts.createSession(created.id)}`;
  };
  return { http, user, coinpayUser, accounts, ids, appToken, calls, close: () => instance.close() };
}

test('fleetbar: no session is 401 and nothing reaches crawlproof', async () => {
  const app = await boot();
  try {
    const res = await app.http('/api/fleetbar/monitors');
    assert.equal(res.status, 401);
    assert.equal((await res.json()).error, 'unauthorized');
    assert.equal(app.calls.length, 0);
  } finally {
    await app.close();
  }
});

test('fleetbar: an OAuth app bearer does not act as the user', async () => {
  const app = await boot();
  try {
    const cookie = await app.user('alice');
    const bearer = await app.appToken(cookie);
    const res = await app.http('/api/fleetbar/monitors', { bearer });
    assert.equal(res.status, 401);
    assert.equal(app.calls.length, 0);
  } finally {
    await app.close();
  }
});

test('fleetbar: an unverified email is 403', async () => {
  const app = await boot({ emailVerified: () => false });
  try {
    const cookie = await app.user('bob');
    const res = await app.http('/api/fleetbar/monitors', { cookie });
    assert.equal(res.status, 403);
    assert.equal((await res.json()).error, 'email_unverified');
    assert.equal(app.calls.length, 0);
  } finally {
    await app.close();
  }
});

test('fleetbar: a session proxies to crawlproof with the partner token and the email', async () => {
  const app = await boot();
  try {
    const cookie = await app.user('carol');
    const res = await app.http('/api/fleetbar/monitors?x=1', { cookie });
    assert.equal(res.status, 200);
    assert.deepEqual(await res.json(), MONITORS);
    assert.equal(app.calls.length, 1);
    const [call] = app.calls;
    assert.equal(call.url, 'https://crawlproof.test/api/uptime/v1/monitors');
    assert.equal(call.headers.get('authorization'), `Bearer ${TOKEN}`);
    assert.equal(call.headers.get('x-fleet-user-email'), 'carol@example.com');
    assert.equal(call.headers.get('cookie'), null);
  } finally {
    await app.close();
  }
});

test('fleetbar: a CoinPay-only account goes up by CoinPay sub, never with an email', async () => {
  const app = await boot();
  try {
    const cookie = await app.coinpayUser('cp_sub_only');
    const res = await app.http('/api/fleetbar/monitors', { cookie });
    assert.equal(res.status, 200);
    assert.equal(app.calls[0].headers.get('x-fleet-user-coinpay-sub'), 'cp_sub_only');
    assert.equal(app.calls[0].headers.get('x-fleet-user-email'), null);
  } finally {
    await app.close();
  }
});

test('fleetbar: a linked account sends both its verified email and its CoinPay sub', async () => {
  const app = await boot();
  try {
    const cookie = await app.user('carol');
    assert.equal(
      await app.accounts.coinpayLink(app.ids.get(cookie), { sub: 'cp_carol' }),
      'linked',
    );
    const res = await app.http('/api/fleetbar/monitors', { cookie });
    assert.equal(res.status, 200);
    assert.equal(app.calls[0].headers.get('x-fleet-user-coinpay-sub'), 'cp_carol');
    assert.equal(app.calls[0].headers.get('x-fleet-user-email'), 'carol@example.com');
  } finally {
    await app.close();
  }
});

test('fleetbar: same-origin writes pass, cross-origin writes are 403', async () => {
  const app = await boot();
  try {
    const cookie = await app.user('dave');
    const body = { name: 'web', type: 'http', target: 'https://example.com' };
    const cross = await app.http('/api/fleetbar/monitors', {
      method: 'POST',
      body,
      cookie,
      headers: { Origin: 'https://evil.example' },
    });
    assert.equal(cross.status, 403);
    assert.equal(app.calls.length, 0);
    const same = await app.http('/api/fleetbar/monitors', {
      method: 'POST',
      body,
      cookie,
      // What a browser sends for a same-origin fetch without an Origin header.
      headers: { 'Sec-Fetch-Site': 'same-origin' },
    });
    assert.equal(same.status, 200);
    assert.equal(app.calls.length, 1);
    assert.equal(app.calls[0].init.method, 'POST');
    assert.deepEqual(JSON.parse(app.calls[0].init.body), body);
  } finally {
    await app.close();
  }
});

test('/fleet shows the panel, loads its script and allows its styles by hash', async () => {
  const app = await boot();
  try {
    const cookie = await app.user('erin');
    const res = await app.http('/fleet', { cookie });
    assert.equal(res.status, 200);
    const html = await res.text();
    assert.match(html, /<h3>SSH access checks<\/h3>/);
    assert.match(html, /<fleet-bar endpoint="\/api\/fleetbar"/);
    assert.ok(html.indexOf('fleet-inventory') < html.indexOf('<fleet-bar'));
    assert.match(html, /<script src="\/vendor\/fleetbar\/fleetbar.js" defer><\/script>/);
    const csp = res.headers.get('content-security-policy');
    assert.ok(csp.includes(`style-src ${FLEETBAR_STYLE_SRC};`));
    const hash = createHash('sha256').update(STYLES).digest('base64');
    assert.ok(csp.includes(`'sha256-${hash}'`));
    const script = await app.http('/vendor/fleetbar/fleetbar.js');
    assert.equal(script.status, 200);
    const source = await script.text();
    assert.ok(source.includes('fleet-bar'));
    // The hash only holds while the built element ships these exact styles.
    assert.ok(source.includes(STYLES));
    // Other pages keep the plain policy.
    const home = await app.http('/');
    assert.match(home.headers.get('content-security-policy'), /style-src 'self';/);
  } finally {
    await app.close();
  }
});
