// W3BS Liquid Agent ("Teach an agent"): contract parity with Infernet's liquid-agent/v1,
// masking at source, permissions, consent, the guarded executor, the local API and the
// end-to-end slice on the mock sites. Training and serving use test/liquid-fake-infernet.mjs.
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { openTraining } from '../training/core/service.mjs';
import { createLocalApp } from '../training/ui/server.mjs';
import { canonicalize, episodeHash, hashCanonical } from '../training/liquid/canonical.mjs';
import { SCHEMAS } from '../training/liquid/contract.mjs';
import { validate } from '../training/liquid/validate.mjs';
import { buildBundle, verifyBundle } from '../training/liquid/dataset.mjs';
import { scanSecrets } from '../training/liquid/secrets.mjs';
import { isSecretField, toEpisodeV1 } from '../training/liquid/convert.mjs';
import { createExpert, createMockEnv, TASKS } from '../training/liquid/mock.mjs';
import {
  desktopLayerEnvironment,
  isExternalEffect,
  runPolicy,
} from '../training/liquid/executor.mjs';
import { PolicyRuntime } from '../training/liquid/runtime.mjs';
import { sampleBundle } from '../training/fixtures/sample.mjs';

const FAKE = fileURLToPath(new URL('./liquid-fake-infernet.mjs', import.meta.url));
const CONTRACT = fileURLToPath(new URL('../training/liquid/contract/fixtures/', import.meta.url));
const human = { kind: 'human', id: 'reviewer' };
const agent = { kind: 'agent', id: 'claude-code' };
const baseEnv = {
  W3BS_TRAINING_PASSPHRASE: 'correct horse battery staple',
  W3BS_INFERNET_BIN: FAKE,
  W3BS_C0MPUTE_BIN: FAKE,
};
const homes = [];
const opened = [];
async function fresh(extra = {}) {
  const home = mkdtempSync(join(tmpdir(), 'w3bs-liquid-'));
  homes.push(home);
  const t = await openTraining({ home, env: { ...baseEnv, ...extra } });
  opened.push(t);
  return { home, t, L: t.liquid };
}
test.after(async () => {
  for (const t of opened) await t.close().catch(() => {});
  homes.forEach((h) => rmSync(h, { recursive: true, force: true }));
});
const rejects = async (promise, code) =>
  assert.rejects(promise, (error) => {
    assert.equal(error.code, code, `${error.code}: ${error.message}`);
    return true;
  });
const readJson = (p) => JSON.parse(readFileSync(p, 'utf8'));
const grant = async (L, ...names) => {
  for (const n of names) await L.setPermission(n, true, human);
};
// 20 reviewed demonstrations -> dataset -> trained (fake) -> evaluated -> activated.
async function trained(L, { count = 20, evaluate = true } = {}) {
  await grant(L, 'recording', 'training_data');
  const demo = await L.recordMockDemonstrations({ count }, human);
  for (const e of demo.episodes)
    await L.review(e.episode_id, { success: true, reviewed: true }, human);
  const ds = await L.exportDataset({ name: 'Mock demos' }, human);
  const job = await L.waitJob((await L.train({ dataset_id: ds.dataset_id }, human)).job_id);
  assert.equal(job.state, 'succeeded', JSON.stringify(job.error));
  const model = job.models.find((m) => m.selected);
  let evaluation = null;
  if (evaluate) {
    evaluation = await L.evaluate({ model_id: model.model_id, per_workflow: 3 }, human);
    await L.activate(model.model_id, {}, human);
  }
  return { demo, ds, job, model, evaluation };
}

// ------------------------------------------------------------------ contract parity
test('contract: vendored schemas hash to contract.json and fixtures validate as Infernet says', () => {
  const contract = readJson(join(CONTRACT, 'contract.json'));
  assert.equal(
    hashCanonical(SCHEMAS),
    contract.schemas_hash,
    'schemas differ from the Infernet commit',
  );
  // The embedded copies are the files on disk, byte for byte after parsing.
  for (const [file, schema] of Object.entries(SCHEMAS))
    assert.deepEqual(
      schema,
      readJson(
        fileURLToPath(new URL(`../training/liquid/contract/schemas/${file}`, import.meta.url)),
      ),
    );
  const valid = readdirSync(join(CONTRACT, 'valid'));
  const invalid = readdirSync(join(CONTRACT, 'invalid'));
  assert.ok(valid.length >= 8 && invalid.length >= 10);
  for (const f of valid) assert.ok(validate(readJson(join(CONTRACT, 'valid', f))).ok, f);
  for (const f of invalid)
    assert.equal(validate(readJson(join(CONTRACT, 'invalid', f))).ok, false, f);
  const episode = readJson(join(CONTRACT, 'valid', 'episode.json'));
  assert.equal(episodeHash(episode), contract.episode_content_hash);
});

test('contract: W3BS re-derives the fixture bundle dataset_hash byte for byte', () => {
  const contract = readJson(join(CONTRACT, 'contract.json'));
  const bundle = readJson(join(CONTRACT, 'valid', 'dataset-bundle.json'));
  assert.equal(verifyBundle(bundle), contract.bundle_dataset_hash);
  const m = bundle.manifest;
  const rebuilt = buildBundle(bundle.episodes, {
    datasetId: m.dataset_id,
    name: m.name,
    createdAt: m.created_at,
    consent: m.consent,
    seed: m.splits.seed,
    strategy: m.splits.strategy,
    provenance: m.provenance,
    retention: m.retention,
    license: m.license,
    validation: 0.15,
    test: 0.25,
  });
  assert.equal(rebuilt.manifest.dataset_hash, contract.bundle_dataset_hash);
  assert.equal(canonicalize(rebuilt.manifest), canonicalize(m));
  // A W3BS-built episode re-hashes the way Infernet's runtime would.
  const ep = toEpisodeV1({
    episode_id: 'ep_w3bs_parity',
    task: 'email.reply_draft',
    created_at: '2026-10-26T00:00:00Z',
    consent_id: 'c1',
    steps: [
      {
        t_ms: 0,
        observation: { app: 'mail', elements: [{ role: 'link', name: 'Inbox' }] },
        action: { type: 'click', target: { role: 'link', name: 'Inbox' } },
      },
    ],
  });
  assert.equal(ep.content_hash, episodeHash(ep));
  assert.equal(
    ep.content_hash,
    hashCanonical(Object.fromEntries(Object.entries(ep).filter(([k]) => k !== 'content_hash'))),
  );
  assert.throws(
    () =>
      buildBundle(
        bundle.episodes.map((e, i) => (i ? e : { ...e, task: 'reddit.post_compose' })),
        { consent: m.consent },
      ),
    /content_hash/,
  );
});

