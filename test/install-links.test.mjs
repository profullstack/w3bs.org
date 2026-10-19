import test from 'node:test';
import assert from 'node:assert/strict';
import { startServer } from '../src/server.mjs';
import { openStore } from '../src/store.mjs';
import { SEND_LIMIT, deviceOf } from '../src/install-links.mjs';

const ORIGIN = 'http://localhost:3000';
const UA = {
  mac: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 15_6) AppleWebKit/605.1.15 Safari/605.1.15',
  android:
    'Mozilla/5.0 (Linux; Android 16; Pixel 9) AppleWebKit/537.36 Chrome/141.0 Mobile Safari/537.36',
  iphone:
    'Mozilla/5.0 (iPhone; CPU iPhone OS 19_0 like Mac OS X) AppleWebKit/605.1.15 Mobile/15E148',
  ipad: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Version/19.0 Mobile/15E148 Safari/604.1',
};

async function boot() {
  const store = await openStore({ path: ':memory:' });
  const mail = [];
  const instance = await startServer({
    store,
    port: 0,
    publicOrigin: ORIGIN,
    mailer: { send: async (m) => (mail.push(m), { sent: true }) },
  });
  const base = `http://localhost:${instance.server.address().port}`;
  const accounts = store.accounts(ORIGIN);
  const user = async (username) => {
    const signup = await accounts.startSignup({ username, email: `${username}@example.com` });
    const { user: created } = await accounts.consumeLink(signup.link);
    return `w3bs_session=${await accounts.createSession(created.id)}`;
  };
  const get = (path, { ua = UA.mac, cookie } = {}) =>
    fetch(`${base}${path}`, {
      headers: { 'User-Agent': ua, ...(cookie ? { Cookie: cookie } : {}) },
    });
  const send = (path, cookie) =>
    fetch(`${base}${path}`, {
      method: 'POST',
      redirect: 'manual',
      headers: {
        'Content-Type': 'application/x-www-form-urlencoded',
        ...(cookie ? { Cookie: cookie } : {}),
      },
      body: '',
    });
  return { get, send, user, mail, close: () => instance.close() };
}

test('deviceOf reads the platform, including iPadOS that claims to be a Mac', () => {
  assert.equal(deviceOf(UA.mac), 'mac');
  assert.equal(deviceOf(UA.android), 'android');
  assert.equal(deviceOf(UA.iphone), 'ios');
  assert.equal(deviceOf(UA.ipad), 'ios');
  assert.equal(deviceOf('curl/8.14'), 'other');
});

test('/i/<package> offers each device its route and never installs by itself', async () => {
  const t = await boot();
  try {
    const mac = await (await t.get('/i/pairux.com/pairux')).text();
    assert.match(mac, /w3bs install pairux\.com\/pairux/);
    assert.match(mac, /curl -fsSL https:\/\/w3bs\.org\/install\.sh \| sh/);
    assert.match(mac, /Registry listing/);
    assert.match(mac, /Sign in<\/a> to email this link/);
    const android = await (await t.get('/i/pairux.com/pairux', { ua: UA.android })).text();
    assert.match(android, /W3BS app for Android/);
    assert.match(android, /http:\/\/localhost:3000\/i\/pairux\.com\/pairux/);
    const iphone = await (await t.get('/i/pairux.com/pairux', { ua: UA.iphone })).text();
    assert.match(iphone, /only from the App Store/);
    assert.equal((await t.get('/i/nope.dev/nothing')).status, 404);
    const detail = await (await t.get('/packages/pairux.com/pairux')).text();
    assert.match(detail, /Install link/);
  } finally {
    await t.close();
  }
});

test('Email me this link: signed in only, to the account address only, rate limited', async () => {
  const t = await boot();
  try {
    const out = await t.send('/i/pairux.com/pairux/send');
    assert.equal(out.status, 303);
    assert.match(out.headers.get('location'), /^\/login\?next=%2Fi%2Fpairux\.com%2Fpairux$/);
    assert.equal(t.mail.length, 0);

    const alice = await t.user('alice');
    const ok = await t.send('/i/pairux.com/pairux/send', alice);
    assert.equal(ok.status, 200);
    assert.match(await ok.text(), /Sent to a•••@example\.com/);
    assert.equal(t.mail.length, 1);
    assert.equal(t.mail[0].to, 'alice@example.com');
    assert.match(t.mail[0].text, /http:\/\/localhost:3000\/i\/pairux\.com\/pairux/);

    for (let i = 1; i < SEND_LIMIT.perHour; i++)
      assert.equal((await t.send('/i/pairux.com/pairux/send', alice)).status, 200);
    const limited = await t.send('/i/pairux.com/pairux/send', alice);
    assert.equal(limited.status, 429);
    assert.equal(t.mail.length, SEND_LIMIT.perHour);
  } finally {
    await t.close();
  }
});
