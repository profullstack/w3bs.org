// Sign in with CoinPay on w3bs.org (relying party): bind by CoinPay sub only, never by email.
import test from 'node:test';
import assert from 'node:assert/strict';
import { startServer } from '../src/server.mjs';
import { openStore } from '../src/store.mjs';

const ORIGIN = 'http://localhost:3000';
const CP = 'https://coinpay.test';
const jwt = (claims) =>
  [{ alg: 'HS256' }, claims]
    .map((p) => Buffer.from(JSON.stringify(p)).toString('base64url'))
    .join('.') + '.sig';

async function boot() {
  const mail = [];
  // The fake CoinPay answers with whoever `who` is, and the nonce the test hands it.
  const cp = { who: null, nonce: null, exchanges: [] };
  const coinpayFetch = async (url, init) => {
    if (String(url).endsWith('/api/oauth/token')) {
      const form = new URLSearchParams(init.body);
      cp.exchanges.push(Object.fromEntries(form));
      return Response.json({
        access_token: 'at',
        expires_in: 3600,
        id_token: jwt({
          iss: CP,
          aud: 'cp_w3bs',
          sub: cp.who.sub,
          exp: Math.floor(Date.now() / 1000) + 600,
          nonce: cp.nonce,
        }),
      });
    }
    return Response.json({
      sub: cp.who.sub,
      email: cp.who.email,
      email_verified: false,
      name: cp.who.name,
    });
  };
  const instance = await startServer({
    store: await openStore({ path: ':memory:' }),
    port: 0,
    publicOrigin: ORIGIN,
    mailer: { send: async (m) => (mail.push(m), { sent: true }) },
    formGuard: { minAgeMs: 0, rateLimit: false },
    coinpay: { clientId: 'cp_w3bs', clientSecret: 'shh', base: CP, fetch: coinpayFetch },
  });
  const base = `http://localhost:${instance.server.address().port}`;
  const browser = () => {
    const jar = new Map();
    const http = async (path, { body, headers = {}, ...rest } = {}) => {
      const response = await fetch(`${base}${path}`, {
        redirect: 'manual',
        ...rest,
        method: rest.method ?? (body !== undefined ? 'POST' : 'GET'),
        headers: {
          ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
          ...(jar.size ? { Cookie: [...jar].map(([k, v]) => `${k}=${v}`).join('; ') } : {}),
          ...headers,
        },
        body: body !== undefined ? JSON.stringify(body) : undefined,
      });
      for (const cookie of response.headers.getSetCookie()) {
        const [pair] = cookie.split(';');
        const [name, value] = pair.split('=');
        if (value) jar.set(name, value);
        else jar.delete(name);
      }
      return response;
    };
    return { http, jar };
  };
  // Runs a whole CoinPay round trip in browser `b`, starting from `startPath` (GET) or a POST.
  const viaCoinpay = async (b, who, { start, state } = {}) => {
    const res = start ? await start() : await b.http('/api/v1/coinpay/oauth/start?next=/fleet');
    const url = new URL(res.headers.get('location') ?? (await res.json()).url);
    cp.who = who;
    cp.nonce = url.searchParams.get('nonce');
    return b.http(
      `/api/v1/coinpay/oauth/callback?code=c1&state=${state ?? url.searchParams.get('state')}`,
    );
  };
  async function signUp(b, username, email) {
    const page = await (await b.http('/signup')).text();
    const form = page.slice(page.indexOf('data-form="signup"'));
    const fields = Object.fromEntries(
      [...form.matchAll(/<input type="hidden" name="([^"]+)" value="([^"]*)"/g)].map((m) => [
        m[1],
        m[2],
      ]),
    );
    await b.http('/api/auth/signup', {
      body: { username, email, password: 'correct horse battery', ...fields },
    });
    const token = new URL(
      /https?:\/\/\S+\/auth\/link\?token=\S+/.exec(mail.at(-1).text)[0],
    ).searchParams.get('token');
    return (await (await b.http('/api/auth/link', { body: { token } })).json()).user;
  }
  return { instance, browser, viaCoinpay, signUp, cp, mail, close: () => instance.close() };
}

test('login page offers CoinPay first; start redirects with S256, state and nonce', async () => {
  const t = await boot();
  try {
    const b = t.browser();
    const html = await (await b.http('/login?next=/oauth/authorize%3Fclient_id%3Dw3bs-cli')).text();
    const coinpayAt = html.indexOf('Sign in with CoinPay');
    assert.ok(coinpayAt > 0 && coinpayAt < html.indexOf('Sign in with a passkey'));
    assert.match(
      html,
      /\/api\/v1\/coinpay\/oauth\/start\?next=%2Foauth%2Fauthorize%3Fclient_id%3Dw3bs-cli/,
    );
    const res = await b.http('/api/v1/coinpay/oauth/start?next=/fleet');
    assert.equal(res.status, 302);
    const url = new URL(res.headers.get('location'));
    assert.equal(url.origin + url.pathname, `${CP}/api/oauth/authorize`);
    assert.equal(url.searchParams.get('code_challenge_method'), 'S256');
    assert.equal(url.searchParams.get('redirect_uri'), `${ORIGIN}/api/v1/coinpay/oauth/callback`);
    assert.equal(url.searchParams.get('scope'), 'openid profile email');
    assert.ok(url.searchParams.get('state') && url.searchParams.get('nonce'));
  } finally {
    await t.close();
  }
});

