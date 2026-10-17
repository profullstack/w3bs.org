import test from 'node:test';
import assert from 'node:assert/strict';
import { validate } from '@logicsrc/validators';
import { startServer } from '../src/server.mjs';
import { openStore } from '../src/store.mjs';
import { bucketVariant, twoProportionP, wilson } from '../src/abtest.mjs';
import { parseVariants } from '../public/ab-testing.js';

const ORIGIN = 'http://localhost:3000';

// SQLite always; PostgreSQL too when W3BS_TEST_DATABASE_URL names a disposable database
// (CI). Usernames are random so runs against a kept database do not collide.
const backends = [{ name: 'sqlite', options: { path: ':memory:' } }];
if (process.env.W3BS_TEST_DATABASE_URL)
  backends.push({ name: 'postgres', options: { databaseUrl: process.env.W3BS_TEST_DATABASE_URL } });

async function boot(backend) {
  const store = await openStore({ ...backend.options, seed: false });
  const instance = await startServer({
    store,
    port: 0,
    publicOrigin: ORIGIN,
    mailer: { send: async () => ({ sent: true }) },
  });
  const base = `http://localhost:${instance.server.address().port}`;
  const accounts = store.accounts(ORIGIN);
  const user = async (name) => {
    const username = `${name}${Math.random().toString(36).slice(2, 8)}`;
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
  const json = async (path, options) => {
    const response = await http(path, options);
    return { status: response.status, body: await response.json() };
  };
  return { http, json, user, close: () => instance.close() };
}

const EXPERIMENT = {
  name: 'Checkout button',
  slug: 'checkout-button',
  unit: 'user',
  variants: [
    { id: 'control', weight: 1, parameters: { color: 'blue' } },
    { id: 'green', weight: 1, parameters: { color: 'green' } },
  ],
  minimumEligiblePerVariant: 5,
  days: 14,
};

for (const backend of backends) {
  test(`[${backend.name}] ab: landing is public; signed-in page lists experiments; strangers see nothing`, async () => {
    const t = await boot(backend);
    try {
      const landing = await t.http('/ab-testing');
      assert.equal(landing.status, 200);
      const html = await landing.text();
      assert.match(html, /A\/B tests that/);
      assert.doesNotMatch(html, /noindex/);
      assert.equal((await t.http('/ab-testing/checkout-button')).status, 302);
      assert.equal((await t.json('/api/ab/experiments')).status, 401);

      const alice = await t.user('alice');
      const made = await t.json('/api/ab/experiments', { cookie: alice, body: EXPERIMENT });
      assert.equal(made.status, 200, JSON.stringify(made.body));
      assert.equal(made.body.state, 'draft');
      assert.equal(made.body.window, null, 'a draft has no fixed window');
      const page = await (await t.http('/ab-testing', { cookie: alice })).text();
      assert.match(page, /checkout-button/);
      assert.match(page, /\/assets\/ab-testing\.js/);
      const detail = await t.http('/ab-testing/checkout-button', { cookie: alice });
      assert.equal(detail.status, 200);
      assert.match(await detail.text(), /no data/);

      const bob = await t.user('bob');
      assert.equal((await t.json('/api/ab/experiments', { cookie: bob })).body.length, 0);
      assert.equal(
        (await t.json('/api/ab/experiments/checkout-button', { cookie: bob })).status,
        404,
      );
      assert.equal((await t.http('/ab-testing/checkout-button', { cookie: bob })).status, 404);
      assert.equal(
        (await t.json('/api/ab/experiments', { cookie: alice, body: EXPERIMENT })).status,
        409,
        'ids are unique per account',
      );
    } finally {
      await t.close();
    }
  });

  test(`[${backend.name}] ab: definitions are checked and frozen once started`, async () => {
    const t = await boot(backend);
    try {
      const alice = await t.user('alice');
      const bad = async (body) =>
        (await t.json('/api/ab/experiments', { cookie: alice, body: { ...EXPERIMENT, ...body } }))
          .status;
      assert.equal(await bad({ slug: 'Has Spaces' }), 400);
      assert.equal(await bad({ variants: [EXPERIMENT.variants[0]] }), 400);
      assert.equal(await bad({ variants: [EXPERIMENT.variants[0], EXPERIMENT.variants[0]] }), 400);
      assert.equal(
        await bad({ variants: [{ id: 'a', weight: 0 }, { id: 'b' }] }),
        400,
        'weights are positive',
      );
      assert.equal(
        await bad({ variants: [{ id: 'a', parameters: { x: { nested: 1 } } }, { id: 'b' }] }),
        400,
      );
      assert.equal(
        await bad({ startsAt: '2026-10-20T00:00:00Z', endsAt: '2026-10-19T00:00:00Z' }),
        400,
      );

      await t.json('/api/ab/experiments', { cookie: alice, body: EXPERIMENT });
      const edited = await t.json('/api/ab/experiments/checkout-button', {
        cookie: alice,
        method: 'PATCH',
        body: { name: 'Checkout CTA' },
      });
      assert.equal(edited.body.name, 'Checkout CTA', 'drafts can be edited');
      const started = await t.json('/api/ab/experiments/checkout-button/state', {
        cookie: alice,
        body: { state: 'running' },
      });
      assert.equal(started.body.state, 'running');
      assert.ok(started.body.window.startsAt, 'starting fixes the window');
      assert.equal(
        Date.parse(started.body.window.endsAt) - Date.parse(started.body.window.startsAt),
        14 * 86400000,
      );
      assert.equal(
        (
          await t.json('/api/ab/experiments/checkout-button', {
            cookie: alice,
            method: 'PATCH',
            body: { name: 'nope' },
          })
        ).status,
        409,
      );
      const move = async (state) =>
        (
          await t.json('/api/ab/experiments/checkout-button/state', {
            cookie: alice,
            body: { state },
          })
        ).status;
      assert.equal(await move('draft'), 409);
      assert.equal(await move('paused'), 200);
      assert.equal(await move('running'), 200);
      assert.equal(await move('closed'), 200);
      assert.equal(await move('running'), 409, 'closed is terminal');
    } finally {
      await t.close();
    }
  });

  test(`[${backend.name}] ab: sticky server assignment, exposure, conversion, results and OpenABTest export`, async () => {
    const t = await boot(backend);
    try {
      const alice = await t.user('alice');
      await t.json('/api/ab/experiments', { cookie: alice, body: EXPERIMENT });
      const { body: key } = await t.json('/api/ab/keys', { cookie: alice, body: { name: 'prod' } });
      assert.match(key.key, /^ab_/);
      const assign = (participant) =>
        t.json('/api/ab/assign', {
          bearer: key.key,
          body: { experiment: 'checkout-button', participant },
        });

      const early = await assign('user-1');
      assert.equal(early.status, 200);
      assert.equal(early.body.variant, null, 'a draft enrolls nobody');
      assert.match(early.body.reason, /draft/);

      await t.json('/api/ab/experiments/checkout-button/state', {
        cookie: alice,
        body: { state: 'running' },
      });
      const first = await assign('user-1');
      assert.equal(first.body.new, true);
      assert.ok(['control', 'green'].includes(first.body.variant));
      assert.deepEqual(
        first.body.parameters,
        EXPERIMENT.variants.find((v) => v.id === first.body.variant).parameters,
      );
      const again = await assign('user-1');
      assert.equal(again.body.new, false);
      assert.equal(again.body.variant, first.body.variant);
      assert.equal(again.body.assignmentId, first.body.assignmentId);

      assert.equal((await assign('someone@example.com')).status, 400, 'no emails as ids');
      assert.equal((await assign('')).status, 400);
      assert.equal(
        (
          await t.json('/api/ab/assign', {
            bearer: 'ab_wrong',
            body: { experiment: 'checkout-button', participant: 'x' },
          })
        ).status,
        401,
      );

      // 40 participants: both variants get some, and the split is stable.
      const variants = {};
      for (let i = 0; i < 40; i++) {
        const { body } = await assign(`p-${i}`);
        variants[`p-${i}`] = body.variant;
      }
      const counts = Object.values(variants).reduce((m, v) => ({ ...m, [v]: (m[v] ?? 0) + 1 }), {});
      assert.ok(counts.control > 5 && counts.green > 5, JSON.stringify(counts));

      const report = (path, body) =>
        t.json(`/api/ab/${path}`, {
          bearer: key.key,
          body: { experiment: 'checkout-button', ...body },
        });
      assert.equal(
        (await report('exposure', { participant: 'never-assigned' })).status,
        409,
        'exposure needs an assignment',
      );
      assert.equal(
        (
          await report('exposure', {
            participant: 'p-0',
            variant: variants['p-0'] === 'green' ? 'control' : 'green',
          })
        ).status,
        409,
        'the variant must match the stored one',
      );
      const exposed = await report('exposure', { participant: 'p-0', surface: 'checkout' });
      assert.equal(exposed.body.recorded, true);
      const retry = await report('exposure', { participant: 'p-0', surface: 'checkout' });
      assert.equal(retry.body.duplicate, true, 'identical retries are no-ops');
      for (const p of ['p-1', 'p-2', 'p-3']) await report('exposure', { participant: p });

      const converted = await report('conversion', {
        participant: 'p-0',
        valueMinor: 4900,
        currency: 'USD',
      });
      assert.equal(converted.body.recorded, true);
      assert.equal(
        (await report('conversion', { participant: 'p-0', valueMinor: 4900, currency: 'USD' })).body
          .duplicate,
        true,
      );
      assert.equal(
        (await report('conversion', { participant: 'p-0', valueMinor: 100, currency: 'USD' }))
          .status,
        409,
        'same conversion, different content',
      );
      // A purchase with an OpenABTest price, and one converting without an exposure.
      const price = {
        offerId: 'offer-1',
        acceptedAt: '2026-10-17T00:00:00Z',
        currency: 'USD',
        listUnitPriceMinor: 5000,
        chargedUnitPriceMinor: 4500,
        discountBps: 1000,
        quantityMilliUnits: 1000,
        rounding: 'half-up',
        totalMinor: 4500,
      };
      assert.equal(
        (await report('conversion', { participant: 'p-1', conversionId: 'order-1', price })).status,
        200,
      );
      assert.equal(
        (
          await report('conversion', {
            participant: 'p-1',
            conversionId: 'order-2',
            price: { ...price, totalMinor: 9999 },
          })
        ).status,
        400,
        'price arithmetic is checked by the reference validator',
      );
      await report('conversion', { participant: 'p-5' });

      const { body: r } = await t.json('/api/ab/experiments/checkout-button', { cookie: alice });
      assert.equal(r.totals.assigned, 41);
      assert.equal(r.totals.exposed, 4);
      assert.equal(r.totals.converted, 3);
      assert.equal(r.winnerPolicy, 'manual-review');
      const byId = Object.fromEntries(r.variants.map((v) => [v.id, v]));
      assert.equal(byId.control.assigned + byId.green.assigned, 41);
      const p5 = byId[variants['p-5']];
      assert.ok(p5.convertedWithoutExposure >= 1, 'a conversion without an exposure is shown');
      for (const v of r.variants) {
        assert.equal(v.conversionRate, v.converted / v.assigned);
        assert.ok(v.interval95[0] <= v.conversionRate && v.conversionRate <= v.interval95[1]);
      }
      assert.equal(r.floors.met, false, 'seven days have not passed');

      // Pausing stops new participants but honours existing assignments.
      await t.json('/api/ab/experiments/checkout-button/state', {
        cookie: alice,
        body: { state: 'paused' },
      });
      assert.equal((await assign('p-0')).body.variant, variants['p-0']);
      assert.equal((await assign('new-person')).body.variant, null);

      // The export is valid OpenABTest 0.1.
      const { body: manifest } = await t.json('/api/ab/experiments/checkout-button/manifest.json', {
        cookie: alice,
      });
      const checked = validate('openabtest-manifest', manifest);
      assert.ok(checked.ok, JSON.stringify(checked.errors));
      assert.equal(manifest.state, 'paused');
      const { body: exported } = await t.json('/api/ab/experiments/checkout-button/events.json', {
        cookie: alice,
      });
      assert.equal(exported.omitted.conversionsWithoutPrice, 2);
      const kinds = exported.events.reduce(
        (m, ev) => ({ ...m, [ev.kind]: (m[ev.kind] ?? 0) + 1 }),
        {},
      );
      assert.deepEqual(kinds, { eligibility: 41, assignment: 41, exposure: 4, conversion: 1 });
      for (const ev of exported.events) {
        const result = validate('openabtest-event', ev);
        assert.ok(result.ok, `${ev.kind}: ${JSON.stringify(result.errors)}`);
      }

      // Keys: owner can revoke; a revoked key stops working.
      assert.equal(
        (await t.json(`/api/ab/keys/${key.id}`, { cookie: alice, method: 'DELETE' })).status,
        200,
      );
      assert.equal((await assign('p-0')).status, 401);
      // The owner's own session works as a producer too (w3bs ab assign).
      const own = await t.json('/api/ab/assign', {
        cookie: alice,
        body: { experiment: 'checkout-button', participant: 'p-0' },
      });
      assert.equal(own.body.variant, variants['p-0']);

      assert.equal(
        (await t.json('/api/ab/experiments/checkout-button', { cookie: alice, method: 'DELETE' }))
          .status,
        200,
      );
      assert.equal((await t.json('/api/ab/experiments', { cookie: alice })).body.length, 0);
    } finally {
      await t.close();
    }
  });
}

test('ab: bucketing follows the weights; statistics helpers', () => {
  const variants = [
    { id: 'a', weight: 1 },
    { id: 'b', weight: 3 },
  ];
  const counts = { a: 0, b: 0 };
  for (let i = 0; i < 4000; i++) counts[bucketVariant('secret', 'x', 1, `p${i}`, variants).id]++;
  assert.ok(counts.b / 4000 > 0.7 && counts.b / 4000 < 0.8, JSON.stringify(counts));
  assert.equal(
    bucketVariant('secret', 'x', 1, 'p1', variants).id,
    bucketVariant('secret', 'x', 1, 'p1', variants).id,
  );
  assert.equal(wilson(0, 0), null);
  const [lo, hi] = wilson(50, 100);
  assert.ok(Math.abs(lo - 0.4038) < 0.001 && Math.abs(hi - 0.5962) < 0.001);
  assert.equal(twoProportionP(0, 0, 1, 1), null);
  assert.ok(Math.abs(twoProportionP(50, 100, 65, 100) - 0.0315) < 0.002);
});

test('ab: the page parses the variant lines', () => {
  assert.deepEqual(parseVariants('control 1 {"color":"blue"}\n\ngreen 2\nred'), [
    { id: 'control', weight: 1, parameters: { color: 'blue' } },
    { id: 'green', weight: 2, parameters: undefined },
    { id: 'red', weight: 1, parameters: undefined },
  ]);
  assert.throws(() => parseVariants('a 1 {bad json}'), /not valid JSON/);
});
