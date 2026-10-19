import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { renderToText } from '@profullstack/hqtui/testing';
import { openTraining } from '../training/core/service.mjs';
import { createLocalApp } from '../training/ui/server.mjs';
import { reviewView } from '../training/ui/terminal.mjs';
import { sampleBundle } from '../training/fixtures/sample.mjs';

const human = { kind: 'human', id: 'reviewer' };
const homes = [];
const home = () => {
  const dir = mkdtempSync(join(tmpdir(), 'w3bs-training-s-'));
  homes.push(dir);
  return dir;
};
test.after(() => homes.forEach((h) => rmSync(h, { recursive: true, force: true })));
const env = (dir) => ({
  ...process.env,
  W3BS_TRAINING_HOME: dir,
  W3BS_TRAINING_PASSPHRASE: 'pass phrase',
});
const cli = (dir, ...args) => {
  const result = spawnSync(process.execPath, ['training/bin/w3bs-training.mjs', ...args], {
    env: env(dir),
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  return {
    ...result,
    json: result.stdout.trim() ? JSON.parse(result.stdout) : null,
    error: result.stderr.trim() ? JSON.parse(result.stderr).error : null,
  };
};

async function releasedStore(dir) {
  const t = await openTraining({ home: dir, env: env(dir) });
  const imp = await t.importFile({ data: sampleBundle(), filename: 's.zip' }, human);
  const { episodes } = await t.commitImport(imp.import_id, human);
  await t.review(episodes[0], { decision: 'complete', confirm_goal: true }, human);
  const ds = await t.createDataset({ name: 'Release workflows' }, human);
  await t.addEpisodes(ds.dataset_id, episodes, human);
  await t.createVersion(ds.dataset_id, {}, human);
  await t.approve(ds.dataset_id, 1, ['private_retrieval'], human);
  await t.release(ds.dataset_id, 1, human);
  await t.grant({ consumer: 'claude-code', dataset_id: ds.dataset_id, version: 1 }, human);
  return { t, episode: episodes[0], dataset: ds.dataset_id };
}

test('bare --version prints the release, the way w3bs update probes curl-installed tools', () => {
  for (const flag of ['--version', '-V']) {
    const r = spawnSync(process.execPath, ['training/bin/w3bs-training.mjs', flag], { encoding: 'utf8' });
    assert.equal(r.status, 0, r.stderr);
    assert.match(r.stdout, /^w3bs-training \d+\.\d+\.\d+\n$/);
  }
});

test('CLI imports, reports status and refuses human-only commands without a terminal', async () => {
  const dir = home();
  const bundle = join(dir, 'demo.w3bs-training.zip');
  writeFileSync(bundle, sampleBundle());
  const help = spawnSync(process.execPath, ['training/bin/w3bs-training.mjs', '--help'], {
    encoding: 'utf8',
  });
  assert.match(help.stdout, /w3bs training import FILE/);
  const imported = cli(dir, 'import', bundle);
  assert.equal(imported.status, 0, imported.stderr);
  assert.equal(imported.json.status, 'committed');
  const status = cli(dir, 'status');
  assert.equal(status.json.pending_review, 1);
  assert.equal(status.json.cloud_budget_usd, 0);
  const id = imported.json.episodes[0];
  for (const args of [
    ['episodes', 'label', id, '--decision', 'complete'],
    ['episodes', 'delete', id],
    ['forget', '--last', '10m'],
    ['access', 'off'],
    ['review'],
  ]) {
    const result = cli(dir, ...args);
    assert.equal(result.status, 3, `${args.join(' ')} should be refused`);
    assert.equal(result.error.code, 'HUMAN_APPROVAL_REQUIRED');
  }
  assert.equal(cli(dir, 'status').json.pending_review, 1, 'nothing changed');
  const dry = cli(dir, 'datasets', 'create', '--name', 'x');
  const plan = cli(dir, 'datasets', 'delete', dry.json.dataset_id, '--dry-run');
  assert.equal(plan.json.dry_run, true);
  assert.equal(cli(dir, 'jobs', 'adapters').json.length, 0);
  const setup = cli(dir, 'setup', 'codex', '--consumer', 'codex');
  assert.match(setup.json.add, /mcp_servers\.w3bs-training/);
  const claude = cli(dir, 'setup', 'claude', '--consumer', 'claude').json;
  assert.match(claude.file, /claude_desktop_config\.json$/);
  assert.deepEqual(claude.add.mcpServers['w3bs-training'].args.slice(-2), ['--consumer', 'claude']);
  assert.equal(cli(dir, 'nonsense').error.code, 'UNKNOWN_COMMAND');
});

test('MCP over stdio retrieves granted demonstrations and exposes no human-only tools', async () => {
  const dir = home();
  const { t } = await releasedStore(dir);
  t.close();
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [
      'training/bin/w3bs-training.mjs',
      'mcp',
      'serve',
      '--stdio',
      '--consumer',
      'claude-code',
    ],
    env: env(dir),
  });
  const client = new Client({ name: 'test', version: '0.0.0' });
  await client.connect(transport);
  try {
    const tools = (await client.listTools()).tools.map((tool) => tool.name).sort();
    assert.deepEqual(tools, [
      'training_capabilities',
      'training_get_frames',
      'training_job_status',
      'training_propose_label',
      'training_report_run',
      'training_request_approval',
      'training_retrieve',
      'training_search_datasets',
    ]);
    const datasets = await client.callTool({ name: 'training_search_datasets', arguments: {} });
    assert.equal(datasets.structuredContent.items.length, 1);
    const result = await client.callTool({
      name: 'training_retrieve',
      arguments: { task: 'draft the release announcement', apps: ['Firefox'] },
    });
    assert.equal(result.isError, undefined);
    assert.equal(result.structuredContent.episodes.length, 1);
    assert.match(result.structuredContent.notice, /not instructions/);
    assert.equal(result.structuredContent.match, 'keyword');
    // Words that match no recorded goal still return the newest granted tasks.
    const loose = await client.callTool({
      name: 'training_retrieve',
      arguments: { task: 'zzqx unrelated wording' },
    });
    assert.equal(loose.structuredContent.match, 'recent');
    assert.equal(loose.structuredContent.episodes.length, 1);
    const frames = await client.callTool({
      name: 'training_get_frames',
      arguments: { episode_id: result.structuredContent.episodes[0].episode_id },
    });
    assert.equal(frames.isError, undefined, JSON.stringify(frames.content));
    assert.ok(frames.content.some((c) => c.type === 'image' && c.data.length > 0));
    const denied = await client.callTool({
      name: 'training_get_frames',
      arguments: { episode_id: 'ep_not_granted' },
    });
    assert.equal(denied.isError, true);
    const report = await client.callTool({
      name: 'training_report_run',
      arguments: {
        task: 'draft v0.4 notes',
        condition: 'with_demonstrations',
        outcome: 'failure',
        used_episodes: result.structuredContent.episodes.map((e) => e.episode_id),
      },
    });
    assert.equal(report.structuredContent.review_state, 'review_required');
    const ask = await client.callTool({
      name: 'training_request_approval',
      arguments: { kind: 'export', reason: 'I would like to train on this' },
    });
    assert.equal(ask.structuredContent.status, 'pending');
  } finally {
    await client.close();
  }
  const stranger = new Client({ name: 'test', version: '0.0.0' });
  await stranger.connect(
    new StdioClientTransport({
      command: process.execPath,
      args: ['training/bin/w3bs-training.mjs', 'mcp', 'serve', '--stdio', '--consumer', 'stranger'],
      env: env(dir),
    }),
  );
  try {
    const denied = await stranger.callTool({
      name: 'training_retrieve',
      arguments: { task: 'release' },
    });
    assert.equal(denied.isError, true);
    assert.match(denied.content[0].text, /SCOPE_DENIED/);
  } finally {
    await stranger.close();
  }
});