// ------------------------------------------------------------------ masking
test('converter: secret fields never carry text, strict drops all typed text, scan flags names', () => {
  const steps = [
    {
      t_ms: 0,
      observation: {
        app: 'mail',
        url: 'https://user:pw@mail.example/signin?next=/inbox#x',
        elements: [{ role: 'textbox', name: 'Password' }],
      },
      action: {
        type: 'type',
        target: { role: 'textbox', name: 'Password' },
        text: 'hunter2-Secret!',
      },
    },
    {
      t_ms: 5,
      observation: {
        app: 'mail',
        elements: [
          { role: 'textbox', name: 'One-time code' },
          { role: 'text', name: 'token=abcdef1234567890' },
        ],
      },
      action: { type: 'type', target: { role: 'textbox', name: 'One-time code' }, text: '493021' },
    },
    {
      t_ms: 9,
      observation: { app: 'shop', focus: { role: 'textbox', name: 'Card number' }, elements: [] },
      action: { type: 'type', text: '4111 1111 1111 1111' },
    },
    {
      t_ms: 12,
      observation: {
        app: 'mail',
        image_ref: `sha256:${'a'.repeat(64)}`,
        elements: [{ role: 'textbox', name: 'Subject' }],
      },
      action: {
        type: 'type',
        target: { role: 'textbox', name: 'Subject' },
        text: 'Quarterly notes',
      },
    },
  ];
  for (const profile of ['strict', 'standard']) {
    const ep = toEpisodeV1({
      episode_id: 'ep_mask_test',
      task: 'email.reply_draft',
      created_at: '2026-10-26T00:00:00Z',
      consent_id: 'c',
      profile,
      steps,
    });
    const text = JSON.stringify(ep);
    for (const secret of ['hunter2', '493021', '4111', 'abcdef1234567890', 'user:pw', 'next='])
      assert.ok(!text.includes(secret), `${profile} leaked ${secret}`);
    assert.deepEqual(scanSecrets(ep), []);
    assert.equal(ep.steps[0].action.text_redacted, true);
    assert.equal(ep.steps[0].observation.url_pattern, 'mail.example/signin');
    assert.equal(ep.steps[2].action.text_redacted, true);
    assert.equal(ep.steps[1].observation.elements[1].name, '[redacted]');
    assert.equal(
      ep.steps[3].observation.image_ref,
      undefined,
      'no image_ref unless screenshots were chosen',
    );
    assert.equal(ep.steps[3].action.text, profile === 'standard' ? 'Quarterly notes' : undefined);
  }
  for (const name of [
    'Password',
    'PIN',
    'Passcode',
    'OTP',
    'Verification code',
    'API key',
    'CVV',
    'Card number',
  ])
    assert.ok(isSecretField({ role: 'textbox', name }), name);
  assert.equal(isSecretField({ role: 'textbox', name: 'Subject' }), false);
});

test(
  'export: no credential leaks in the bundle on disk; the secret scan runs before export',
  { timeout: 120000 },
  async () => {
    const { L } = await fresh();
    await grant(L, 'recording', 'training_data');
    const cap = await L.startCapture(
      { task: 'email.reply_draft', app: 'mail', profile: 'standard' },
      human,
    );
    const env = createMockEnv('email.reply_draft', { seed: 3, signIn: true });
    await L.captureStep(cap.capture_id, {
      t_ms: 0,
      observation: env.observe(),
      action: {
        type: 'type',
        target: { role: 'textbox', name: 'Password' },
        text: 'hunter2-Secret!',
      },
    });
    await L.captureStep(cap.capture_id, {
      t_ms: 400,
      observation: env.observe(),
      action: {
        type: 'type',
        target: { role: 'textbox', name: 'Email address' },
        text: 'me@example.com password: hunter2-Secret!',
      },
    });
    await L.captureStep(cap.capture_id, {
      t_ms: 900,
      observation: env.observe(),
      action: { type: 'click', target: { role: 'button', name: 'Sign in' } },
    });
    const stopped = await L.stopCapture(cap.capture_id, human);
    await L.review(stopped.episode_id, { success: true, reviewed: true }, human);
    const ds = await L.exportDataset({}, human);
    const raw = readFileSync(ds.path, 'utf8');
    assert.ok(!raw.includes('hunter2'), 'password text reached the export');
    assert.equal(verifyBundle(JSON.parse(raw)), ds.dataset_hash);
    // An episode posted over the API with a secret in a visible name is masked on the way in.
    const posted = toEpisodeV1({
      episode_id: 'ep_api_one',
      task: 'email.reply_draft',
      created_at: '2026-10-26T00:00:00Z',
      consent_id: 'c',
      steps: [
        {
          t_ms: 0,
          observation: { app: 'mail', elements: [{ role: 'text', name: 'ok' }] },
          action: { type: 'done' },
        },
      ],
    });
    posted.steps[0].observation.elements[0].name = 'Authorization: Bearer abcdefghijklmnop1234';
    const added = await L.addEpisodes([posted], human);
    const stored = await L.getEpisode(added.episodes[0]);
    assert.equal(stored.episode.steps[0].observation.elements[0].name, '[redacted]');
    assert.equal(stored.state, 'review_required', 'API episodes always land unreviewed');
  },
);

