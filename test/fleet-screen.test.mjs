import test from 'node:test';
import assert from 'node:assert/strict';
import { startServer } from '../src/server.mjs';
import { openStore } from '../src/store.mjs';

const ORIGIN = 'http://localhost:3000';
const INFO = { hostname: 'kde-box', os: 'Fedora Linux 44', w3bs_version: '0.1.6' };
const SHARED = { ...INFO, screen: { mode: 'view', vnc: '127.0.0.1:5900', reachable: true } };

async function boot() {
  const store = await openStore({ path: ':memory:' });
  const instance = await startServer({
    store,
    port: 0,
    publicOrigin: ORIGIN,
    mailer: { send: async () => ({ sent: true }) },
  });
  const port = instance.server.address().port;
  const base = `http://localhost:${port}`;
  const accounts = store.accounts(ORIGIN);
  const user = async (username) => {
    const signup = await accounts.startSignup({ username, email: `${username}@example.com` });
    const { user: created } = await accounts.consumeLink(signup.link);
    return `w3bs_session=${await accounts.createSession(created.id)}`;
  };
  const http = (path, { body, cookie, bearer, method } = {}) =>
    fetch(`${base}${path}`, {
      method: method ?? (body !== undefined ? 'POST' : 'GET'),
      headers: {
        ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
        ...(cookie ? { Cookie: cookie } : {}),
        ...(bearer ? { Authorization: `Bearer ${bearer}` } : {}),
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
  // Bun's WebSocket takes request headers, so a test can be the agent or the browser.
  const socket = (path, headers = {}) => {
    const ws = new WebSocket(`ws://localhost:${port}${path}`, { headers });
    ws.binaryType = 'arraybuffer';
    const messages = [];
    const waiters = [];
    ws.addEventListener('message', (event) => {
      messages.push(event.data);
      waiters.splice(0).forEach((resolve) => resolve());
    });
    const closed = new Promise((resolve) =>
      ws.addEventListener('close', (event) => resolve({ code: event.code, reason: event.reason })),
    );
    const opened = new Promise((resolve) => {
      ws.addEventListener('open', () => resolve(true));
      ws.addEventListener('error', () => resolve(false));
    });
    const next = async () => {
      while (!messages.length) await new Promise((resolve) => waiters.push(resolve));
      return messages.shift();
    };
    return { ws, opened, closed, next };
  };
  const machine = async (cookie, info = SHARED) => {
    const { token } = await (await http('/api/fleet/tokens', { cookie, body: {} })).json();
    const joined = await (await http('/api/fleet/join', { body: { token, info } })).json();
    const [listed] = (await (await http('/api/fleet/machines', { cookie })).json()).filter(
      (m) => m.id === joined.machine_id,
    );
    return { ...joined, id: listed.id };
  };
  return { http, user, socket, machine, store, close: () => instance.close() };
}
const text = (data) => new TextDecoder().decode(data);
const until = async (check) => {
  for (let i = 0; i < 100 && !(await check()); i++) await new Promise((r) => setTimeout(r, 20));
};

test('fleet screen: owner watches through the relay; RFB passes both ways; session is logged', async () => {
  const t = await boot();
  try {
    const alice = await t.user('alice');
    const m = await t.machine(alice);
    const agentHeaders = { Authorization: `Bearer ${m.agent_key}` };
    const browser = { Cookie: alice, Origin: ORIGIN };

    // The page and the card link.
    const page = await t.http(`/fleet/${m.id}/screen`, { cookie: alice });
    assert.equal(page.status, 200);
    assert.match(await page.text(), /\/assets\/fleet-screen\.js/);
    let [listed] = await (await t.http('/api/fleet/machines', { cookie: alice })).json();
    assert.deepEqual(listed.info.screen, SHARED.screen);
    assert.equal(listed.screen_listening, false);

    // Without the agent's standing connection, the viewer is told why.
    const early = t.socket(`/api/fleet/machines/${m.id}/screen`, browser);
    assert.equal((await early.closed).code, 4404);

    const listen = t.socket('/api/fleet/listen', agentHeaders);
    assert.equal(await listen.opened, true);
    await until(
      async () =>
        (await (await t.http('/api/fleet/machines', { cookie: alice })).json())[0].screen_listening,
    );
    [listed] = await (await t.http('/api/fleet/machines', { cookie: alice })).json();
    assert.equal(listed.screen_listening, true);

    const viewer = t.socket(`/api/fleet/machines/${m.id}/screen`, browser);
    assert.equal(await viewer.opened, true);
    const signal = JSON.parse(await listen.next());
    assert.equal(signal.type, 'screen');
    assert.match(signal.session, /^s_/);
    // Bytes the viewer sends before the machine answers are kept for it.
    viewer.ws.send(new TextEncoder().encode('early'));
    const agent = t.socket(`/api/fleet/screen/${signal.session}`, agentHeaders);
    assert.equal(await agent.opened, true);
    assert.equal(text(await agent.next()), 'early');
    agent.ws.send(new TextEncoder().encode('RFB 003.008\n'));
    assert.equal(text(await viewer.next()), 'RFB 003.008\n');
    viewer.ws.send(new TextEncoder().encode('RFB 003.008\n'));
    assert.equal(text(await agent.next()), 'RFB 003.008\n');
    // A session id is good for one agent connection.
    const again = t.socket(`/api/fleet/screen/${signal.session}`, agentHeaders);
    assert.equal(await again.opened, false);

    viewer.ws.close();
    assert.equal((await agent.closed).code, 1000, 'closing the page ends the machine side');
    await until(
      async () =>
        (await (await t.http('/api/fleet/machines', { cookie: alice })).json())[0].actions.length,
    );
    [listed] = await (await t.http('/api/fleet/machines', { cookie: alice })).json();
    assert.equal(listed.actions[0].action, 'screen');
    assert.equal(listed.actions[0].status, 'done');
    assert.match(listed.actions[0].output, /^Watched for \d+s \(view, from /);

    // The agent can refuse with a reason, which the viewer sees as the close reason.
    const second = t.socket(`/api/fleet/machines/${m.id}/screen`, browser);
    const { session } = JSON.parse(await listen.next());
    const failing = t.socket(`/api/fleet/screen/${session}`, agentHeaders);
    await failing.opened;
    failing.ws.send(JSON.stringify({ error: 'nothing listens on 127.0.0.1:5900' }));
    assert.deepEqual(await second.closed, {
      code: 4502,
      reason: 'nothing listens on 127.0.0.1:5900',
    });

    // Removing the machine closes its standing connection.
    await t.http(`/api/fleet/machines/${m.id}`, { cookie: alice, method: 'DELETE' });
    assert.equal((await listen.closed).code, 1000);
  } finally {
    await t.close();
  }
});

test('fleet screen: strangers, other sites, other machines and unshared screens are refused', async () => {
  const t = await boot();
  try {
    const alice = await t.user('alice');
    const bob = await t.user('bob');
    const shared = await t.machine(alice);
    const unshared = await t.machine(alice, INFO);
    const bobs = await t.machine(bob);
    const listen = t.socket('/api/fleet/listen', { Authorization: `Bearer ${shared.agent_key}` });
    assert.equal(await listen.opened, true);

    const path = `/api/fleet/machines/${shared.id}/screen`;
    // Cookies ride along on cross-site WebSockets, so the Origin must be ours.
    assert.equal(
      await t.socket(path, { Cookie: alice, Origin: 'https://evil.example' }).opened,
      false,
    );
    assert.equal(await t.socket(path, { Cookie: alice }).opened, false);
    assert.equal(await t.socket(path, { Origin: ORIGIN }).opened, false);
    assert.equal(await t.socket(path, { Cookie: bob, Origin: ORIGIN }).opened, false);
    // An agent key is not a viewer.
    assert.equal(
      await t.socket(path, { Authorization: `Bearer ${shared.agent_key}`, Origin: ORIGIN }).opened,
      false,
    );
    // Not shared: accepted, then closed with the reason.
    const notShared = t.socket(`/api/fleet/machines/${unshared.id}/screen`, {
      Cookie: alice,
      Origin: ORIGIN,
    });
    assert.equal((await notShared.closed).code, 4403);
    // Bob's agent cannot pick up Alice's session.
    const viewer = t.socket(path, { Cookie: alice, Origin: ORIGIN });
    const { session } = JSON.parse(await listen.next());
    assert.equal(
      await t.socket(`/api/fleet/screen/${session}`, { Authorization: `Bearer ${bobs.agent_key}` })
        .opened,
      false,
    );
    assert.equal(
      await t.socket('/api/fleet/listen', { Authorization: 'Bearer fm_nope' }).opened,
      false,
    );
    // Unknown upgrade paths are dropped.
    assert.equal(
      await t.socket('/api/fleet/other', { Cookie: alice, Origin: ORIGIN }).opened,
      false,
    );
    viewer.ws.close();
  } finally {
    await t.close();
  }
});

// Production looks agent keys up in Postgres, a real network wait before the upgrade. Bun
// 1.4.0's built-in ws threw in handleUpgrade after such a wait (fixed in 1.4.2); SQLite never
// showed it, so slow the lookup down on purpose.
test('fleet screen: the upgrade survives a slow key lookup', async () => {
  const t = await boot();
  try {
    const alice = await t.user('alice');
    const m = await t.machine(alice);
    const fleet = t.store.fleet;
    const lookup = fleet.machineByKey;
    fleet.machineByKey = async (key) => {
      await new Promise((resolve) => setTimeout(resolve, 30));
      return lookup(key);
    };
    const listen = t.socket('/api/fleet/listen', { Authorization: `Bearer ${m.agent_key}` });
    assert.equal(await listen.opened, true);
    listen.ws.close();
  } finally {
    await t.close();
  }
});

test('fleet screen: the join command carries the screen mode', async () => {
  const t = await boot();
  try {
    const alice = await t.user('alice');
    const made = await (
      await t.http('/api/fleet/tokens', { cookie: alice, body: { screen: 'control' } })
    ).json();
    assert.match(made.command, /w3bs fleet join fj_\S+ --screen control/);
    const plain = await (
      await t.http('/api/fleet/tokens', { cookie: alice, body: { screen: 'rm -rf' } })
    ).json();
    assert.doesNotMatch(plain.command, /--screen/);
  } finally {
    await t.close();
  }
});