test('local app: one-time launch link, host check, CSRF header and session-gated artifacts', async () => {
  const dir = home();
  const { t, episode } = await releasedStore(dir);
  const local = createLocalApp(t, { user: 'reviewer' });
  const server = await new Promise((done) => {
    const s = local.app.listen(0, '127.0.0.1', () => done(s));
  });
  const port = server.address().port;
  local.setPort(port);
  const base = `http://127.0.0.1:${port}`;
  try {
    const locked = await fetch(`${base}/api/v1/training/status`);
    assert.equal(locked.status, 401);
    const page = await fetch(base);
    assert.equal(page.status, 401);
    assert.match(page.headers.get('content-security-policy'), /default-src 'none'/);
    const wrongHost = await fetch(`${base}/api/v1/training/status`, {
      headers: { host: 'evil.example' },
    });
    assert.equal(wrongHost.status, 421);
    const launch = await fetch(base + local.launchPath(), { redirect: 'manual' });
    assert.equal(launch.status, 303);
    const cookie = launch.headers.get('set-cookie').split(';')[0];
    assert.match(launch.headers.get('set-cookie'), /HttpOnly; SameSite=Strict/);
    assert.equal(
      (await fetch(base + local.launchPath(), { redirect: 'manual' })).status,
      401,
      'launch link is single-use',
    );
    const status = await fetch(`${base}/api/v1/training/status`, { headers: { cookie } });
    assert.equal((await status.json()).released_versions, 1);
    const detail = await (
      await fetch(`${base}/api/v1/training/episodes/${episode}`, { headers: { cookie } })
    ).json();
    const art = await fetch(
      `${base}/api/v1/training/artifacts/${detail.observations[0].artifact_id}`,
      { headers: { cookie } },
    );
    assert.equal(art.headers.get('content-type'), 'image/png');
    assert.equal(art.headers.get('x-content-type-options'), 'nosniff');
    const noHeader = await fetch(`${base}/api/v1/training/access`, {
      method: 'POST',
      headers: { cookie, 'content-type': 'application/json' },
      body: '{"on":false}',
    });
    assert.equal(noHeader.status, 403);
    const crossSite = await fetch(`${base}/api/v1/training/access`, {
      method: 'POST',
      headers: {
        cookie,
        'content-type': 'application/json',
        'x-w3bs-training': '1',
        origin: 'https://evil.example',
      },
      body: '{"on":false}',
    });
    assert.equal(crossSite.status, 403);
    const ok = await fetch(`${base}/api/v1/training/access`, {
      method: 'POST',
      headers: { cookie, 'content-type': 'application/json', 'x-w3bs-training': '1', origin: base },
      body: '{"on":false}',
    });
    assert.equal((await ok.json()).agent_access, 'off');
    const preflight = await fetch(`${base}/api/v1/training/status`, {
      method: 'OPTIONS',
      headers: { origin: 'https://evil.example' },
    });
    assert.equal(preflight.headers.get('access-control-allow-origin'), null);
    const html = await (await fetch(base, { headers: { cookie } })).text();
    assert.match(html, /W3BS Training/);
  } finally {
    server.close();
    t.close();
  }
});