// ------------------------------------------------------------------ permissions and consent
test('permissions: all off on a fresh store; only a person grants; anyone revokes; audited', async () => {
  const { L } = await fresh();
  const perms = await L.permissions();
  assert.deepEqual(
    perms.map((p) => p.name),
    ['recording', 'training_data', 'network_upload', 'remote_inference', 'auto_execute'],
  );
  assert.ok(perms.every((p) => p.on === false));
  const status = await L.status();
  assert.equal(status.kill_switch, false);
  assert.equal(status.recording.on, false);
  assert.ok(
    status.policies.every((p) => p.mode === 'off'),
    'every task starts Off',
  );
  await rejects(L.setPermission('recording', true, agent), 'HUMAN_APPROVAL_REQUIRED');
  await L.setPermission('recording', true, human, { scope: { apps: ['mail'] } });
  assert.equal((await L.permissions())[0].on, true);
  await L.setPermission('recording', false, agent);
  assert.equal((await L.permissions())[0].on, false);
  const log = await L.auditLog({ kind: 'permission' });
  assert.deepEqual(
    log.map((r) => [r.kind, r.actor, r.detail.revocation]),
    [
      ['permission.revoked', 'agent:claude-code', true],
      ['permission.granted', 'human:reviewer', false],
    ],
  );
  assert.deepEqual(log[1].detail.scope, { apps: ['mail'] });
  await rejects(L.setPermission('telepathy', true, human), 'INVALID_INPUT');
});

test('consent: no capture before the permission and an explicit per-session start; scope enforced', async () => {
  const { L } = await fresh();
  await rejects(
    L.startCapture({ task: 'email.reply_draft', app: 'mail' }, human),
    'PERMISSION_REQUIRED',
  );
  await L.setPermission('recording', true, human);
  await rejects(
    L.startCapture({ task: 'email.reply_draft', app: 'mail' }, agent),
    'HUMAN_APPROVAL_REQUIRED',
  );
  await rejects(
    L.captureStep('cap_nope', { observation: { app: 'mail' }, action: { type: 'wait' } }),
    'NOT_FOUND',
  );
  const cap = await L.startCapture({ task: 'email.reply_draft', app: 'mail' }, human);
  assert.equal(cap.indicator, true);
  assert.equal((await L.status()).recording.on, true, 'the indicator shows while recording');
  await rejects(L.startCapture({ task: 'email.reply_draft', app: 'mail' }, human), 'INVALID_STATE');
  const env = createMockEnv('email.reply_draft', { seed: 1 });
  assert.equal(
    (
      await L.captureStep(cap.capture_id, {
        t_ms: 0,
        observation: env.observe(),
        action: { type: 'wait' },
      })
    ).captured,
    true,
  );
  const other = await L.captureStep(cap.capture_id, {
    t_ms: 1,
    observation: { app: 'browser', elements: [] },
    action: { type: 'wait' },
  });
  assert.equal(other.captured, false, 'another app is outside the scope');
  await L.pauseCapture(cap.capture_id, agent);
  await rejects(
    L.captureStep(cap.capture_id, {
      t_ms: 2,
      observation: env.observe(),
      action: { type: 'wait' },
    }),
    'NOT_RECORDING',
  );
  await rejects(L.resumeCapture(cap.capture_id, agent), 'HUMAN_APPROVAL_REQUIRED');
  await L.resumeCapture(cap.capture_id, human);
  // Revoking the permission stops the recording at once.
  await L.setPermission('recording', false, agent);
  const [row] = await L.listCaptures();
  assert.equal(row.state, 'stopped');
  assert.equal(row.excluded, 1);
  assert.equal((await L.status()).recording.on, false);
  await rejects(
    L.captureStep(cap.capture_id, {
      t_ms: 3,
      observation: env.observe(),
      action: { type: 'wait' },
    }),
    'NOT_RECORDING',
  );
});

test(
  'consent: no export or training without training-data use; no upload without permission, preview and confirmation',
  { timeout: 120000 },
  async () => {
    const { L } = await fresh();
    await L.setPermission('recording', true, human);
    const demo = await L.recordMockDemonstrations({ count: 6 }, human);
    for (const e of demo.episodes)
      await L.review(e.episode_id, { success: true, reviewed: true }, human);
    await rejects(L.exportDataset({}, human), 'PERMISSION_REQUIRED');
    await L.setPermission('training_data', true, human);
    await rejects(L.exportDataset({}, agent), 'HUMAN_APPROVAL_REQUIRED');
    const ds = await L.exportDataset({}, human);
    await rejects(
      L.previewRemoteTrain(
        { dataset_id: ds.dataset_id, worker: 'peer-12345', max_price_usd: 1 },
        human,
      ),
      'PERMISSION_REQUIRED',
    );
    await rejects(
      L.train({ dataset_id: ds.dataset_id, target: 'c0mpute' }, human),
      'PERMISSION_REQUIRED',
    );
    await L.setPermission('network_upload', true, human);
    await rejects(
      L.train({ dataset_id: ds.dataset_id, target: 'c0mpute' }, human),
      'PREVIEW_REQUIRED',
    );
    const preview = await L.previewRemoteTrain(
      { dataset_id: ds.dataset_id, worker: 'peer-12345', max_price_usd: 2.5 },
      human,
    );
    assert.equal(preview.dataset_hash, ds.dataset_hash);
    assert.equal(preview.episodes, 6);
    assert.match(preview.privacy, /sees the redacted dataset in plaintext/);
    await rejects(
      L.train(
        { dataset_id: ds.dataset_id, target: 'c0mpute', preview_id: preview.preview_id },
        human,
      ),
      'CONFIRMATION_REQUIRED',
    );
    // The fake c0mpute exits 3 without --yes, so success proves --yes was passed only now.
    const job = await L.waitJob(
      (
        await L.train(
          {
            dataset_id: ds.dataset_id,
            target: 'c0mpute',
            preview_id: preview.preview_id,
            confirm: true,
          },
          human,
        )
      ).job_id,
    );
    assert.equal(job.state, 'succeeded', JSON.stringify(job.error));
    assert.equal(job.train_job.privacy.mode, 'trusted_worker');
    assert.equal(job.train_job.privacy.upload_approved, true);
    assert.equal(job.receipts.length, 2);
    assert.ok(job.models.every((m) => m.source === 'c0mpute:peer-12345'));
    await rejects(
      L.train(
        {
          dataset_id: ds.dataset_id,
          target: 'c0mpute',
          preview_id: preview.preview_id,
          confirm: true,
        },
        human,
      ),
      'PREVIEW_REQUIRED',
    );
  },
);

