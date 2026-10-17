import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { openTraining } from '../training/core/service.mjs';
import { collapseSteps, moshscriptFilename, toMoshscript } from '../training/core/moshscript.mjs';
import { createLocalApp } from '../training/ui/server.mjs';
import { sampleBundle } from '../training/fixtures/sample.mjs';

const human = { kind: 'human', id: 'reviewer' };
const homes = [];
test.after(() => homes.forEach((h) => rmSync(h, { recursive: true, force: true })));

async function store() {
  const dir = mkdtempSync(join(tmpdir(), 'w3bs-training-m-'));
  homes.push(dir);
  const t = await openTraining({
    home: dir,
    env: { ...process.env, W3BS_TRAINING_HOME: dir, W3BS_TRAINING_PASSPHRASE: 'pass phrase' },
  });
  const imp = await t.importFile({ data: sampleBundle(), filename: 's.zip' }, human);
  const { episodes } = await t.commitImport(imp.import_id, human);
  return { t, episode: episodes[0] };
}

// Runs a .mosh the way moshcode does (an async function body inside `with`),
// against stub verbs, and returns what it said and ran.
async function runMosh(script, argv = []) {
  const said = [];
  const shells = [];
  const ais = [];
  const scope = {
    argv,
    env: {},
    say: (...parts) => said.push(parts.join(' ')),
    shell: (cmd) => (shells.push(cmd), { ok: true, code: 0 }),
    ai: (prompt, opts) => (ais.push({ prompt, opts }), 'done'),
  };
  const AsyncFunction = Object.getPrototypeOf(async () => {}).constructor;
  const body = script.replace(/^#!.*\n/, '');
  await new AsyncFunction('__scope__', `with (__scope__) {\n${body}\n}`)(scope);
  return { said, shells, ais };
}

test('moshscript export needs a person and a reviewed, successful episode', async () => {
  const { t, episode } = await store();
  try {
    await assert.rejects(
      t.exportMoshscript(episode, { kind: 'agent', id: 'claude-code' }),
      /needs a person/,
    );
    await assert.rejects(t.exportMoshscript(episode, human), /reviewed, successful/);
    await t.review(episode, { decision: 'complete', confirm_goal: true }, human);
    const result = await t.exportMoshscript(episode, human);
    assert.match(result.filename, /\.mosh$/);
    assert.match(result.script, /^#!\/usr\/bin\/env moshscript\n/);
    assert.equal(result.run, `moshcode run ${result.filename}`);

    // The copy is an export: deleting the episode names it as outside W3BS.
    const report = await t.deleteEpisodes([episode], human);
    assert.deepEqual(
      report.exports_outside_w3bs.map((x) => x.export_id),
      [result.export_id],
    );
  } finally {
    t.close();
  }
});

test('the exported script hands the steps to an engine and replays nothing', async () => {
  const { t, episode } = await store();
  try {
    await t.review(episode, { decision: 'complete', confirm_goal: true }, human);
    const { script } = await t.exportMoshscript(episode, human);
    const { body } = await t.getEpisode(episode).then((e) => ({ body: e.episode }));

    const claude = await runMosh(script);
    assert.equal(claude.shells.length, 2, 'checks for w3bs, then opens claude');
    assert.match(claude.shells[1], /^claude '/);
    assert.ok(claude.shells[1].includes(body.goal.value.replace(/'/g, `'\\''`)));
    assert.match(claude.shells[1], /w3bs-desktop/);
    assert.match(claude.shells[1], /never reuse old/);
    assert.ok(claude.said[0].includes(body.goal.value));

    const other = await runMosh(script, ['gemini']);
    assert.equal(other.shells.length, 1);
    assert.equal(other.ais.length, 1);
    assert.deepEqual(other.ais[0].opts, { engine: 'gemini' });
  } finally {
    t.close();
  }
});

test('hostile store text stays data inside the script', async () => {
  const goal = 'it\'s `${process.exit(1)}` "quoted"\n// newline sep';
  const script = toMoshscript({
    episodeId: 'ep_0123456789abcdef',
    goal,
    explanation: "'); shell('rm -rf ~'); ('",
    steps: [
      { n: 1, type: 'focus', target: { app: 'TronBrowser', window: "a'b" }, evidence: 'observed' },
      { n: 2, type: 'focus', target: { app: 'TronBrowser', window: "a'b" }, evidence: 'observed' },
      { n: 3, type: 'focus', target: { app: 'hqterm' }, evidence: 'observed' },
    ],
    gaps: [{ kind: 'paused' }],
    stopConditions: ['x'],
    humanApprovals: ['y'],
    exportedAt: '2026-10-17T00:00:00.000Z',
    version: 'test',
  });
  const { shells, said } = await runMosh(script);
  assert.equal(shells.length, 2);
  assert.ok(said[0].includes('`${process.exit(1)}`'));
  assert.match(said[1], /2 step\(s\) across TronBrowser, hqterm/);
  // The comment header is one line per value, so a newline cannot end the comment.
  assert.ok(script.split('\n')[1].startsWith('// it'));
  assert.equal(
    moshscriptFilename(goal, 'ep_0123456789abcdef'),
    'it-s-process-exit-1-quoted-newline-sep-01234567.mosh',
  );
});

test('runs of the same focus step collapse into one', () => {
  const s = (n, app) => ({ n, type: 'focus', target: { app }, evidence: 'observed' });
  const out = collapseSteps([s(1, 'A'), s(2, 'A'), s(3, 'B'), s(4, 'A')]);
  assert.deepEqual(
    out.map((x) => [x.n, x.recorded, x.app]),
    [
      [1, '1-2', 'A'],
      [2, '3', 'B'],
      [3, '4', 'A'],
    ],
  );
});

test('local app downloads the .mosh as an attachment', async () => {
  const { t, episode } = await store();
  await t.review(episode, { decision: 'complete', confirm_goal: true }, human);
  const local = createLocalApp(t, { user: 'reviewer' });
  const server = await new Promise((done) => {
    const s = local.app.listen(0, '127.0.0.1', () => done(s));
  });
  local.setPort(server.address().port);
  const base = `http://127.0.0.1:${server.address().port}`;
  try {
    const launch = await fetch(base + local.launchPath(), { redirect: 'manual' });
    const cookie = launch.headers.get('set-cookie').split(';')[0];
    const noHeader = await fetch(`${base}/api/v1/training/episodes/${episode}/moshscript`, {
      method: 'POST',
      headers: { cookie },
    });
    assert.equal(noHeader.status, 403);
    const res = await fetch(`${base}/api/v1/training/episodes/${episode}/moshscript`, {
      method: 'POST',
      headers: { cookie, 'x-w3bs-training': '1', origin: base },
    });
    assert.equal(res.status, 200);
    assert.match(res.headers.get('content-disposition'), /^attachment; filename=".+\.mosh"$/);
    assert.match(await res.text(), /^#!\/usr\/bin\/env moshscript/);
    const exports = await (
      await fetch(`${base}/api/v1/training/exports`, { headers: { cookie } })
    ).json();
    assert.equal(exports.length, 1);
  } finally {
    server.close();
    t.close();
  }
});
