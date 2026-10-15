// w3bs.org/training/app and /api/v1/training end to end on the real server, with the registry in
// SQLite and Training workspaces in PostgreSQL. Runs when W3BS_TEST_DATABASE_URL is set.
import test from 'node:test';
import assert from 'node:assert/strict';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { startServer } from '../src/server.mjs';
import { openStore } from '../src/store.mjs';
import { schemaFor } from '../training/hosted.mjs';
import { readZip } from '../training/core/zip.mjs';
import { sampleBundle } from '../training/fixtures/sample.mjs';
import { DEVICE_GRANT } from '../src/accounts.mjs';
import { accessToken, pushBundle } from '../training/core/remote.mjs';
import { openTraining } from '../training/core/service.mjs';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const url = process.env.W3BS_TEST_DATABASE_URL;
const ORIGIN = 'http://localhost:3000';

test(
  'hosted Training app: sign-in, CSRF, review, agent token, MCP over HTTP, isolation',
  { skip: !url },
  async () => {
    const store = await openStore({ path: ':memory:' });
    const instance = await startServer({
      store,
      port: 0,
      publicOrigin: ORIGIN,
      mailer: { send: async () => ({ sent: true }) },
      training: { databaseUrl: url, masterKey: 'b'.repeat(64) },
    });
    const base = `http://localhost:${instance.server.address().port}`;
    const accounts = store.accounts(ORIGIN);
    const ids = [];
    const user = async (username) => {
      const signup = await accounts.startSignup({ username, email: `${username}@example.com` });
      const { user: created } = await accounts.consumeLink(signup.link);
      ids.push(created.id);
      return `w3bs_session=${await accounts.createSession(created.id)}`;
    };
    const call = async (
      path,
      { cookie, bearer, method, body, raw, headers = {}, csrf = true } = {},
    ) => {
      const r = await fetch(`${base}/api/v1/training${path}`, {
        redirect: 'manual',
        method: method ?? (body !== undefined || raw ? 'POST' : 'GET'),
        headers: {
          ...(body !== undefined ? { 'content-type': 'application/json' } : {}),
          ...(cookie ? { cookie } : {}),
          ...(bearer ? { authorization: `Bearer ${bearer}` } : {}),
          ...(csrf ? { 'x-w3bs-training': '1' } : {}),
          ...headers,
        },
        body: raw ?? (body !== undefined ? JSON.stringify(body) : undefined),
      });
      const type = r.headers.get('content-type') || '';
      return {
        status: r.status,
        headers: r.headers,
        json: type.includes('json') ? await r.json() : null,
        buffer: type.includes('zip') ? Buffer.from(await r.arrayBuffer()) : null,
      };
    };
    const stamp = Date.now().toString(36);
    try {
      // Signed out.
      assert.equal((await call('/status')).status, 401);
      const gate = await fetch(`${base}/training/app/`, { redirect: 'manual' });
      assert.equal(gate.status, 302);
      assert.match(gate.headers.get('location'), /^\/login\?next=/);
      const page = await fetch(`${base}/training`);
      assert.match(await page.text(), /Teach agents/);

      const alice = await user(`alice${stamp}`);
      const app = await fetch(`${base}/training/app/`, { headers: { cookie: alice } });
      assert.equal(app.status, 200);
      assert.match(app.headers.get('content-security-policy'), /img-src 'self' blob:/);
      assert.doesNotMatch(
        app.headers.get('content-security-policy'),
        /crawlproof/,
        'no analytics on the private app',
      );
      assert.match(
        await (await fetch(`${base}/training/app/assets/app.js`)).text(),
        /W3BS Training local app/,
      );
      const status = await call('/status', { cookie: alice });
      assert.equal(status.json.key_store, 'server-managed');

      // CSRF: a cookie write without the app header is refused.
      assert.equal(
        (await call('/datasets', { cookie: alice, body: { name: 'x' }, csrf: false })).status,
        403,
      );

      // Upload, review, version, approve, release, grant: all through the API as the person.
      const up = await call('/imports', {
        cookie: alice,
        raw: sampleBundle(),
        headers: { 'x-filename': 'a.zip', 'content-type': 'application/octet-stream' },
      });
      assert.equal(up.status, 200, JSON.stringify(up.json));
      const done = await call(`/imports/${up.json.import_id}/complete`, {
        cookie: alice,
        method: 'POST',
      });
      const ep = done.json.episodes[0];
      await call(`/episodes/${ep}/review`, {
        cookie: alice,
        body: { decision: 'complete', confirm_goal: true },
      });
      const ds = (await call('/datasets', { cookie: alice, body: { name: 'Release workflows' } }))
        .json.dataset_id;
      await call(`/datasets/${ds}/episodes`, { cookie: alice, body: { add: [ep] } });
      await call(`/datasets/${ds}/versions`, { cookie: alice, body: {} });
      const approved = await call(`/datasets/${ds}/versions/1/approve`, {
        cookie: alice,
        body: { purposes: ['private_retrieval', 'model_training'] },
      });
      assert.equal(approved.status, 200, JSON.stringify(approved.json));
      await call(`/datasets/${ds}/versions/1/release`, { cookie: alice, method: 'POST' });
      await call('/grants', {
        cookie: alice,
        body: { consumer: 'claude-code', dataset_id: ds, version: 1 },
      });

      // Download export: a zip in the response, nothing on the server.
      const exp = await call('/exports', {
        cookie: alice,
        body: { dataset_id: ds, version: 1, format: 'next-action', out: '/etc/evil' },
      });
      assert.equal(exp.status, 200);
      assert.match(exp.headers.get('content-disposition'), /attachment; filename=/);
      assert.ok(readZip(exp.buffer).get('examples.jsonl'));

      // Agent token -> MCP over Streamable HTTP.
      const tok = await call('/agent-tokens', { cookie: alice, body: { consumer: 'claude-code' } });
      assert.equal(tok.status, 200);
      assert.match(tok.json.token, /^trn_/);
      assert.match(tok.json.claude_code, /claude mcp add --transport http w3bs-training/);
      const client = new Client({ name: 't', version: '0' });
      await client.connect(
        new StreamableHTTPClientTransport(new URL(`${base}/api/v1/training/mcp`), {
          requestInit: { headers: { authorization: `Bearer ${tok.json.token}` } },
        }),
      );
      const r = await client.callTool({
        name: 'training_retrieve',
        arguments: { task: 'draft the release announcement' },
      });
      assert.equal(r.structuredContent.episodes.length, 1);
      await client.close();

      // The token is an agent: it cannot approve or make more tokens.
      assert.equal(
        (
          await call(`/datasets/${ds}/versions/1/approve`, {
            bearer: tok.json.token,
            body: { purposes: ['evaluation'] },
          })
        ).status,
        403,
      );
      assert.equal(
        (await call('/agent-tokens', { bearer: tok.json.token, body: { consumer: 'x' } })).status,
        403,
      );
      // A forged token for another account fails.
      const forged = tok.json.token.replace(/^trn_[^.]+\./, `trn_${Buffer.from('someone-else').toString('base64url')}.`);
      assert.equal((await call('/status', { bearer: forged })).status, 401);

      // Revoked token stops working.
      await call(`/agent-tokens/${tok.json.id}`, { cookie: alice, method: 'DELETE' });
      assert.equal((await call('/status', { bearer: tok.json.token })).status, 401);

      // `w3bs training push`: a w3bs login session (device flow), refreshed because it expired,
      // then a local bundle uploaded as the CLI agent; the person still reviews it on the site.
      const carolCookie = await user(`carol${stamp}`);
      const carolId = ids.at(-1);
      const dev = await accounts.startDevice({ client_id: 'w3bs-cli' });
      await accounts.decideDevice(carolId, dev.userCode, true);
      const tokens = await accounts.exchange({
        grant_type: DEVICE_GRANT,
        device_code: dev.deviceCode,
        client_id: 'w3bs-cli',
      });
      const cfg = mkdtempSync(join(tmpdir(), 'w3bs-auth-'));
      mkdirSync(join(cfg, 'w3bs'));
      writeFileSync(
        join(cfg, 'w3bs/auth.json'),
        JSON.stringify({
          [ORIGIN]: {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_at: 1,
            username: `carol${stamp}`,
          },
        }),
      );
      try {
        // The server advertises its public origin (localhost:3000); route that to this test port.
        const viaPort = (u, init) => fetch(String(u).replace(ORIGIN, base), init);
        const fresh = await accessToken(ORIGIN, {
          env: { XDG_CONFIG_HOME: cfg },
          fetchImpl: viaPort,
        });
        assert.notEqual(fresh, tokens.access_token, 'expired token was refreshed');
        assert.equal(
          JSON.parse(readFileSync(join(cfg, 'w3bs/auth.json'), 'utf8'))[ORIGIN].access_token,
          fresh,
        );
        const home = mkdtempSync(join(tmpdir(), 'w3bs-push-'));
        const local = await openTraining({ home, env: { W3BS_TRAINING_PASSPHRASE: 'p' } });
        const imp = await local.importFile(
          { data: sampleBundle({ variant: 'p' }), filename: 'p.zip' },
          { kind: 'human', id: 'carol' },
        );
        const { episodes: localEps } = await local.commitImport(imp.import_id, {
          kind: 'human',
          id: 'carol',
        });
        const bundle = await local.bundleEpisodes(localEps, { kind: 'human', id: 'carol' });
        local.close();
        rmSync(home, { recursive: true, force: true });
        const pushed = await pushBundle(base, bundle, { token: fresh, filename: 'push.zip' });
        assert.equal(pushed.status, 'committed');
        assert.match(pushed.review, /\/training\/app\/#\/inbox$/);
        const inbox = await call('/inbox', { cookie: carolCookie });
        assert.equal(inbox.json.total, 1);
        // The CLI bearer is an agent: it cannot approve.
        assert.equal(
          (
            await call(`/episodes/${pushed.episodes[0]}/review`, {
              bearer: fresh,
              body: { decision: 'complete' },
            })
          ).status,
          403,
        );
      } finally {
        rmSync(cfg, { recursive: true, force: true });
      }

      // Another account sees an empty workspace.
      const bob = await user(`bob${stamp}`);
      assert.equal((await call('/status', { cookie: bob })).json.pending_review, 0);
      assert.equal((await call('/episodes', { cookie: bob })).json.length, 0);
      assert.equal((await call(`/episodes/${ep}`, { cookie: bob })).status, 404);
    } finally {
      await instance.close();
      const { default: pg } = await import('pg');
      const c = new pg.Client({ connectionString: url });
      await c.connect();
      for (const id of ids) await c.query(`DROP SCHEMA IF EXISTS "${schemaFor(id)}" CASCADE`);
      await c.end();
    }
  },
);