// ------------------------------------------------------------------ review
test(
  'review: labels, reversible redaction, dedupe, only reviewed episodes export',
  { timeout: 120000 },
  async () => {
    const { L } = await fresh();
    await grant(L, 'recording', 'training_data');
    const cap = await L.startCapture(
      { task: 'reddit.post_compose', app: 'browser', profile: 'standard' },
      human,
    );
    const env = createMockEnv('reddit.post_compose', { seed: 2 });
    await L.captureStep(cap.capture_id, {
      t_ms: 0,
      observation: env.observe(),
      action: {
        type: 'type',
        target: { role: 'textbox', name: 'Search' },
        text: 'selfhosted mail',
      },
    });
    const { episode_id: id } = await L.stopCapture(cap.capture_id, human);
    await rejects(L.review(id, { reviewed: true }, agent), 'HUMAN_APPROVAL_REQUIRED');
    await rejects(L.review(id, { success: true, failure_reason: 'x' }, human), 'INVALID_INPUT');
    const reviewed = await L.review(
      id,
      {
        goal: 'Search for a sub',
        success: false,
        failure_reason: 'stopped early',
        risk: 'medium',
        reviewed: true,
      },
      human,
    );
    assert.deepEqual(reviewed.labels, {
      success: false,
      reviewed: true,
      risk: 'medium',
      goal: 'Search for a sub',
      failure_reason: 'stopped early',
    });
    const before = reviewed.episode.content_hash;
    const { redaction_id } = await L.redact(id, { step: 0, field: 'action.text' }, human);
    let ep = await L.getEpisode(id);
    assert.equal(ep.episode.steps[0].action.text, undefined);
    assert.equal(ep.episode.steps[0].action.text_redacted, true);
    assert.notEqual(ep.episode.content_hash, before);
    assert.equal(
      JSON.stringify(ep).includes('selfhosted mail'),
      false,
      'the original is not served',
    );
    ep = await L.unredact(id, redaction_id, human);
    assert.equal(ep.episode.steps[0].action.text, 'selfhosted mail');
    assert.equal(ep.episode.content_hash, before, 'undoing restores the same content');
    // Two identical episodes from the API: the later one is set aside.
    const mk = (eid) => ({
      ...toEpisodeV1({
        episode_id: eid,
        task: 'linkedin.notifications_review',
        created_at: '2026-10-26T00:00:00Z',
        consent_id: 'c',
        steps: [
          {
            t_ms: 0,
            observation: {
              app: 'browser',
              url_pattern: 'linkedin.mock/feed',
              elements: [{ role: 'link', name: 'Alerts' }],
            },
            action: { type: 'click', target: { role: 'link', name: 'Alerts' } },
          },
        ],
      }),
    });
    const added = await L.addEpisodes([mk('ep_dup_first'), mk('ep_dup_second')], human);
    for (const e of added.episodes) await L.review(e, { success: true, reviewed: true }, human);
    const { duplicates } = await L.dedupe(agent);
    assert.deepEqual(duplicates, [{ episode_id: 'ep_dup_second', dup_of: 'ep_dup_first' }]);
    await rejects(L.exportDataset({ episodes: ['ep_dup_second'] }, human), 'INVALID_STATE');
    const ds = await L.exportDataset({}, human);
    assert.equal(ds.episodes, 2, 'reviewed episodes only, duplicates left out');
  },
);

test('format 0.1: a Training-store episode converts to a masked, unreviewed EpisodeV1', async () => {
  const { t, L } = await fresh();
  const imp = await t.importFile({ data: sampleBundle(), filename: 'sample.zip' }, human);
  const { episodes } = await t.commitImport(imp.import_id, human);
  await rejects(L.importTrainingEpisode(episodes[0], {}, human), 'PERMISSION_REQUIRED');
  await L.setPermission('training_data', true, human);
  const r = await L.importTrainingEpisode(
    episodes[0],
    { task: 'w3bs.release_announcement' },
    human,
  );
  const ep = await L.getEpisode(r.episode_id);
  assert.equal(ep.state, 'review_required');
  assert.equal(ep.episode.labels.reviewed, false);
  assert.equal(ep.episode.capture.source, 'import');
  assert.ok(validate(ep.episode).ok);
  assert.ok(ep.episode.steps.every((s) => s.action.text === undefined));
  assert.deepEqual(scanSecrets(ep.episode), []);
  await rejects(L.importTrainingEpisode(episodes[0], {}, human), 'CONFLICT');
});

// ------------------------------------------------------------------ executor (unit)
const MH = `sha256:${'b'.repeat(64)}`;
function stubPolicy(actionFor, { seqShift = () => 0, model = MH } = {}) {
  return {
    open: async (scope) => ({ session_id: 's1', model_hash: MH, scope }),
    close: async () => {},
    predict: async (session, req) => {
      const action = actionFor(req);
      return {
        schema_version: 'liquid-agent/v1',
        kind: 'action_proposal',
        session_id: session.session_id,
        sequence_index: req.sequence_index + seqShift(req),
        model_hash: model,
        action,
        confidence: 0.9,
        uncertainty: 0.1,
        requires_approval: false,
        abstain_reason: action ? null : 'low_confidence',
      };
    },
  };
}
const gate = (over = {}) => ({
  killSwitch: async () => false,
  activeModelHash: async () => MH,
  permitted: async () => true,
  ...over,
});

