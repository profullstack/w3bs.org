import test from 'node:test';
import assert from 'node:assert/strict';
import { startServer } from '../src/server.mjs';
import { openStore } from '../src/store.mjs';

const ORIGIN = 'http://localhost:3000';

async function boot() {
  const store = await openStore({ path: ':memory:' });
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
  const http = (path, { body, cookie, bearer, method } = {}) =>
    fetch(`${base}${path}`, {
      redirect: 'manual',
      method: method ?? (body !== undefined ? 'POST' : 'GET'),
      headers: {
        ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
        ...(cookie ? { Cookie: cookie } : {}),
        ...(bearer ? { Authorization: `Bearer ${bearer}` } : {}),
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
  return { http, user, close: () => instance.close() };
}

const INFO = {
  hostname: 'dev2.profullstack.com',
  os: 'Ubuntu 24.04.3 LTS',
  kernel: '6.8.0-85-generic',
  arch: 'x86_64',
  cpus: 8,
  uptime_s: 123456,
  load: [0.5, 0.75, 1],
  mem: { total: 32e9, available: 12e9 },
  disk: { mount: '/', total: 400e9, available: 190e9 },
  w3bs_version: '0.1.5',
  desktop: { installed: false, active: false },
  user: 'anthony',
  secret: 'should not be stored',
};

test('fleet: join with a one-time token, report, list; strangers see nothing', async () => {
  const t = await boot();
  try {
    assert.equal((await t.http('/fleet')).status, 302);
    const alice = await t.user('alice');
    const page = await t.http('/fleet', { cookie: alice });
    assert.equal(page.status, 200);
    assert.match(await page.text(), /id="fleet-data"/);
    const made = await (
      await t.http('/api/fleet/tokens', { cookie: alice, body: { name: 'dev2' } })
    ).json();
    assert.match(made.token, /^fj_/);
    assert.match(made.command, /w3bs fleet join fj_/);
    const joined = await t.http('/api/fleet/join', { body: { token: made.token, info: INFO } });
    assert.equal(joined.status, 200);
    const machine = await joined.json();
    assert.match(machine.agent_key, /^fm_/);
    assert.equal(machine.name, 'dev2');
    assert.equal(machine.allow_actions, false);
    assert.equal(machine.interval, 30);
    assert.equal(
      (await t.http('/api/fleet/join', { body: { token: made.token, info: INFO } })).status,
      401,
      'tokens work once',
    );
    const beat = await (
      await t.http('/api/fleet/heartbeat', { bearer: machine.agent_key, body: { info: INFO } })
    ).json();
    assert.deepEqual(beat.actions, []);
    const [listed] = await (await t.http('/api/fleet/machines', { cookie: alice })).json();
    assert.equal(listed.name, 'dev2');
    assert.equal(listed.online, true);
    assert.equal(listed.info.hostname, 'dev2.profullstack.com');
    assert.equal(listed.info.secret, undefined, 'only known fields are stored');
    // Another account sees nothing and cannot act on it; an agent key is not an account.
    const bob = await t.user('bob');
    assert.deepEqual(await (await t.http('/api/fleet/machines', { cookie: bob })).json(), []);
    assert.equal(
      (await t.http(`/api/fleet/machines/${listed.id}`, { cookie: bob, method: 'DELETE' })).status,
      404,
    );
    assert.equal((await t.http('/api/fleet/machines', { bearer: machine.agent_key })).status, 401);
    assert.equal((await t.http('/api/fleet/machines')).status, 401);
    // Actions are refused on a machine that joined without them.
    const refused = await t.http(`/api/fleet/machines/${listed.id}/actions`, {
      cookie: alice,
      body: { action: 'update' },
    });
    assert.equal(refused.status, 409);
    // Leaving removes it, and its key stops working.
    assert.equal(
      (await t.http('/api/fleet/leave', { bearer: machine.agent_key, body: {} })).status,
      200,
    );
    assert.equal(
      (await t.http('/api/fleet/heartbeat', { bearer: machine.agent_key, body: { info: INFO } }))
        .status,
      401,
    );
    assert.deepEqual(await (await t.http('/api/fleet/machines', { cookie: alice })).json(), []);
  } finally {
    await t.close();
  }
});

test('fleet: allowlisted actions queue, run on the next report, and record their result', async () => {
  const t = await boot();
  try {
    const owner = await t.user('carol');
    const { token } = await (
      await t.http('/api/fleet/tokens', { cookie: owner, body: { allowActions: true } })
    ).json();
    const machine = await (await t.http('/api/fleet/join', { body: { token, info: INFO } })).json();
    assert.equal(machine.allow_actions, true);
    assert.equal(machine.name, 'dev2.profullstack.com', 'defaults to the hostname');
    const [listed] = await (await t.http('/api/fleet/machines', { cookie: owner })).json();
    assert.equal(
      (
        await t.http(`/api/fleet/machines/${listed.id}/actions`, {
          cookie: owner,
          body: { action: 'rm -rf /' },
        })
      ).status,
      400,
    );
    const queued = await (
      await t.http(`/api/fleet/machines/${listed.id}/actions`, {
        cookie: owner,
        body: { action: 'update' },
      })
    ).json();
    assert.equal(queued.status, 'queued');
    const beat = await (
      await t.http('/api/fleet/heartbeat', { bearer: machine.agent_key, body: { info: INFO } })
    ).json();
    assert.deepEqual(beat.actions, [{ id: queued.id, action: 'update' }]);
    // Handed out once: the next report does not repeat it.
    assert.deepEqual(
      (
        await (
          await t.http('/api/fleet/heartbeat', { bearer: machine.agent_key, body: { info: INFO } })
        ).json()
      ).actions,
      [],
    );
    let [state] = await (await t.http('/api/fleet/machines', { cookie: owner })).json();
    assert.equal(state.actions[0].status, 'running');
    await t.http(`/api/fleet/actions/${queued.id}/result`, {
      bearer: machine.agent_key,
      body: { status: 'done', output: 'w3bs 0.1.5 is current' },
    });
    [state] = await (await t.http('/api/fleet/machines', { cookie: owner })).json();
    assert.equal(state.actions[0].status, 'done');
    assert.equal(state.actions[0].output, 'w3bs 0.1.5 is current');
    // Renaming and removing from the owner side.
    await t.http(`/api/fleet/machines/${listed.id}`, {
      cookie: owner,
      method: 'PATCH',
      body: { name: 'web <b>1</b>' },
    });
    [state] = await (await t.http('/api/fleet/machines', { cookie: owner })).json();
    assert.equal(state.name, 'web b1b', 'names are plain text');
    assert.equal(
      (await t.http(`/api/fleet/machines/${listed.id}`, { cookie: owner, method: 'DELETE' }))
        .status,
      200,
    );
    assert.equal(
      (await t.http('/api/fleet/heartbeat', { bearer: machine.agent_key, body: { info: INFO } }))
        .status,
      401,
    );
  } finally {
    await t.close();
  }
});