test('a new CoinPay login creates an account with no verified email, then signs back into it', async () => {
  const t = await boot();
  try {
    const b = t.browser();
    const res = await t.viaCoinpay(b, {
      sub: 'cp_1',
      email: 'grace@example.com',
      name: 'Grace Hopper',
    });
    assert.equal(res.status, 302);
    assert.equal(res.headers.get('location'), '/fleet');
    assert.equal(t.cp.exchanges[0].client_secret, 'shh');
    const me = await (await b.http('/api/me')).json();
    assert.equal(me.username, 'grace-hopper');
    assert.equal(me.email, null);
    assert.equal(me.email_verified, false);
    // Same sub, new browser: the same account.
    const again = t.browser();
    await t.viaCoinpay(again, { sub: 'cp_1', email: 'changed@example.com', name: 'G' });
    assert.equal((await (await again.http('/api/me')).json()).sub, me.sub);
    // The CoinPay address is not a sign-in address.
    assert.equal(t.mail.length, 0);
    // Adding an email needs the emailed link, and only then is it verified.
    const page = await (await b.http('/account')).text();
    assert.match(page, /No confirmed email/);
    const form = page.slice(page.indexOf('data-form="add-email"'));
    const fields = Object.fromEntries(
      [...form.matchAll(/<input type="hidden" name="([^"]+)" value="([^"]*)"/g)].map((m) => [
        m[1],
        m[2],
      ]),
    );
    await b.http('/api/account/email', { body: { email: 'grace@example.com', ...fields } });
    assert.equal(t.mail.at(-1).to, 'grace@example.com');
    const token = new URL(
      /https?:\/\/\S+\/auth\/link\?token=\S+/.exec(t.mail.at(-1).text)[0],
    ).searchParams.get('token');
    assert.equal((await b.http('/api/auth/link', { body: { token } })).status, 200);
    const after = await (await b.http('/api/me')).json();
    assert.equal(after.email, 'grace@example.com');
    assert.equal(after.email_verified, true);
  } finally {
    await t.close();
  }
});

test('a CoinPay login whose email matches an existing account does NOT get that account', async () => {
  const t = await boot();
  try {
    const ada = await t.signUp(t.browser(), 'ada', 'ada@example.com');
    const eve = t.browser();
    await t.viaCoinpay(eve, { sub: 'cp_eve', email: 'ada@example.com', name: 'ada' });
    const me = await (await eve.http('/api/me')).json();
    assert.notEqual(me.sub, ada.id);
    assert.notEqual(me.username, 'ada');
    assert.equal(me.email, null);
  } finally {
    await t.close();
  }
});

test('callback rejects a bad state, a missing cookie and a wrong nonce', async () => {
  const t = await boot();
  try {
    const b = t.browser();
    const bad = await t.viaCoinpay(
      b,
      { sub: 'cp_x', email: null, name: null },
      { state: 'forged' },
    );
    assert.equal(bad.headers.get('location'), '/login?coinpay=state');
    assert.equal((await b.http('/api/me')).status, 401);

    // A state started in another browser (no cookie here) is refused.
    const other = t.browser();
    const started = new URL(
      (await other.http('/api/v1/coinpay/oauth/start')).headers.get('location'),
    );
    const stranger = t.browser();
    const res = await stranger.http(
      `/api/v1/coinpay/oauth/callback?code=c&state=${started.searchParams.get('state')}`,
    );
    assert.equal(res.headers.get('location'), '/login?coinpay=state');

    // Wrong nonce in the id_token.
    const n = t.browser();
    const go = new URL((await n.http('/api/v1/coinpay/oauth/start')).headers.get('location'));
    t.cp.who = { sub: 'cp_n', email: null, name: null };
    t.cp.nonce = 'not-the-nonce';
    const back = await n.http(
      `/api/v1/coinpay/oauth/callback?code=c&state=${go.searchParams.get('state')}`,
    );
    assert.equal(back.headers.get('location'), '/login?coinpay=failed');
    assert.equal((await n.http('/api/me')).status, 401);
    // The pending request is single use.
    const replay = await n.http(
      `/api/v1/coinpay/oauth/callback?code=c&state=${go.searchParams.get('state')}`,
    );
    assert.equal(replay.headers.get('location'), '/login?coinpay=state');
  } finally {
    await t.close();
  }
});

test('Connect CoinPay needs a session, binds to that account, and disconnect guards the last way in', async () => {
  const t = await boot();
  try {
    const anon = t.browser();
    assert.equal((await anon.http('/api/account/coinpay', { body: {} })).status, 401);

    const b = t.browser();
    const ada = await t.signUp(b, 'ada', 'ada@example.com');
    const linked = await t.viaCoinpay(
      b,
      { sub: 'cp_ada', email: 'x@example.com', name: 'A' },
      {
        start: () => b.http('/api/account/coinpay', { body: {} }),
      },
    );
    assert.equal(linked.headers.get('location'), '/account?coinpay=linked');
    // Now CoinPay signs into ada.
    const later = t.browser();
    await t.viaCoinpay(later, { sub: 'cp_ada', email: null, name: null });
    assert.equal((await (await later.http('/api/me')).json()).sub, ada.id);
    // Someone else cannot link the same CoinPay account.
    const bob = t.browser();
    await t.signUp(bob, 'bob', 'bob@example.com');
    const stolen = await t.viaCoinpay(
      bob,
      { sub: 'cp_ada', email: null, name: null },
      {
        start: () => bob.http('/api/account/coinpay', { body: {} }),
      },
    );
    assert.equal(stolen.headers.get('location'), '/account?coinpay=sub_taken');
    // ada has a confirmed email, so she may disconnect.
    assert.equal((await b.http('/api/account/coinpay', { method: 'DELETE' })).status, 200);

    // A CoinPay-only account cannot disconnect its only way in.
    const c = t.browser();
    await t.viaCoinpay(c, { sub: 'cp_only', email: null, name: null });
    assert.equal((await c.http('/api/account/coinpay', { method: 'DELETE' })).status, 409);
  } finally {
    await t.close();
  }
});
