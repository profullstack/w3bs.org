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

// An OpenDevice inventory (logicsrc.com/docs/opendevice) in the shape @profullstack/fleet reads.
const INVENTORY = {
  $schema: 'https://logicsrc.com/docs/opendevice',
  currency: 'USD',
  devices: [
    {
      id: 'dev2',
      name: 'dev2',
      profile: 'server',
      hostnames: ['dev2.profullstack.com'],
      provider: { name: 'DediRock' },
      cost: { amount: 149, interval: 'month' },
    },
    {
      id: 'eu1',
      name: 'eu1',
      profile: 'server',
      cost: { amount: 120, interval: 'year', currency: 'EUR' },
    },
    { id: 'laptop', name: 'laptop', profile: 'desktop', cost: { amount: 40, interval: 'month' } },
    { id: 'ops-phone', name: 'ops phone', profile: 'phone', cost: { interval: 'month' } },
    { id: 'tab', name: 'tab', profile: 'tablet' },
  ],
};

test('fleet: an OpenDevice inventory merges with joined machines and adds up the month', async () => {
  const t = await boot();
  try {
    const dana = await t.user('dana');
    assert.equal(
      (await t.http('/api/fleet/inventory', { cookie: dana })).status,
      404,
      'no inventory yet',
    );
    const empty = await (await t.http('/api/fleet', { cookie: dana })).json();
    assert.deepEqual(empty.devices, []);
    assert.equal(empty.summary.devices, 0);

    // Validated on write: credentials never belong in one, and it must have devices.
    const bad = await t.http('/api/fleet/inventory', {
      cookie: dana,
      method: 'PUT',
      body: { devices: [{ name: 'x', password: 'hunter2' }] },
    });
    assert.equal(bad.status, 400);
    const badBody = await bad.json();
    assert.equal(badBody.error.code, 'INVALID_INVENTORY');
    assert.match(badBody.error.details.errors[0], /credentials/);
    assert.equal(
      (await t.http('/api/fleet/inventory', { cookie: dana, method: 'PUT', body: { nope: 1 } }))
        .status,
      400,
    );
    // Owner only.
    assert.equal(
      (await t.http('/api/fleet/inventory', { method: 'PUT', body: INVENTORY })).status,
      401,
    );

    const saved = await (
      await t.http('/api/fleet/inventory', { cookie: dana, method: 'PUT', body: INVENTORY })
    ).json();
    assert.equal(saved.devices, 5);
    assert.ok(saved.warnings.length >= 2, 'unstated costs are warnings, not errors');
    assert.deepEqual(saved.summary.monthly, { EUR: 10, USD: 189 });
    // Read back whole, so `fleet list <url>` can use it.
    assert.deepEqual(
      await (await t.http('/api/fleet/inventory', { cookie: dana })).json(),
      INVENTORY,
    );

    // dev2 joins: it matches the inventory device by hostname/name and gets its cost.
    const { token } = await (
      await t.http('/api/fleet/tokens', { cookie: dana, body: { name: 'dev2' } })
    ).json();
    await t.http('/api/fleet/join', { body: { token, info: INFO } });
    const view = await (await t.http('/api/fleet', { cookie: dana })).json();
    assert.equal(view.machines.length, 1);
    assert.equal(view.machines[0].device.id, 'dev2');
    assert.equal(view.machines[0].device.profile, 'server');
    assert.equal(view.machines[0].device.provider, 'DediRock');
    assert.deepEqual(view.machines[0].device.monthly, {
      amount: 149,
      currency: 'USD',
      state: 'stated',
    });
    // Devices without an agent are rows of their own.
    assert.deepEqual(
      view.devices.map((d) => [d.id, d.profile]),
      [
        ['eu1', 'server'],
        ['laptop', 'desktop'],
        ['ops-phone', 'phone'],
        ['tab', 'tablet'],
      ],
    );
    assert.equal(view.devices.find((d) => d.id === 'ops-phone').monthly.state, 'unstated');
    assert.equal(view.devices.find((d) => d.id === 'eu1').monthly.amount, 10, 'year / 12');
    assert.deepEqual(view.summary.unstated, ['ops-phone', 'tab'], 'never guessed');
    assert.equal(view.summary.monthly_text, '€10.00 + $189.00');
    assert.equal(view.summary.devices, 5);
    assert.equal(view.summary.matched, 1);
    assert.equal(view.summary.agentless, 4);
    assert.deepEqual(view.summary.by_profile, { server: 2, desktop: 1, phone: 1, tablet: 1 });
    // A device can name its machine by id when name and hostname do not line up.
    const { token: t2 } = await (
      await t.http('/api/fleet/tokens', { cookie: dana, body: { name: 'box' } })
    ).json();
    const other = await (
      await t.http('/api/fleet/join', { body: { token: t2, info: { hostname: 'odd-host' } } })
    ).json();
    const pinned = structuredClone(INVENTORY);
    pinned.devices[1].agents = [{ name: 'w3bs', id: other.machine_id }];
    await t.http('/api/fleet/inventory', { cookie: dana, method: 'PUT', body: pinned });
    const pinnedView = await (await t.http('/api/fleet', { cookie: dana })).json();
    assert.equal(pinnedView.machines.find((m) => m.name === 'box').device.id, 'eu1');
    assert.equal(pinnedView.summary.agentless, 3);
    await t.http(`/api/fleet/machines/${other.machine_id}`, { cookie: dana, method: 'DELETE' });
    await t.http('/api/fleet/inventory', { cookie: dana, method: 'PUT', body: INVENTORY });
    // `w3bs fleet ls` keeps its array, now with the device on each machine.
    const [listed] = await (await t.http('/api/fleet/machines', { cookie: dana })).json();
    assert.equal(listed.device.id, 'dev2');
    // The page renders it.
    assert.match(await (await t.http('/fleet', { cookie: dana })).text(), /"agentless":4/);

    // Another account sees none of it.
    const erin = await t.user('erin');
    assert.equal((await t.http('/api/fleet/inventory', { cookie: erin })).status, 404);
    assert.deepEqual((await (await t.http('/api/fleet', { cookie: erin })).json()).devices, []);

    // Deleting it leaves the machines.
    await t.http('/api/fleet/inventory', { cookie: dana, method: 'DELETE' });
    const after = await (await t.http('/api/fleet', { cookie: dana })).json();
    assert.equal(after.machines.length, 1);
    assert.equal(after.machines[0].device, null);
    assert.deepEqual(after.devices, []);
  } finally {
    await t.close();
  }
});