test('executor: every external write asks a person in every mode, Bounded included; Suggest never executes', async () => {
  for (const mode of ['suggest', 'approve', 'bounded']) {
    for (const risky of [
      { type: 'send', target: { role: 'button', name: 'Send' } },
      { type: 'click', target: { role: 'button', name: 'Send' } },
    ]) {
      const env = createMockEnv('email.reply_draft', { seed: 4 });
      // Walk to the compose screen first, then the policy insists on sending.
      const expert = createExpert('email.reply_draft');
      const asks = [];
      const report = await runPolicy({
        env,
        task: 'email.reply_draft',
        mode,
        maxSteps: 12,
        policy: stubPolicy((req) =>
          req.observation.url_pattern === 'mail://compose'
            ? risky
            : expert.next(req.observation, { dt_ms: req.dt_ms, prev_action: req.prev_action }),
        ),
        gate: gate(),
        approve: async (q) => {
          asks.push(q);
          return { approved: !q.external };
        },
        human: async () => null,
      });
      assert.equal(env.writes.length, 0, `${mode}: a write reached the site`);
      assert.ok(
        !env.history.some((h) => h.result.status === 'blocked'),
        `${mode}: executor attempted an unapproved write`,
      );
      if (mode === 'suggest') assert.equal(report.executed.length, 0);
      else {
        const external = asks.filter((q) => q.external);
        assert.equal(external.length, 1, `${mode}: the external write was asked about`);
        assert.equal(report.denials, 1);
      }
    }
  }
  assert.equal(
    isExternalEffect({ type: 'click', target: { role: 'button', name: 'Submit post' } }),
    true,
  );
  assert.equal(
    isExternalEffect({ type: 'click', target: { role: 'button', name: 'Save draft' } }),
    false,
  );
});

test('executor: approved external write carries the approval id to the site', async () => {
  const env = createMockEnv('reddit.post_compose', { seed: 7 });
  const expert = createExpert('reddit.post_compose');
  const report = await runPolicy({
    env,
    task: 'reddit.post_compose',
    mode: 'bounded',
    policy: stubPolicy((req) =>
      expert.next(req.observation, { dt_ms: req.dt_ms, prev_action: req.prev_action }),
    ),
    gate: gate(),
    approve: async (q) => ({ approved: true, approval_id: `ok-${q.sequence_index}` }),
    human: async () => null,
    dtFor: (a) => (a.type === 'wait' ? 1200 : 700),
  });
  assert.equal(report.success, true);
  assert.equal(report.approvals_requested, 1, 'only the post needed a person in Bounded mode');
  assert.equal(env.writes.length, 1);
  assert.match(env.writes[0].approval, /^ok-\d+$/);
});

test('executor: kill switch wins before the next action, including one thrown during an approval wait', async () => {
  let killed = false;
  const env = createMockEnv('linkedin.notifications_review', { seed: 5 });
  const expert = createExpert('linkedin.notifications_review');
  const report = await runPolicy({
    env,
    task: 'linkedin.notifications_review',
    mode: 'approve',
    policy: stubPolicy((req) => expert.next(req.observation, { prev_action: req.prev_action })),
    gate: gate({ killSwitch: async () => killed }),
    approve: async () => {
      killed = true; // thrown while the person was deciding
      return { approved: true };
    },
    human: async () => null,
  });
  assert.equal(report.executed.length, 0);
  assert.equal(report.stopped_by, 'kill_switch');
});

test('executor: stale observation, model hash mismatch, out-of-schema, missing target and sequence errors', async () => {
  const run = (policy, extra = {}) => {
    const env = createMockEnv('email.reply_draft', { seed: 6 });
    return runPolicy({
      env,
      task: 'email.reply_draft',
      mode: 'bounded',
      policy,
      gate: gate(),
      approve: async () => ({ approved: true }),
      human: async () => null,
      ...extra,
      ...(extra.envHook ? extra.envHook(env) : {}),
    });
  };
  let r = await run(
    stubPolicy(() => ({ type: 'click', target: { role: 'link', name: 'Inbox' } }), {
      model: `sha256:${'c'.repeat(64)}`,
    }),
  );
  assert.equal(r.stopped_by, 'model_hash_mismatch');
  assert.equal(r.executed.length, 0);
  r = await run(
    stubPolicy(() => ({ type: 'click', target: { role: 'button', name: 'Not on screen' } })),
  );
  assert.equal(r.failures.target_not_visible, 1);
  assert.equal(r.executed.length, 0);
  r = await run(stubPolicy(() => ({ type: 'teleport' })));
  assert.equal(r.failures.out_of_schema, 1);
  r = await run(stubPolicy(() => ({ type: 'wait' }), { seqShift: () => 1 }));
  assert.equal(r.stopped_by, 'stale_session', 'a proposal for another sequence index is refused');
  // The screen changes between the proposal and the check.
  const env = createMockEnv('email.reply_draft', { seed: 6 });
  const observe = env.observe;
  let calls = 0;
  env.observe = () => {
    const o = observe();
    if (++calls % 2 === 0)
      o.elements = [...o.elements, { role: 'dialog', name: 'Update available' }];
    return o;
  };
  r = await runPolicy({
    env,
    task: 'email.reply_draft',
    mode: 'bounded',
    maxSteps: 2,
    policy: stubPolicy(() => ({ type: 'click', target: { role: 'link', name: 'Inbox' } })),
    gate: gate(),
    approve: async () => ({ approved: true }),
    human: async () => null,
  });
  assert.ok(r.failures.stale_observation >= 1);
  assert.equal(r.executed.length, 0);
  // Step bound.
  r = await run(
    stubPolicy(() => ({ type: 'wait' })),
    { maxSteps: 3 },
  );
  assert.equal(r.stopped_by, 'step_limit');
  assert.throws(() => desktopLayerEnvironment(), /mock sites/);
  await assert.rejects(
    run(
      stubPolicy(() => null),
      { mode: 'off' },
    ),
    /Off/,
  );
});

