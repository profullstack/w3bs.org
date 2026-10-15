import test from 'node:test';
import assert from 'node:assert/strict';
import { openStore } from '../src/store.mjs';
import { CLI_CLIENT_ID, DEVICE_GRANT } from '../src/accounts.mjs';

// SQLite always; PostgreSQL too when W3BS_TEST_DATABASE_URL names a disposable database
// (CI). Names are random so runs against a kept database do not collide.
const backends = [{ name: 'sqlite', options: { path: ':memory:' } }];
if (process.env.W3BS_TEST_DATABASE_URL)
  backends.push({ name: 'postgres', options: { databaseUrl: process.env.W3BS_TEST_DATABASE_URL } });
const unique = (name) => `${name}-${Math.random().toString(36).slice(2, 10)}`;
// Counts in a shared database include earlier runs; compare against where this one started.
const counter = async (store) => {
  const start = Object.fromEntries(
    await Promise.all(
      ['signup_completed', 'cli_login_completed', 'fleet_first_report'].map(async (name) => [
        name,
        await store.events.count(name),
      ]),
    ),
  );
  return async (name) => (await store.events.count(name)) - start[name];
};
for (const backend of backends) {
  test(`[${backend.name}] events: sign-up, CLI login and the first fleet report are counted, once where it matters`, async () => {
    const store = await openStore({ ...backend.options, seed: false });
    const count = await counter(store);
    try {
      const accounts = store.accounts('http://localhost:3000');
      const ada = unique('ada');
      const signup = await accounts.startSignup({ username: ada, email: `${ada}@example.com` });
      const { user } = await accounts.consumeLink(signup.link);
      assert.equal(await count('signup_completed'), 1);

      // Signing in again by email is not a sign-up.
      const again = await accounts.startEmailLogin(`${ada}@example.com`);
      await accounts.consumeLink(again.link);
      assert.equal(await count('signup_completed'), 1);

      const device = await accounts.startDevice({ client_id: CLI_CLIENT_ID, scope: 'profile' });
      await accounts.decideDevice(user.id, device.userCode, true);
      const tokens = await accounts.exchange({
        grant_type: DEVICE_GRANT,
        client_id: CLI_CLIENT_ID,
        device_code: device.deviceCode,
      });
      assert.equal(await count('cli_login_completed'), 1);
      // A refresh keeps the same sign-in; it is not a new login.
      await accounts.exchange({
        grant_type: 'refresh_token',
        client_id: CLI_CLIENT_ID,
        refresh_token: tokens.refresh_token,
      });
      assert.equal(await count('cli_login_completed'), 1);

      // Joining alone is not activation; the agent's report is. Only the first one counts.
      const join = async (name) => {
        const { token } = await store.fleet.createToken(user.id, { name });
        return (await store.fleet.join({ token, info: { hostname: `${name}.example.com` } }))
          .agent_key;
      };
      const first = await join('one');
      assert.equal(await count('fleet_first_report'), 0);
      await store.fleet.heartbeat(first, { cpus: 2 });
      await store.fleet.heartbeat(first, { cpus: 2 });
      await store.fleet.heartbeat(await join('two'), { cpus: 4 });
      assert.equal(await count('fleet_first_report'), 1);
    } finally {
      await store.close();
    }
  });

  test(`[${backend.name}] events: the unique index keeps one activation per account even without the cache`, async () => {
    const store = await openStore({ ...backend.options, seed: false });
    const count = await counter(store);
    try {
      const accounts = store.accounts('http://localhost:3000');
      const bea = unique('bea');
      const { user } = await accounts.consumeLink(
        (await accounts.startSignup({ username: bea, email: `${bea}@example.com` })).link,
      );
      assert.equal(await store.events.record('fleet_first_report', user.id), true);
      assert.equal(await store.events.record('fleet_first_report', user.id), true);
      assert.equal(await count('fleet_first_report'), 1);
      await assert.rejects(() => store.events.record('made_up_event', user.id), /Unknown event/);
    } finally {
      await store.close();
    }
  });
}
