import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { validate } from '@logicsrc/validators';
import { startServer } from '../src/server.mjs';
import { openStore } from '../src/store.mjs';
import { toOpenTask } from '../training/core/opentask.mjs';

// https, so published tasks carry their moshscript runner (OpenTask runners are https only).
const ORIGIN = 'https://w3bs.test';
const sha256 = (text) => createHash('sha256').update(text).digest('hex');

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
    return { username, cookie: `__Host-w3bs_session=${await accounts.createSession(created.id)}` };
  };
  const http = (path, { body, cookie, method } = {}) =>
    fetch(`${base}${path}`, {
      redirect: 'manual',
      method: method ?? (body !== undefined ? 'POST' : 'GET'),
      headers: {
        Host: new URL(ORIGIN).host,
        ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
        ...(cookie ? { Cookie: cookie } : {}),
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
  const json = async (path, options) => {
    const response = await http(path, options);
    return { status: response.status, body: await response.json() };
  };
  return { http, json, user, close: () => instance.close() };
}

const episode = (n, over = {}) =>
  toOpenTask({
    episodeId: `ep_00000000000000${String(n).padStart(2, '0')}`,
    goal: `Bump the version ${n}`,
    explanation: 'Edit package.json, then commit.',
    steps: [
      {
        n: 1,
        type: 'focus',
        target: { app: 'hqterm', window: 'me@box: ~/src/pkg' },
        evidence: 'observed',
      },
      {
        n: 2,
        type: 'text_edit',
        target: { app: 'hqterm' },
        text: '{{version}}',
        evidence: 'observed',
      },
      {
        n: 3,
        type: 'key_shortcut',
        target: { app: 'hqterm' },
        keys: ['enter'],
        evidence: 'observed',
      },
    ],
    parameters: [{ name: 'version', type: 'string' }],
    profile: 'semantic-demonstration',
    outcome: { value: 'success', evidence_type: 'state_change' },
    stopConditions: ['a target cannot be found on the current screen'],
    humanApprovals: ['before pushing anything'],
    version: '0.2.6',
    ...over,
  });

for (const backend of backends) {
  test(`tasks and workflows publish, serve and unpublish (${backend.name})`, async () => {
    const t = await boot(backend);
    try {
      const anon = await t.json('/api/v1/tasks', { body: { task: episode(1) } });
      assert.equal(anon.status, 401);

      const anthony = await t.user('anthony');
      const made = await t.json('/api/v1/tasks', {
        body: { task: episode(1) },
        cookie: anthony.cookie,
      });
      assert.equal(made.status, 201, JSON.stringify(made.body));
      assert.match(made.body.id, /^bump-the-version-1-[a-z0-9]{6}$/);
      assert.equal(made.body.url, `${ORIGIN}/tasks/${made.body.id}`);

      // Republishing the same episode keeps the id.
      const again = await t.json('/api/v1/tasks', {
        body: { task: episode(1) },
        cookie: anthony.cookie,
      });
      assert.equal(again.body.id, made.body.id);
      assert.equal(again.body.republished, true);

      // The served OpenTask validates, carries no window title and points at its own .mosh.
      const raw = await (await t.http(`/tasks/${made.body.id}.json`)).text();
      const doc = JSON.parse(raw);
      assert.deepEqual(validate('opentask', doc), { ok: true, kind: 'opentask', data: doc });
      assert.equal(doc.id, made.body.id);
      assert.ok(!raw.includes('me@box'));
      const mosh = await (await t.http(`/tasks/${made.body.id}.mosh`)).text();
      assert.match(mosh, /^#!\/usr\/bin\/env moshscript\n/);
      assert.equal(doc.runners[0].sha256, sha256(mosh));
      assert.equal(doc.runners[0].url, `${ORIGIN}/tasks/${made.body.id}.mosh`);

      // Only reviewed, successful demonstrations, and never coordinates.
      const bad = episode(2);
      bad.steps[0].x = 10;
      assert.equal(
        (await t.json('/api/v1/tasks', { body: { task: bad }, cookie: anthony.cookie })).status,
        400,
      );
      const unreviewed = episode(2);
      unreviewed.source.reviewed = false;
      assert.equal(
        (await t.json('/api/v1/tasks', { body: { task: unreviewed }, cookie: anthony.cookie }))
          .status,
        400,
      );

      // --workflow creates the workflow, then appends to it by title.
      const second = await t.json('/api/v1/tasks', {
        body: { task: episode(2), workflow: 'Release a package' },
        cookie: anthony.cookie,
      });
      assert.equal(second.status, 201);
      const wfId = second.body.workflow.id;
      const third = await t.json('/api/v1/tasks', {
        body: { task: episode(3), workflow: 'release a PACKAGE' },
        cookie: anthony.cookie,
      });
      assert.equal(third.body.workflow.id, wfId);

      const wf = (await t.json(`/workflows/${wfId}.json`)).body;
      assert.equal(validate('openworkflow', wf).ok, true);
      assert.deepEqual(
        wf.tasks.map((x) => x.id),
        [second.body.id, third.body.id],
      );
      // Every digest matches the bytes served.
      for (const entry of wf.tasks)
        assert.equal(
          entry.sha256,
          sha256(await (await t.http(new URL(entry.url).pathname)).text()),
        );

      const index = (await t.json('/.well-known/openworkflow.json')).body;
      assert.equal(validate('openworkflow-index', index).ok, true);
      assert.deepEqual(
        index.workflows.map((w) => [w.id, w.tasks]),
        [[wfId, 2]],
      );

      // Workflows from the API: someone else may collect anyone's published tasks.
      const other = await t.user('preshy');
      const own = await t.json('/api/v1/workflows', {
        body: { title: 'Ship it', order: 'any', tasks: [made.body.id, second.body.id] },
        cookie: other.cookie,
      });
      assert.equal(own.status, 201);
      assert.equal(
        (
          await t.json(`/api/v1/workflows/${own.body.id}`, {
            method: 'DELETE',
            cookie: anthony.cookie,
          })
        ).status,
        404,
        'only the owner deletes a workflow',
      );
      assert.equal(
        (
          await t.json('/api/v1/workflows', {
            body: { title: 'x', tasks: ['nope-123456'] },
            cookie: other.cookie,
          })
        ).status,
        404,
      );

      // Pages render.
      const list = await (await t.http('/tasks')).text();
      assert.ok(list.includes('Bump the version 1'));
      assert.ok(
        (await (await t.http(`/tasks/${made.body.id}`)).text()).includes(
          `moshcode run ${made.body.id}.mosh`,
        ),
      );
      assert.ok((await (await t.http('/workflows')).text()).includes('Release a package'));
      assert.ok((await (await t.http(`/workflows/${wfId}`)).text()).includes('Bump the version 3'));
      assert.equal((await t.http('/tasks/nope-000000')).status, 404);

      // Unpublishing removes the task from every workflow and drops a workflow left empty.
      assert.equal(
        (
          await t.json(`/api/v1/tasks/${second.body.id}`, {
            method: 'DELETE',
            cookie: other.cookie,
          })
        ).status,
        404,
        'only the publisher unpublishes',
      );
      await t.json(`/api/v1/tasks/${second.body.id}`, { method: 'DELETE', cookie: anthony.cookie });
      const gone = await t.json(`/api/v1/tasks/${third.body.id}`, {
        method: 'DELETE',
        cookie: anthony.cookie,
      });
      assert.deepEqual(gone.body.workflows_removed, [wfId]);
      assert.equal((await t.http(`/workflows/${wfId}.json`)).status, 404);
      const shipIt = (await t.json(`/workflows/${own.body.id}.json`)).body;
      assert.deepEqual(
        shipIt.tasks.map((x) => x.id),
        [made.body.id],
      );
    } finally {
      await t.close();
    }
  });
}