// ------------------------------------------------------------------ runtime contract (fake infernet)
test(
  'runtime: duplicate/late sequence ids and foreign account scopes are refused by the serve contract',
  { timeout: 120000 },
  async () => {
    const { L } = await fresh();
    const { model } = await trained(L, { count: 6, evaluate: false });
    const rt = new PolicyRuntime({ env: { ...process.env, ...baseEnv } });
    try {
      const m = (await L.listModels()).find((x) => x.model_id === model.model_id);
      await rejects(rt.load(m.model_dir, `sha256:${'0'.repeat(64)}`), 'E_MODEL_HASH');
      await rt.load(m.model_dir, m.model_hash);
      await rt.openSession(m.model_hash, 'sess-a', { account_scope: 'acct_a' });
      const env = createMockEnv('email.reply_draft', { seed: 1 });
      const req = (seq) => ({
        schema_version: 'liquid-agent/v1',
        kind: 'predict_request',
        session_id: 'sess-a',
        model_hash: m.model_hash,
        sequence_index: seq,
        dt_ms: 0,
        task: 'email.reply_draft',
        observation: env.observe(),
        prev_action: null,
      });
      assert.equal((await rt.predict(req(0), 'acct_a')).sequence_index, 0);
      await rejects(rt.predict(req(0), 'acct_a'), 'E_SEQ_DUPLICATE');
      await rejects(rt.predict(req(3), 'acct_a'), 'E_SEQ_GAP');
      await rejects(rt.predict(req(1), 'acct_b'), 'E_SESSION_UNKNOWN');
    } finally {
      await rt.close();
    }
  },
);

// ------------------------------------------------------------------ end to end
test(
  'end to end: 20 mock episodes -> reviewed dataset -> train -> evaluate -> activate -> suggest/execute with approval, audited',
  { timeout: 120000 },
  async () => {
    const { L } = await fresh();
    const { demo, ds, job, model, evaluation } = await trained(L);
    assert.equal(demo.episodes.length, 20);
    assert.ok(demo.episodes.every((e) => e.success));
    assert.equal(ds.episodes, 20);
    assert.ok(ds.counts.train > 0 && ds.counts.test + ds.counts.validation > 0);
    assert.equal(job.selected, 'cfc');
    assert.match(job.recommendation, /beat the GRU baseline/);
    assert.deepEqual(job.models.map((m) => m.family).sort(), ['cfc', 'gru']);
    assert.match(model.model_hash, /^sha256:[0-9a-f]{64}$/);
    assert.equal(evaluation.pass, true, evaluation.gaps.join('; '));
    for (const task of TASKS) {
      assert.equal(evaluation.workflows[task].task_success, 1);
      assert.ok(evaluation.layout_variant[task].episodes >= 1);
      assert.equal(evaluation.workflows[task].external_writes_without_approval, 0);
    }
    assert.ok(
      evaluation.workflows['reddit.post_compose'].approvals_requested >= 3,
      'every post was approved by the evaluator',
    );

    // Modes: Off by default; Suggest needs a person; acting needs auto_execute.
    await rejects(L.startRun({ task: 'email.reply_draft' }, human), 'POLICY_OFF');
    await rejects(L.setMode('email.reply_draft', 'suggest', agent), 'HUMAN_APPROVAL_REQUIRED');
    assert.equal((await L.capability('email.reply_draft')).recommended_mode, 'suggest');
    await rejects(L.setMode('email.reply_draft', 'approve', human), 'PERMISSION_REQUIRED');
    await L.setPermission('auto_execute', true, human);
    await rejects(
      L.setMode('reddit.post_compose', 'bounded', human, {
        bounds: { max_steps: 20, action_types: ['post'] },
      }),
      'INVALID_INPUT',
    );

    // Email draft reply, Execute with approval: every action asked, all approved.
    await L.setMode('email.reply_draft', 'approve', human);
    const asked = [];
    let r = await L.waitRun(
      (
        await L.startRun({ task: 'email.reply_draft', seed: 11 }, human, {
          approver: async (q) => (asked.push(q.action.type), { approved: true }),
          person: async () => null,
        })
      ).run_id,
    );
    assert.equal(r.state, 'finished', JSON.stringify(r.error));
    assert.equal(r.report.success, true);
    assert.equal(
      r.report.approvals_requested,
      r.report.executed.length,
      'every action needed approval',
    );
    assert.deepEqual(r.external_writes, [], 'a draft reply sends nothing');

    // Reddit compose, Bounded: the post (an external write) still waits for a person.
    await L.setMode('reddit.post_compose', 'bounded', human, { bounds: { max_steps: 30 } });
    const externals = [];
    r = await L.waitRun(
      (
        await L.startRun({ task: 'reddit.post_compose', seed: 12 }, human, {
          approver: async (q) => (
            externals.push(q.external),
            { approved: true, approval_id: 'person-1' }
          ),
          person: async () => null,
        })
      ).run_id,
    );
    assert.equal(r.report.success, true);
    assert.deepEqual(externals, [true], 'only the external write asked');
    assert.equal(r.external_writes.length, 1);
    assert.equal(r.external_writes[0].approval, 'person-1');

    // LinkedIn notifications, Suggest: the person follows each suggestion; nothing is executed by the agent.
    await L.setMode('linkedin.notifications_review', 'suggest', human);
    r = await L.waitRun(
      (
        await L.startRun({ task: 'linkedin.notifications_review', seed: 13 }, human, {
          approver: async () => ({ approved: false }),
          person: async (_o, p) => p.action,
        })
      ).run_id,
    );
    assert.equal(r.report.success, true);
    assert.equal(r.report.executed.length, 0);
    assert.equal(r.correction_episode, null);

    const kinds = new Set((await L.auditLog({ limit: 2000 })).map((a) => a.kind));
    for (const k of [
      'proposal',
      'approval.requested',
      'approval.granted',
      'action.executed',
      'action.by_person',
      'run.finished',
      'model.activate',
      'model.evaluate',
      'dataset.export',
      'train.succeeded',
      'capture.start',
      'capture.stop',
    ])
      assert.ok(kinds.has(k), `audit has ${k}`);
  },
);