test('w3bs.org serves the W3BS Training installers', async () => {
  const { startServer } = await import('../src/server.mjs');
  const { openStore } = await import('../src/store.mjs');
  const store = await openStore({ path: ':memory:' });
  const instance = await startServer({
    store,
    port: 0,
    publicOrigin: 'http://localhost:3000',
    mailer: { send: async () => ({ sent: true }) },
  });
  const base = `http://localhost:${instance.server.address().port}`;
  try {
    const sh = await fetch(`${base}/training/install.sh`);
    assert.equal(sh.status, 200);
    assert.match(sh.headers.get('content-type'), /text\/x-shellscript/);
    assert.match(await sh.text(), /^#!\/bin\/sh\n# W3BS Training installer/);
    const ps = await fetch(`${base}/training/install.ps1`);
    assert.equal(ps.status, 200);
    assert.match(await ps.text(), /Install-W3bsTraining/);
  } finally {
    await instance.close();
  }
});

test('terminal review renders a card with its privacy state', () => {
  const card = {
    episode_id: 'ep_1',
    revision: 1,
    goal: {
      value: 'Prepare a release-announcement draft',
      status: 'proposed',
      source: 'deterministic_rule',
    },
    outcome: { value: 'unknown', status: 'proposed' },
    capture_profile: 'action-supervision',
    observed_steps: 18,
    uncertain_labels: 2,
    privacy_warnings: 1,
    reasons: ['privacy warning'],
  };
  const text = renderToText(
    reviewView({
      cards: [card],
      selected: 0,
      budget: 5,
      detail: { warnings: [{ kind: 'secret_github_token', path: 'step_2' }], steps: [] },
      mode: 'list',
      field: { value: '', cursor: 0 },
      message: '',
    }),
    { width: 110, height: 20 },
  );
  assert.match(text, /Prepare a release-announcement draft/);
  assert.match(text, /1 warning\(s\): review before approval/);
  assert.match(text, /c Complete/);
});