test(
  'runs: kill switch under fault injection, account switch resets the session, corrections queue for review',
  { timeout: 120000 },
  async () => {
    const { L } = await fresh();
    await trained(L);
    await L.setPermission('auto_execute', true, human);
    await L.setMode('linkedin.notifications_review', 'bounded', human, {
      bounds: { max_steps: 30 },
    });
    // The kill switch is thrown (from another surface) right after the 2nd action.
    let r = await L.waitRun(
      (
        await L.startRun(
          { task: 'linkedin.notifications_review', seed: 21, faults: { killAfter: 2 } },
          human,
          { approver: async () => ({ approved: true }), person: async () => null },
        )
      ).run_id,
    );
    assert.equal(r.report.executed.length, 2, 'no action after the switch');
    assert.equal(r.report.stopped_by, 'kill_switch');
    await rejects(L.startRun({ task: 'linkedin.notifications_review' }, human), 'KILL_SWITCH');
    await rejects(L.setKillSwitch(false, agent), 'HUMAN_APPROVAL_REQUIRED');
    await L.setKillSwitch(false, human);

    // Account switch after the 2nd action: the old session is closed, a new one starts at 0.
    r = await L.waitRun(
      (
        await L.startRun(
          { task: 'linkedin.notifications_review', seed: 22, faults: { switchAccountAt: 2 } },
          human,
          { approver: async () => ({ approved: true }), person: async () => null },
        )
      ).run_id,
    );
    assert.equal(r.report.session_resets, 1);
    assert.equal(r.report.success, true, 'the run recovers on the new account from a clean state');
    assert.ok((await L.auditLog({ kind: 'session.reset' })).length >= 1);

    // Suggest mode where the person does something else once: a proposed correction episode.
    await L.setMode('email.reply_draft', 'suggest', human);
    let first = true;
    r = await L.waitRun(
      (
        await L.startRun({ task: 'email.reply_draft', seed: 23 }, human, {
          approver: async () => ({ approved: false }),
          person: async (_o, p) => {
            if (first && p.action?.type === 'click' && p.action.target?.role === 'link') {
              first = false;
              return { type: 'wait' };
            }
            return p.action;
          },
        })
      ).run_id,
    );
    assert.ok(r.correction_episode, 'a correction was queued');
    const corr = await L.getEpisode(r.correction_episode);
    assert.equal(corr.state, 'proposed');
    assert.equal(corr.source, 'correction');
    const ds = await L.exportDataset({}, human);
    assert.equal(
      ds.episodes,
      20,
      'the correction is not in any dataset until accepted and reviewed',
    );
    await rejects(L.acceptCorrection(r.correction_episode, agent), 'HUMAN_APPROVAL_REQUIRED');
    assert.equal((await L.acceptCorrection(r.correction_episode, human)).state, 'review_required');

    // Revoking auto_execute drops acting modes back to Suggest.
    await L.setPermission('auto_execute', false, agent);
    assert.equal((await L.policyFor('linkedin.notifications_review')).mode, 'suggest');
  },
);

test(
  'training: baseline selection, failures and activation gates',
  { timeout: 120000 },
  async () => {
    const { L } = await fresh({ FAKE_SELECT: 'gru' });
    const { job, model } = await trained(L, { count: 6, evaluate: false });
    assert.equal(job.selected, 'gru');
    assert.match(job.recommendation, /GRU \(baseline\) won/);
    assert.equal(model.family, 'gru');
    await rejects(L.activate(model.model_id, {}, human), 'EVALUATION_REQUIRED');
    await rejects(L.setMode('email.reply_draft', 'suggest', human), 'CAPABILITY_CHECK_FAILED');
    const failing = await fresh({ FAKE_TRAIN_FAIL: '1' });
    await grant(failing.L, 'recording', 'training_data');
    const demo = await failing.L.recordMockDemonstrations({ count: 3 }, human);
    for (const e of demo.episodes)
      await failing.L.review(e.episode_id, { success: true, reviewed: true }, human);
    const ds = await failing.L.exportDataset({}, human);
    const failed = await failing.L.waitJob(
      (await failing.L.train({ dataset_id: ds.dataset_id }, human)).job_id,
    );
    assert.equal(failed.state, 'failed');
    assert.match(failed.error.message, /injected fault/);
    const missing = await fresh({ W3BS_INFERNET_BIN: '', PATH: '/nonexistent' });
    await grant(missing.L, 'recording', 'training_data');
    const d2 = await missing.L.recordMockDemonstrations({ count: 1 }, human);
    await missing.L.review(d2.episodes[0].episode_id, { success: true, reviewed: true }, human);
    const ds2 = await missing.L.exportDataset({}, human);
    await rejects(missing.L.train({ dataset_id: ds2.dataset_id }, human), 'E_RUNTIME_MISSING');
  },
);

test(
  'evaluation: a reckless policy fails on the mock sites and cannot be activated',
  { timeout: 120000 },
  async () => {
    const { L } = await fresh({ FAKE_POLICY: 'reckless' });
    const { model } = await trained(L, { count: 6, evaluate: false });
    const report = await L.evaluate({ model_id: model.model_id, per_workflow: 2 }, human);
    assert.equal(report.pass, false);
    assert.ok(report.gaps.length);
    for (const task of TASKS)
      assert.equal(report.workflows[task].external_writes_without_approval, 0);
    assert.ok(
      report.workflows['email.reply_draft'].failure_modes.denied >= 1,
      'the evaluator denied the premature send',
    );
    await rejects(L.activate(model.model_id, {}, human), 'EVALUATION_REQUIRED');
  },
);

// ------------------------------------------------------------------ local API
test(
  'local API: liquid routes behind the human session, header and origin checks',
  { timeout: 120000 },
  async () => {
    const { t, L } = await fresh();
    const local = createLocalApp(t, { user: 'reviewer' });
    const server = await new Promise((done) => {
      const s = local.app.listen(0, '127.0.0.1', () => done(s));
    });
    const port = server.address().port;
    local.setPort(port);
    const base = `http://127.0.0.1:${port}/api/v1/training/liquid`;
    try {
      assert.equal((await fetch(`${base}/status`)).status, 401);
      const launch = await fetch(`http://127.0.0.1:${port}${local.launchPath()}`, {
        redirect: 'manual',
      });
      const cookie = launch.headers.get('set-cookie').split(';')[0];
      const call = async (path, { method = 'GET', body, headers = {} } = {}) => {
        const res = await fetch(base + path, {
          method,
          headers: {
            cookie,
            'content-type': 'application/json',
            'x-w3bs-training': '1',
            origin: `http://127.0.0.1:${port}`,
            ...headers,
          },
          body: body === undefined ? undefined : JSON.stringify(body),
        });
        return { status: res.status, body: await res.json() };
      };
      assert.equal(
        (
          await fetch(`${base}/permissions/recording`, {
            method: 'POST',
            headers: { cookie, 'content-type': 'application/json' },
            body: '{"on":true}',
          })
        ).status,
        403,
        'no CSRF header',
      );
      assert.equal(
        (await call('/status')).body.permissions.every((p) => !p.on),
        true,
      );
      for (const p of ['recording', 'training_data', 'auto_execute'])
        assert.equal(
          (await call(`/permissions/${p}`, { method: 'POST', body: { on: true } })).body.on,
          true,
        );
      // POST /training/episodes
      const ep = toEpisodeV1({
        episode_id: 'ep_http_one',
        task: 'email.reply_draft',
        created_at: '2026-10-26T00:00:00Z',
        consent_id: 'c',
        steps: [
          {
            t_ms: 0,
            observation: { app: 'mail', elements: [{ role: 'link', name: 'Inbox' }] },
            action: { type: 'click', target: { role: 'link', name: 'Inbox' } },
          },
        ],
      });
      assert.deepEqual(
        (await call('/episodes', { method: 'POST', body: { episodes: [ep] } })).body.episodes,
        ['ep_http_one'],
      );
      assert.equal(
        (
          await call('/episodes', {
            method: 'POST',
            body: { episodes: [{ ...ep, schema_version: 'x' }] },
          })
        ).status,
        422,
      );
      const demo = await call('/demo', { method: 'POST', body: { count: 9 } });
      for (const e of [...demo.body.episodes.map((x) => x.episode_id), 'ep_http_one'])
        assert.equal(
          (
            await call(`/episodes/${e}/review`, {
              method: 'POST',
              body: { success: true, reviewed: true },
            })
          ).status,
          200,
        );
      // POST /training/datasets
      const ds = (await call('/datasets', { method: 'POST', body: { name: 'http' } })).body;
      assert.equal(ds.episodes, 10);
      // POST /training/jobs + GET /training/jobs/:id
      const job = (await call('/jobs', { method: 'POST', body: { dataset_id: ds.dataset_id } }))
        .body;
      await L.waitJob(job.job_id);
      const got = (await call(`/jobs/${job.job_id}`)).body;
      assert.equal(got.state, 'succeeded');
      const modelId = got.models.find((m) => m.selected).model_id;
      // POST /training/evaluate + POST /training/models/:id/activate
      assert.equal(
        (await call(`/models/${modelId}/activate`, { method: 'POST', body: {} })).status,
        409,
      );
      const ev = (
        await call('/evaluate', { method: 'POST', body: { model_id: modelId, per_workflow: 2 } })
      ).body;
      assert.equal(ev.pass, true, JSON.stringify(ev.gaps));
      const act = (await call(`/models/${modelId}/activate`, { method: 'POST', body: {} })).body;
      assert.deepEqual(act.tasks.sort(), [...TASKS].sort());
      // A run answered through the asks queue, the way the local app does it.
      assert.equal(
        (
          await call('/tasks/reddit.post_compose/mode', {
            method: 'POST',
            body: { mode: 'bounded', bounds: { max_steps: 30 } },
          })
        ).body.mode,
        'bounded',
      );
      const run = (
        await call('/runs', {
          method: 'POST',
          body: { task: 'reddit.post_compose', seed: 5, faults: { killAfter: 1 } },
        })
      ).body;
      let asks = [];
      for (let i = 0; i < 200 && !asks.length; i += 1) {
        await new Promise((r) => setTimeout(r, 25));
        asks = (await call('/asks')).body;
      }
      assert.equal(asks.length, 1);
      assert.equal(asks[0].kind, 'approval');
      assert.equal(asks[0].external, true);
      assert.equal(
        (await call(`/asks/${asks[0].ask_id}`, { method: 'POST', body: { approve: true } })).body
          .approved,
        true,
      );
      const done = await L.waitRun(run.run_id);
      assert.equal(done.report.success, true, 'faults are not reachable over HTTP');
      assert.equal(done.external_writes.length, 1);
      assert.ok(
        (await call('/audit')).body.some(
          (r) => r.kind === 'approval.granted' && r.actor === 'human:person',
        ),
      );
      assert.equal(
        (await call('/kill-switch', { method: 'POST', body: { on: true } })).body.kill_switch,
        true,
      );
    } finally {
      server.close();
    }
  },
);

test(
  'CLI: teach status works; granting needs a terminal person; revoking and the kill switch do not',
  { timeout: 120000 },
  () => {
    const home = mkdtempSync(join(tmpdir(), 'w3bs-liquid-cli-'));
    homes.push(home);
    const cli = (...args) =>
      spawnSync(process.execPath, ['training/bin/w3bs-training.mjs', 'teach', ...args], {
        encoding: 'utf8',
        env: { ...process.env, ...baseEnv, W3BS_TRAINING_HOME: home },
        stdio: ['ignore', 'pipe', 'pipe'],
      });
    const status = cli('status');
    assert.equal(status.status, 0, status.stderr);
    assert.equal(
      JSON.parse(status.stdout).permissions.every((p) => !p.on),
      true,
    );
    const grantRes = cli('permissions', 'grant', 'recording');
    assert.equal(grantRes.status, 3);
    assert.match(grantRes.stderr, /HUMAN_APPROVAL_REQUIRED/);
    assert.equal(cli('permissions', 'revoke', 'recording').status, 0);
    assert.equal(JSON.parse(cli('kill-switch', 'on').stdout).kill_switch, true);
    assert.equal(cli('kill-switch', 'off').status, 3);
    assert.equal(cli('record', '--task', 'email.reply_draft', '--app', 'mail').status, 3);
  },
);
