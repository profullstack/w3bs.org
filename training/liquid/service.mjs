// W3BS Liquid Agent: "Teach an agent" inside W3BS Training. Every surface (CLI, local app,
// HTTP) calls this module; policy lives here, as in training/core/service.mjs.
//
// Flow: consented, scoped capture -> masked EpisodeV1 drafts -> review/label/redact/dedupe ->
// DatasetBundleV1 export -> train (Infernet locally, or a trusted c0mpute worker after an
// explicit upload preview) -> evaluate on the mock sites -> activate -> Suggest / Execute
// with approval / Bounded automation through the guarded executor.
//
// Permissions (recording, training_data, network_upload, remote_inference, auto_execute) are
// separate, OFF on a fresh store, granted only by a person and revocable by anyone; every
// change is audit-logged. The kill switch is read fresh before every action.
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { decrypt, encrypt } from '../core/crypto.mjs';
import { newId } from '../core/ingest.mjs';
import { canonicalize, hashCanonical, sha256Hex } from './canonical.mjs';
import { FAMILIES, SCHEMA_VERSION } from './contract.mjs';
import { fromFormat01, maskStep, taskId, toEpisodeV1 } from './convert.mjs';
import { buildBundle, sealEpisode, verifyBundle } from './dataset.mjs';
import { MODES, runPolicy, runtimePolicy } from './executor.mjs';
import { APPS, GOALS, RISK, TASKS, createExpert, createMockEnv, humanDelay, rng } from './mock.mjs';
import { PolicyRuntime, requireBin, resolveBin, runJson } from './runtime.mjs';
import { assertNoSecrets } from './secrets.mjs';
import { assertValid } from './validate.mjs';

export const PERMISSIONS = {
  recording: 'Record a scoped Teach session (one app or window, one task)',
  training_data: 'Use reviewed episodes to build datasets and train models',
  network_upload: 'Send a reviewed, redacted dataset to a trusted c0mpute worker',
  remote_inference: 'Ask a model served by another machine for suggestions',
  auto_execute: 'Let the agent perform actions (each external write is still approved by you)',
};
export const DEFAULT_THRESHOLDS = { autonomous_success: 0.8, layout_autonomous_success: 0.5 };
const DDL = [
  'CREATE TABLE IF NOT EXISTS liquid_permissions (name TEXT PRIMARY KEY, state TEXT NOT NULL, scope TEXT, changed_at TEXT NOT NULL, actor TEXT NOT NULL)',
  'CREATE TABLE IF NOT EXISTS liquid_audit (sequence INTEGER PRIMARY KEY AUTOINCREMENT, at TEXT NOT NULL, actor TEXT NOT NULL, kind TEXT NOT NULL, subject TEXT, detail TEXT)',
  'CREATE TABLE IF NOT EXISTS liquid_captures (capture_id TEXT PRIMARY KEY, task TEXT NOT NULL, app TEXT NOT NULL, state TEXT NOT NULL, consent TEXT NOT NULL, started_at TEXT NOT NULL, updated_at TEXT NOT NULL, steps INTEGER NOT NULL DEFAULT 0, excluded INTEGER NOT NULL DEFAULT 0, episode_id TEXT)',
  'CREATE TABLE IF NOT EXISTS liquid_episodes (episode_id TEXT PRIMARY KEY, source TEXT NOT NULL, state TEXT NOT NULL, task TEXT NOT NULL, fingerprint TEXT NOT NULL, dup_of TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL)',
  'CREATE TABLE IF NOT EXISTS liquid_datasets (dataset_id TEXT PRIMARY KEY, dataset_hash TEXT NOT NULL, created_at TEXT NOT NULL, actor TEXT NOT NULL, body TEXT NOT NULL)',
  'CREATE TABLE IF NOT EXISTS liquid_previews (preview_id TEXT PRIMARY KEY, created_at TEXT NOT NULL, body TEXT NOT NULL, used_at TEXT)',
  'CREATE TABLE IF NOT EXISTS liquid_jobs (job_id TEXT PRIMARY KEY, dataset_id TEXT NOT NULL, target TEXT NOT NULL, state TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, actor TEXT NOT NULL, body TEXT NOT NULL)',
  'CREATE TABLE IF NOT EXISTS liquid_models (model_id TEXT PRIMARY KEY, model_hash TEXT NOT NULL UNIQUE, family TEXT NOT NULL, job_id TEXT NOT NULL, created_at TEXT NOT NULL, body TEXT NOT NULL)',
  'CREATE TABLE IF NOT EXISTS liquid_evals (eval_id TEXT PRIMARY KEY, model_id TEXT NOT NULL, model_hash TEXT NOT NULL, pass INTEGER NOT NULL, created_at TEXT NOT NULL, body TEXT NOT NULL)',
  'CREATE TABLE IF NOT EXISTS liquid_activations (task TEXT PRIMARY KEY, model_id TEXT NOT NULL, model_hash TEXT NOT NULL, activated_at TEXT NOT NULL, actor TEXT NOT NULL)',
  'CREATE TABLE IF NOT EXISTS liquid_policies (task TEXT PRIMARY KEY, mode TEXT NOT NULL, bounds TEXT, updated_at TEXT NOT NULL, actor TEXT NOT NULL)',
  'CREATE TABLE IF NOT EXISTS liquid_runs (run_id TEXT PRIMARY KEY, task TEXT NOT NULL, mode TEXT NOT NULL, state TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, body TEXT NOT NULL)',
  'CREATE TABLE IF NOT EXISTS liquid_asks (ask_id TEXT PRIMARY KEY, run_id TEXT NOT NULL, kind TEXT NOT NULL, state TEXT NOT NULL, created_at TEXT NOT NULL, decided_at TEXT, actor TEXT, body TEXT NOT NULL, answer TEXT)',
];

const fail = (code, message, status = 400, details) => {
  const e = new Error(message);
  e.code = code;
  e.status = status;
  if (details) e.details = details;
  return e;
};
const json = (v) => JSON.stringify(v);
const parse = (s) => (s == null ? null : JSON.parse(s));
const percentile = (xs, p) => {
  if (!xs.length) return null;
  const s = [...xs].sort((a, b) => a - b);
  return Number(s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))].toFixed(3));
};

export async function openLiquid({
  db,
  blobs,
  keys,
  home,
  env = process.env,
  now = () => new Date(),
  training,
}) {
  for (const sql of DDL) await db.run(sql);
  const spawnEnv = { ...process.env, ...env };
  const iso = () => now().toISOString();
  const who = (actor) => `${actor?.kind ?? 'unknown'}:${actor?.id ?? 'unknown'}`;
  const dataDir = home ? resolve(home, 'liquid') : null;
  const live = { runtimes: new Set(), jobs: new Map(), runs: new Map(), waiters: new Map() };

  const audit = async (actor, kind, subject, detail) =>
    await db.run(
      'INSERT INTO liquid_audit (at, actor, kind, subject, detail) VALUES (?, ?, ?, ?, ?)',
      [
        iso(),
        typeof actor === 'string' ? actor : who(actor),
        kind,
        subject ?? null,
        detail === undefined ? null : json(detail),
      ],
    );
  const requireHuman = (actor, what) => {
    if (actor?.kind !== 'human')
      throw fail(
        'HUMAN_APPROVAL_REQUIRED',
        `${what} needs a person. Use the Teach page in W3BS Training or run the command in an interactive terminal.`,
        403,
      );
  };
  const meta = async (key, fallback) =>
    (await db.get('SELECT value FROM meta WHERE key = ?', [key]))?.value ?? fallback;
  const setMeta = (key, value) =>
    db.run(
      'INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value',
      [key, value],
    );

  // Sealed blobs (AAD = id) for anything carrying captured content.
  const seal = (kind, id, value) =>
    blobs.put(
      `liquid/${kind}/${id}.bin`,
      encrypt(keys.key(), Buffer.from(json(value)), `${kind}:${id}`),
    );
  const unseal = async (kind, id) => {
    const sealed = await blobs.get(`liquid/${kind}/${id}.bin`);
    if (!sealed) throw fail('NOT_FOUND', `${kind} ${id} is gone.`, 404);
    return JSON.parse(decrypt(keys.key(), sealed, `${kind}:${id}`).toString());
  };

  // ------------------------------------------------------------------ permissions
  const permissionState = async (name) =>
    (await db.get('SELECT state FROM liquid_permissions WHERE name = ?', [name]))?.state === 'on';
  const requirePermission = async (name, what) => {
    if (!(await permissionState(name)))
      throw fail(
        'PERMISSION_REQUIRED',
        `${what} needs the "${name}" permission, which is off. Turn it on in Teach > Permissions.`,
        403,
        { permission: name },
      );
  };
  const killSwitch = async () => (await meta('liquid.kill_switch', 'off')) === 'on';

  // ------------------------------------------------------------------ episodes
  const fingerprint = (ep) =>
    sha256Hex(
      canonicalize({
        task: ep.task,
        steps: ep.steps.map((s) => [s.observation.url_pattern ?? '', s.action]),
      }),
    );
  const episodeRow = async (id) => {
    const row = await db.get('SELECT * FROM liquid_episodes WHERE episode_id = ?', [id]);
    if (!row) throw fail('NOT_FOUND', `Episode ${id} was not found.`, 404);
    return row;
  };
  const loadEpisode = async (id) => ({
    row: await episodeRow(id),
    ...(await unseal('episodes', id)),
  });
  const storeEpisode = async (episode, { source, state, redactions = [], meta: extra = {} }) => {
    assertValid(episode, 'episode');
    assertNoSecrets(episode, `episode ${episode.episode_id}`);
    const fp = fingerprint(episode);
    const exists = await db.get('SELECT 1 FROM liquid_episodes WHERE episode_id = ?', [
      episode.episode_id,
    ]);
    if (exists)
      await db.run(
        'UPDATE liquid_episodes SET state = ?, task = ?, fingerprint = ?, updated_at = ? WHERE episode_id = ?',
        [state, episode.task, fp, iso(), episode.episode_id],
      );
    else
      await db.run(
        'INSERT INTO liquid_episodes (episode_id, source, state, task, fingerprint, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)',
        [episode.episode_id, source, state, episode.task, fp, iso(), iso()],
      );
    await seal('episodes', episode.episode_id, { episode, redactions, meta: extra });
    return episode;
  };
  const summary = (row, episode) => ({
    episode_id: row.episode_id,
    source: row.source,
    state: row.state,
    task: row.task,
    dup_of: row.dup_of ?? null,
    steps: episode?.steps.length,
    labels: episode?.labels,
    content_hash: episode?.content_hash,
    created_at: row.created_at,
    updated_at: row.updated_at,
  });

  // ------------------------------------------------------------------ models
  const modelRow = async (id) => {
    const row = await db.get('SELECT * FROM liquid_models WHERE model_id = ? OR model_hash = ?', [
      id,
      id,
    ]);
    if (!row) throw fail('NOT_FOUND', `Model ${id} was not found.`, 404);
    return { ...row, body: parse(row.body) };
  };
  const activeFor = async (task) =>
    await db.get('SELECT * FROM liquid_activations WHERE task = ?', [task]);
  const policyFor = async (task) => {
    const row = await db.get('SELECT * FROM liquid_policies WHERE task = ?', [task]);
    return row
      ? {
          task,
          mode: row.mode,
          bounds: parse(row.bounds),
          updated_at: row.updated_at,
          actor: row.actor,
        }
      : { task, mode: 'off', bounds: null };
  };
  const passingEval = async (modelHash) =>
    await db.get(
      'SELECT * FROM liquid_evals WHERE model_hash = ? AND pass = 1 ORDER BY created_at DESC LIMIT 1',
      [modelHash],
    );

  // Registers the models a train run produced, checking each weights file against its hash.
  const registerModels = async (jobId, output, source, receipts = []) => {
    const out = [];
    for (const r of output.results ?? []) {
      if (!FAMILIES.includes(r.family)) throw fail('E_SCHEMA', `unknown family ${r.family}`, 502);
      const manifestPath = join(r.model_dir, 'manifest.json');
      const manifest = existsSync(manifestPath)
        ? JSON.parse(readFileSync(manifestPath, 'utf8'))
        : null;
      if (manifest) {
        assertValid(manifest, 'model_manifest');
        const weights = readFileSync(join(r.model_dir, manifest.weights.file));
        const actual = `sha256:${sha256Hex(weights)}`;
        if (actual !== manifest.weights.weights_hash || actual !== r.model_hash)
          throw fail(
            'E_HASH_MISMATCH',
            `${r.family} weights hash ${actual} does not match the reported ${r.model_hash}`,
            502,
          );
        if (manifest.training.dataset_hash !== output.dataset_hash)
          throw fail(
            'E_HASH_MISMATCH',
            `${r.family} was trained on ${manifest.training.dataset_hash}, not ${output.dataset_hash}`,
            502,
          );
      }
      const model_id = manifest?.model_id ?? `lm_${r.model_hash.slice(7, 23)}`;
      const body = {
        model_id,
        model_hash: r.model_hash,
        family: r.family,
        model_dir: r.model_dir,
        dataset_hash: output.dataset_hash,
        metrics: r.eval?.metrics ?? manifest?.eval?.metrics ?? {},
        selected: output.selected === r.family,
        source,
        receipts: receipts.filter((x) => x.output_hash === r.model_hash),
        tasks: manifest?.input_schema?.tasks ?? [],
        manifest_hash: manifest?.manifest_hash ?? null,
        job_id: jobId,
      };
      await db.run(
        'INSERT INTO liquid_models (model_id, model_hash, family, job_id, created_at, body) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(model_id) DO UPDATE SET body = excluded.body',
        [model_id, r.model_hash, r.family, jobId, iso(), json(body)],
      );
      out.push(body);
    }
    return out;
  };

  // ------------------------------------------------------------------ asks (approvals)
  // A run waiting for a person: an approval, a suggestion to act on, or a take-over.
  const ask = async (runId, kind, body, { timeoutMs = 10 * 60 * 1000 } = {}) => {
    const askId = newId('ask');
    await db.run(
      'INSERT INTO liquid_asks (ask_id, run_id, kind, state, created_at, body) VALUES (?, ?, ?, ?, ?, ?)',
      [askId, runId, kind, 'pending', iso(), json(body)],
    );
    return await new Promise((done) => {
      const timer = setTimeout(async () => {
        live.waiters.delete(askId);
        await db.run(
          "UPDATE liquid_asks SET state = 'expired', decided_at = ? WHERE ask_id = ? AND state = 'pending'",
          [iso(), askId],
        );
        done({ expired: true });
      }, timeoutMs);
      live.waiters.set(askId, (answer) => {
        clearTimeout(timer);
        live.waiters.delete(askId);
        done(answer);
      });
    });
  };
  const denyAllPending = async (reason) => {
    for (const row of await db.all("SELECT ask_id FROM liquid_asks WHERE state = 'pending'")) {
      await db.run(
        "UPDATE liquid_asks SET state = 'cancelled', decided_at = ?, answer = ? WHERE ask_id = ?",
        [iso(), json({ reason }), row.ask_id],
      );
      live.waiters.get(row.ask_id)?.({ approved: false, stop: true, reason });
    }
  };

  const liquid = {
    PERMISSIONS,
    killSwitch,

    async status() {
      const counts = Object.fromEntries(
        (await db.all('SELECT state, COUNT(*) AS n FROM liquid_episodes GROUP BY state')).map(
          (r) => [r.state, Number(r.n)],
        ),
      );
      const capture = await db.get(
        "SELECT * FROM liquid_captures WHERE state IN ('recording', 'paused') ORDER BY started_at DESC LIMIT 1",
      );
      return {
        feature: 'W3BS Liquid Agent',
        schema_version: SCHEMA_VERSION,
        permissions: await liquid.permissions(),
        kill_switch: await killSwitch(),
        recording: capture
          ? {
              on: capture.state === 'recording',
              state: capture.state,
              capture_id: capture.capture_id,
              task: capture.task,
              app: capture.app,
              since: capture.started_at,
              steps: Number(capture.steps),
            }
          : { on: false, state: 'idle' },
        episodes: counts,
        datasets: Number((await db.get('SELECT COUNT(*) AS n FROM liquid_datasets')).n),
        models: Number((await db.get('SELECT COUNT(*) AS n FROM liquid_models')).n),
        activations: await db.all(
          'SELECT task, model_id, model_hash, activated_at FROM liquid_activations ORDER BY task',
        ),
        policies: await Promise.all(TASKS.map(policyFor)),
        pending_asks: Number(
          (await db.get("SELECT COUNT(*) AS n FROM liquid_asks WHERE state = 'pending'")).n,
        ),
        runtimes: {
          infernet: !!resolveBin('infernet', spawnEnv),
          c0mpute: !!resolveBin('c0mpute', spawnEnv),
        },
        executors: ['mock'],
      };
    },

    // -------------------------------------------------------------- permissions
    async permissions() {
      const rows = new Map(
        (await db.all('SELECT * FROM liquid_permissions')).map((r) => [r.name, r]),
      );
      return Object.entries(PERMISSIONS).map(([name, description]) => {
        const r = rows.get(name);
        return {
          name,
          description,
          on: r?.state === 'on',
          changed_at: r?.changed_at ?? null,
          by: r?.actor ?? null,
          scope: parse(r?.scope ?? null),
        };
      });
    },
    // A person grants; anyone (an agent too) may revoke. Revocation takes effect at once.
    async setPermission(name, on, actor, { scope } = {}) {
      if (!PERMISSIONS[name])
        throw fail(
          'INVALID_INPUT',
          `Unknown permission ${name}. One of: ${Object.keys(PERMISSIONS).join(', ')}.`,
        );
      if (on) requireHuman(actor, `Turning on "${name}"`);
      await db.run(
        'INSERT INTO liquid_permissions (name, state, scope, changed_at, actor) VALUES (?, ?, ?, ?, ?) ON CONFLICT(name) DO UPDATE SET state = excluded.state, scope = excluded.scope, changed_at = excluded.changed_at, actor = excluded.actor',
        [name, on ? 'on' : 'off', scope ? json(scope) : null, iso(), who(actor)],
      );
      await audit(actor, on ? 'permission.granted' : 'permission.revoked', name, {
        scope: scope ?? null,
        revocation: !on,
      });
      if (!on && name === 'recording')
        for (const c of await db.all(
          "SELECT capture_id FROM liquid_captures WHERE state IN ('recording', 'paused')",
        ))
          await liquid.stopCapture(c.capture_id, { kind: 'system', id: 'permission-revoked' });
      if (!on && name === 'auto_execute') {
        for (const p of await db.all(
          "SELECT task FROM liquid_policies WHERE mode IN ('approve', 'bounded')",
        )) {
          await db.run(
            "UPDATE liquid_policies SET mode = 'suggest', bounds = NULL, updated_at = ?, actor = ? WHERE task = ?",
            [iso(), who(actor), p.task],
          );
          await audit(actor, 'policy.mode', p.task, {
            mode: 'suggest',
            reason: 'auto_execute revoked',
          });
        }
        await denyAllPending('auto_execute revoked');
      }
      return (await liquid.permissions()).find((p) => p.name === name);
    },
    async setKillSwitch(on, actor) {
      if (!on) requireHuman(actor, 'Releasing the kill switch');
      await setMeta('liquid.kill_switch', on ? 'on' : 'off');
      await audit(actor, on ? 'kill_switch.on' : 'kill_switch.off', null, {});
      if (on) {
        await denyAllPending('kill switch');
        for (const rt of live.runtimes) rt.killSwitch(true).catch(() => {});
      }
      return { kill_switch: on };
    },

    // -------------------------------------------------------------- capture
    async startCapture({ task, app, window, profile = 'strict', screenshots = false }, actor) {
      requireHuman(actor, 'Starting a Teach recording');
      await requirePermission('recording', 'Recording');
      if (!app) throw fail('INVALID_INPUT', 'Choose the one app or window to record.');
      if (!['strict', 'standard'].includes(profile))
        throw fail('INVALID_INPUT', 'profile is strict or standard.');
      if (await db.get("SELECT 1 FROM liquid_captures WHERE state IN ('recording', 'paused')"))
        throw fail('INVALID_STATE', 'A Teach recording is already running. Stop it first.', 409);
      const id = taskId(task ?? 'w3bs.task');
      const captureId = newId('cap');
      const consent = {
        consent_id: `cs_${captureId.slice(4)}`,
        scope: {
          app: String(app).slice(0, 64),
          window: window ? String(window).slice(0, 200) : null,
          task: id,
          screenshots: !!screenshots,
          profile,
        },
        purposes: ['capture'],
        granted_at: iso(),
        granted_by: who(actor),
      };
      await db.run(
        'INSERT INTO liquid_captures (capture_id, task, app, state, consent, started_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)',
        [captureId, id, consent.scope.app, 'recording', json(consent), iso(), iso()],
      );
      await seal('captures', captureId, { steps: [] });
      await audit(actor, 'capture.start', captureId, consent.scope);
      return { capture_id: captureId, state: 'recording', indicator: true, consent };
    },
    // The recorder adapter's entry point. Masked here, before anything is stored.
    async captureStep(captureId, step, actor = { kind: 'system', id: 'recorder' }) {
      const row = await db.get('SELECT * FROM liquid_captures WHERE capture_id = ?', [captureId]);
      if (!row) throw fail('NOT_FOUND', `No Teach recording ${captureId}.`, 404);
      if (row.state !== 'recording')
        throw fail(
          'NOT_RECORDING',
          `Recording ${captureId} is ${row.state}; nothing was captured.`,
          409,
        );
      if (!(await permissionState('recording')))
        throw fail('PERMISSION_REQUIRED', 'Recording permission was revoked.', 403);
      const consent = parse(row.consent);
      if ((step?.observation?.app ?? '') !== consent.scope.app) {
        await db.run(
          'UPDATE liquid_captures SET excluded = excluded + 1, updated_at = ? WHERE capture_id = ?',
          [iso(), captureId],
        );
        return { captured: false, reason: 'outside the recording scope' };
      }
      const masked = maskStep(step, {
        profile: consent.scope.profile,
        screenshots: consent.scope.screenshots,
      });
      const data = await unseal('captures', captureId);
      data.steps.push(masked);
      await seal('captures', captureId, data);
      await db.run(
        'UPDATE liquid_captures SET steps = steps + 1, updated_at = ? WHERE capture_id = ?',
        [iso(), captureId],
      );
      return { captured: true, steps: data.steps.length };
    },
    async pauseCapture(captureId, actor) {
      return liquid.setCaptureState(captureId, 'paused', ['recording'], actor);
    },
    async resumeCapture(captureId, actor) {
      requireHuman(actor, 'Resuming a recording');
      await requirePermission('recording', 'Recording');
      return liquid.setCaptureState(captureId, 'recording', ['paused'], actor);
    },
    async setCaptureState(captureId, state, from, actor) {
      const row = await db.get('SELECT state FROM liquid_captures WHERE capture_id = ?', [
        captureId,
      ]);
      if (!row) throw fail('NOT_FOUND', `No Teach recording ${captureId}.`, 404);
      if (!from.includes(row.state))
        throw fail('INVALID_STATE', `Recording ${captureId} is ${row.state}.`, 409);
      await db.run('UPDATE liquid_captures SET state = ?, updated_at = ? WHERE capture_id = ?', [
        state,
        iso(),
        captureId,
      ]);
      await audit(actor, `capture.${state === 'paused' ? 'pause' : 'resume'}`, captureId, {});
      return { capture_id: captureId, state, indicator: state === 'recording' };
    },
    // Stop: the steps become one unreviewed episode draft. Anyone may stop a recording.
    async stopCapture(captureId, actor, { discard = false } = {}) {
      const row = await db.get('SELECT * FROM liquid_captures WHERE capture_id = ?', [captureId]);
      if (!row) throw fail('NOT_FOUND', `No Teach recording ${captureId}.`, 404);
      if (!['recording', 'paused'].includes(row.state))
        throw fail('INVALID_STATE', `Recording ${captureId} is already ${row.state}.`, 409);
      const consent = parse(row.consent);
      const data = await unseal('captures', captureId);
      let episodeId = null;
      if (!discard && data.steps.length) {
        const t0 = data.steps[0].t_ms;
        const episode = toEpisodeV1({
          episode_id: `ep_${captureId.slice(4)}`,
          task: row.task,
          created_at: row.started_at,
          split_group: captureId,
          consent_id: consent.consent_id,
          profile: consent.scope.profile,
          source: 'w3bs-desktop',
          app_scope: [consent.scope.app],
          screenshots: consent.scope.screenshots,
          steps: data.steps.map((s) => ({ ...s, t_ms: s.t_ms - t0 })),
          labels: {
            reviewed: false,
            success: false,
            risk: RISK[row.task] ?? 'low',
            goal: GOALS[row.task],
          },
        });
        await storeEpisode(episode, { source: 'capture', state: 'review_required' });
        episodeId = episode.episode_id;
      }
      await blobs.del(`liquid/captures/${captureId}.bin`);
      await db.run(
        'UPDATE liquid_captures SET state = ?, updated_at = ?, episode_id = ? WHERE capture_id = ?',
        [discard ? 'discarded' : 'stopped', iso(), episodeId, captureId],
      );
      await audit(actor, discard ? 'capture.discard' : 'capture.stop', captureId, {
        steps: data.steps.length,
        excluded: Number(row.excluded),
        episode_id: episodeId,
      });
      return {
        capture_id: captureId,
        state: discard ? 'discarded' : 'stopped',
        indicator: false,
        episode_id: episodeId,
        steps: data.steps.length,
        excluded: Number(row.excluded),
      };
    },
    async listCaptures() {
      return (
        await db.all(
          'SELECT capture_id, task, app, state, started_at, updated_at, steps, excluded, episode_id FROM liquid_captures ORDER BY started_at DESC LIMIT 200',
        )
      ).map((r) => ({
        ...r,
        steps: Number(r.steps),
        excluded: Number(r.excluded),
      }));
    },

    // Teach on the mock sites: the scripted demonstrator performs each task inside an ordinary
    // consented Teach recording, so the episodes go through the same capture, masking and
    // review path as a real one. Lets a person try the whole flow without a recorder.
    async recordMockDemonstrations(
      { count = 20, tasks = TASKS, seed = 1, variants = [0, 1, 2] } = {},
      actor,
    ) {
      requireHuman(actor, 'Recording demonstrations');
      const out = [];
      for (let i = 0; i < count; i += 1) {
        const task = tasks[i % tasks.length];
        const variant = variants[i % variants.length];
        const env = createMockEnv(task, { seed: seed + i, variant });
        const expert = createExpert(task);
        const r = rng(`demo:${seed + i}`);
        const cap = await liquid.startCapture({ task, app: APPS[task] }, actor);
        let t = 0;
        let prev = null;
        let dt = 0;
        for (let n = 0; n < 60 && !env.finished; n += 1) {
          const observation = env.observe();
          const action = expert.next(observation, { dt_ms: dt, prev_action: prev });
          const result = env.apply(action, { approval: 'demonstrator' });
          await liquid.captureStep(cap.capture_id, { t_ms: t, observation, action, result });
          prev = action;
          dt = humanDelay(r, action);
          t += dt;
          env.tick(dt);
        }
        const stopped = await liquid.stopCapture(cap.capture_id, actor);
        out.push({
          episode_id: stopped.episode_id,
          task,
          variant,
          success: env.success,
          steps: stopped.steps,
        });
      }
      return { episodes: out };
    },

    // -------------------------------------------------------------- episodes
    // POST /training/episodes: EpisodeV1 from a recorder adapter. Re-masked and scanned here;
    // always lands unreviewed.
    async addEpisodes(episodes, actor) {
      requireHuman(actor, 'Adding episodes');
      await requirePermission('recording', 'Adding recorded episodes');
      if (!Array.isArray(episodes) || !episodes.length)
        throw fail('INVALID_INPUT', 'Send { episodes: [EpisodeV1, ...] }.');
      const out = [];
      for (const raw of episodes) {
        if (raw?.schema_version !== SCHEMA_VERSION)
          throw fail('E_SCHEMA', 'Each episode must be liquid-agent/v1.', 422);
        const episode = toEpisodeV1({
          episode_id: raw.episode_id,
          task: raw.task,
          created_at: raw.created_at,
          split_group: raw.split_group,
          variant: raw.variant,
          consent_id: raw.capture?.consent_id ?? 'api',
          profile: raw.capture?.redaction_profile ?? 'strict',
          source: raw.capture?.source === 'synthetic' ? 'synthetic' : 'import',
          app_scope: raw.capture?.app_scope,
          steps: raw.steps ?? [],
          labels: { ...raw.labels, reviewed: false },
        });
        if (
          await db.get('SELECT 1 FROM liquid_episodes WHERE episode_id = ?', [episode.episode_id])
        )
          throw fail('CONFLICT', `Episode ${episode.episode_id} already exists.`, 409);
        await storeEpisode(episode, { source: 'api', state: 'review_required' });
        out.push(episode.episode_id);
      }
      await audit(actor, 'episodes.add', null, { episodes: out });
      return { episodes: out, state: 'review_required' };
    },
    // A format-0.1 episode from the Training store, converted for review.
    async importTrainingEpisode(trainingEpisodeId, { task } = {}, actor) {
      requireHuman(actor, 'Converting an episode');
      await requirePermission('training_data', 'Converting a recorded episode for training');
      const detail = await training.getEpisode(trainingEpisodeId);
      const { episode, dropped } = fromFormat01(detail, {
        consent_id: `import:${trainingEpisodeId}`.slice(0, 128),
        task,
      });
      if (await db.get('SELECT 1 FROM liquid_episodes WHERE episode_id = ?', [episode.episode_id]))
        throw fail('CONFLICT', `Episode ${trainingEpisodeId} was already converted.`, 409);
      await storeEpisode(episode, { source: 'format-0.1', state: 'review_required' });
      await audit(actor, 'episodes.import', episode.episode_id, {
        from: trainingEpisodeId,
        dropped,
      });
      return { episode_id: episode.episode_id, dropped_steps: dropped, state: 'review_required' };
    },
    async listEpisodes({ state } = {}) {
      const rows = await db.all(
        `SELECT * FROM liquid_episodes ${state ? 'WHERE state = ?' : ''} ORDER BY created_at DESC LIMIT 1000`,
        state ? [state] : [],
      );
      const out = [];
      for (const r of rows) out.push(summary(r, (await unseal('episodes', r.episode_id)).episode));
      return out;
    },
    async getEpisode(id) {
      const { row, episode, redactions, meta: extra } = await loadEpisode(id);
      return {
        ...summary(row, episode),
        episode,
        redactions: redactions.map((r) => ({
          redaction_id: r.redaction_id,
          step: r.step,
          field: r.field,
          at: r.at,
        })),
        ...(extra?.corrections ? { corrections: extra.corrections } : {}),
      };
    },
    async review(id, input, actor) {
      requireHuman(actor, 'Reviewing an episode');
      const { row, episode, redactions, meta: extra } = await loadEpisode(id);
      if (!['review_required', 'reviewed'].includes(row.state))
        throw fail('INVALID_STATE', `Episode ${id} is ${row.state}.`, 409);
      const labels = { ...episode.labels };
      if (input.goal !== undefined) labels.goal = String(input.goal).slice(0, 500);
      if (input.success !== undefined) labels.success = input.success === true;
      if (input.risk !== undefined) {
        if (!['low', 'medium', 'high'].includes(input.risk))
          throw fail('INVALID_INPUT', 'risk is low, medium or high.');
        labels.risk = input.risk;
      }
      if (input.failure_reason !== undefined) {
        if (input.failure_reason)
          labels.failure_reason = String(input.failure_reason).slice(0, 500);
        else delete labels.failure_reason;
      }
      if (input.reviewed !== undefined) labels.reviewed = input.reviewed === true;
      if (labels.success && labels.failure_reason)
        throw fail('INVALID_INPUT', 'A successful episode has no failure_reason.');
      const next = sealEpisode({ ...episode, labels });
      await storeEpisode(next, {
        source: row.source,
        state: labels.reviewed ? 'reviewed' : 'review_required',
        redactions,
        meta: extra,
      });
      await audit(actor, 'episode.review', id, { labels });
      return liquid.getEpisode(id);
    },
    // Reversible redaction: the original value stays sealed on this device only.
    async redact(id, { step, field }, actor) {
      requireHuman(actor, 'Redacting');
      const { row, episode, redactions, meta: extra } = await loadEpisode(id);
      const s = episode.steps[Number(step)];
      if (!s) throw fail('INVALID_INPUT', `Episode ${id} has no step ${step}.`);
      const next = structuredClone(episode);
      const t = next.steps[Number(step)];
      let original;
      const m = /^observation\.elements\.(\d+)\.name$/.exec(field ?? '');
      if (field === 'action.text') {
        if (t.action.text === undefined) throw fail('INVALID_INPUT', 'That step carries no text.');
        original = t.action.text;
        delete t.action.text;
        t.action.text_redacted = true;
      } else if (field === 'action.target.name' && t.action.target) {
        original = t.action.target.name;
        t.action.target.name = '[redacted]';
      } else if (field === 'observation.focus.name' && t.observation.focus) {
        original = t.observation.focus.name;
        t.observation.focus.name = '[redacted]';
      } else if (m && t.observation.elements?.[Number(m[1])]) {
        original = t.observation.elements[Number(m[1])].name;
        t.observation.elements[Number(m[1])].name = '[redacted]';
      } else
        throw fail(
          'INVALID_INPUT',
          'field is action.text, action.target.name, observation.focus.name or observation.elements.N.name.',
        );
      const redaction = {
        redaction_id: newId('red'),
        step: Number(step),
        field,
        original,
        at: iso(),
      };
      await storeEpisode(sealEpisode(next), {
        source: row.source,
        state: row.state,
        redactions: [...redactions, redaction],
        meta: extra,
      });
      await audit(actor, 'episode.redact', id, { step: Number(step), field });
      return { redaction_id: redaction.redaction_id, episode: await liquid.getEpisode(id) };
    },
    async unredact(id, redactionId, actor) {
      requireHuman(actor, 'Undoing a redaction');
      const { row, episode, redactions, meta: extra } = await loadEpisode(id);
      const r = redactions.find((x) => x.redaction_id === redactionId);
      if (!r) throw fail('NOT_FOUND', `No redaction ${redactionId}.`, 404);
      const next = structuredClone(episode);
      const t = next.steps[r.step];
      if (r.field === 'action.text') {
        t.action.text = r.original;
        delete t.action.text_redacted;
      } else if (r.field === 'action.target.name') t.action.target.name = r.original;
      else if (r.field === 'observation.focus.name') t.observation.focus.name = r.original;
      else t.observation.elements[Number(r.field.split('.')[2])].name = r.original;
      await storeEpisode(sealEpisode(next), {
        source: row.source,
        state: row.state,
        redactions: redactions.filter((x) => x !== r),
        meta: extra,
      });
      await audit(actor, 'episode.unredact', id, { redaction_id: redactionId });
      return liquid.getEpisode(id);
    },
    // Marks later copies of the same task trace as duplicates (kept, excluded from datasets).
    async dedupe(actor) {
      const rows = await db.all(
        "SELECT episode_id, fingerprint FROM liquid_episodes WHERE state IN ('review_required', 'reviewed') ORDER BY created_at, episode_id",
      );
      const seen = new Map();
      const marked = [];
      for (const r of rows) {
        if (seen.has(r.fingerprint)) {
          await db.run(
            "UPDATE liquid_episodes SET state = 'duplicate', dup_of = ?, updated_at = ? WHERE episode_id = ?",
            [seen.get(r.fingerprint), iso(), r.episode_id],
          );
          marked.push({ episode_id: r.episode_id, dup_of: seen.get(r.fingerprint) });
        } else seen.set(r.fingerprint, r.episode_id);
      }
      await audit(actor, 'episodes.dedupe', null, { marked: marked.length });
      return { duplicates: marked };
    },
    async discardEpisode(id, actor) {
      requireHuman(actor, 'Discarding an episode');
      await episodeRow(id);
      await db.run(
        "UPDATE liquid_episodes SET state = 'discarded', updated_at = ? WHERE episode_id = ?",
        [iso(), id],
      );
      await audit(actor, 'episode.discard', id, {});
      return { episode_id: id, state: 'discarded' };
    },
    async deleteEpisode(id, actor) {
      requireHuman(actor, 'Deleting an episode');
      await episodeRow(id);
      await blobs.del(`liquid/episodes/${id}.bin`);
      await db.run('DELETE FROM liquid_episodes WHERE episode_id = ?', [id]);
      await audit(actor, 'episode.delete', id, {});
      return { episode_id: id, deleted: true };
    },
    // A correction from an agent run becomes reviewable only when a person accepts it.
    async acceptCorrection(id, actor) {
      requireHuman(actor, 'Accepting a correction as an example');
      const row = await episodeRow(id);
      if (row.state !== 'proposed')
        throw fail(
          'INVALID_STATE',
          `Episode ${id} is ${row.state}, not a proposed correction.`,
          409,
        );
      await db.run(
        "UPDATE liquid_episodes SET state = 'review_required', updated_at = ? WHERE episode_id = ?",
        [iso(), id],
      );
      await audit(actor, 'correction.accept', id, {});
      return { episode_id: id, state: 'review_required' };
    },

    // -------------------------------------------------------------- datasets
    async exportDataset(
      { name, episodes: ids, seed = 0, retention_days = 30, validation = 0.15, test = 0.25 } = {},
      actor,
    ) {
      requireHuman(actor, 'Exporting a dataset');
      await requirePermission('training_data', 'Building a training dataset');
      const rows = ids?.length
        ? await Promise.all(ids.map(episodeRow))
        : await db.all(
            "SELECT * FROM liquid_episodes WHERE state = 'reviewed' ORDER BY created_at, episode_id",
          );
      const bad = rows.filter((r) => r.state !== 'reviewed');
      if (bad.length)
        throw fail(
          'INVALID_STATE',
          'Only reviewed episodes (not duplicates, proposals or discarded) can be exported.',
          409,
          bad.map((r) => ({ episode_id: r.episode_id, reason: r.state })),
        );
      if (!rows.length)
        throw fail('INVALID_STATE', 'No reviewed episodes to export. Review some first.', 409);
      const episodes = [];
      for (const r of rows) episodes.push((await unseal('episodes', r.episode_id)).episode);
      const consent = {
        consent_id: `dc_${newId('x').slice(2)}`,
        purposes: ['model_training', 'evaluation'],
        granted_at: iso(),
      };
      const retention = {
        days: retention_days === null ? null : Math.max(0, Number(retention_days)),
        delete_on_revoke: true,
      };
      const bundle = buildBundle(episodes, {
        name: name ? String(name).slice(0, 160) : 'Teach an agent',
        consent,
        retention,
        provenance: {
          source: 'w3bs',
          generator: 'w3bs-training liquid',
          tool_version: training?.version ?? 'dev',
        },
        strategy: 'by_session',
        seed: Number(seed) || 0,
        validation,
        test,
        createdAt: iso(),
      });
      const datasetId = bundle.manifest.dataset_id;
      let path = null;
      if (dataDir) {
        mkdirSync(join(dataDir, 'datasets'), { recursive: true, mode: 0o700 });
        path = join(dataDir, 'datasets', `${datasetId}.json`);
        writeFileSync(path, JSON.stringify(bundle), { mode: 0o600 });
      }
      const body = {
        dataset_id: datasetId,
        dataset_hash: bundle.manifest.dataset_hash,
        name: bundle.manifest.name,
        episodes: bundle.manifest.episodes.length,
        counts: bundle.manifest.splits.counts,
        tasks: [...new Set(episodes.map((e) => e.task))].sort(),
        consent,
        retention,
        path,
      };
      await db.run(
        'INSERT INTO liquid_datasets (dataset_id, dataset_hash, created_at, actor, body) VALUES (?, ?, ?, ?, ?) ON CONFLICT(dataset_id) DO UPDATE SET body = excluded.body',
        [datasetId, body.dataset_hash, iso(), who(actor), json(body)],
      );
      await audit(actor, 'dataset.export', datasetId, {
        dataset_hash: body.dataset_hash,
        episodes: body.episodes,
        counts: body.counts,
      });
      return body;
    },
    async listDatasets() {
      return (await db.all('SELECT body FROM liquid_datasets ORDER BY created_at DESC')).map((r) =>
        parse(r.body),
      );
    },
    async getDataset(id) {
      const row = await db.get(
        'SELECT body FROM liquid_datasets WHERE dataset_id = ? OR dataset_hash = ?',
        [id, id],
      );
      if (!row) throw fail('NOT_FOUND', `Dataset ${id} was not found.`, 404);
      return parse(row.body);
    },
    async readBundle(id) {
      const ds = await liquid.getDataset(id);
      if (!ds.path || !existsSync(ds.path))
        throw fail(
          'NOT_FOUND',
          `Dataset ${id} has no bundle file (retention may have removed it).`,
          404,
        );
      const bundle = JSON.parse(readFileSync(ds.path, 'utf8'));
      if (verifyBundle(bundle) !== ds.dataset_hash)
        throw fail('E_HASH_MISMATCH', 'The bundle file changed on disk.', 409);
      return { dataset: ds, bundle };
    },

    // -------------------------------------------------------------- training
    // Remote training is two calls: this preview, then train({ preview_id, confirm: true }).
    async previewRemoteTrain({ dataset_id, worker, max_price_usd, public_url }, actor) {
      requireHuman(actor, 'Preparing an upload');
      await requirePermission('training_data', 'Training');
      await requirePermission('network_upload', 'Sending a dataset to a c0mpute worker');
      if (!worker || !/^[A-Za-z0-9._:-]{4,128}$/.test(worker))
        throw fail('INVALID_INPUT', 'Name the trusted worker by its peer id.');
      const cap = Number(max_price_usd);
      if (!(cap >= 0)) throw fail('INVALID_INPUT', 'Set a cost cap in USD (max_price_usd).');
      const ds = await liquid.getDataset(dataset_id);
      const preview = {
        preview_id: newId('pv'),
        dataset_id: ds.dataset_id,
        dataset_hash: ds.dataset_hash,
        episodes: ds.episodes,
        worker,
        max_price_usd: cap,
        public_url: public_url ?? null,
        retention: {
          dataset: 'the worker deletes the dataset when the job ends',
          artifacts_days: ds.retention.days,
        },
        privacy:
          'The worker sees the redacted dataset in plaintext while it trains: encryption in transit does not hide it from the machine doing the work. Only choose a worker you trust.',
        expires_at: new Date(now().getTime() + 15 * 60 * 1000).toISOString(),
      };
      await db.run('INSERT INTO liquid_previews (preview_id, created_at, body) VALUES (?, ?, ?)', [
        preview.preview_id,
        iso(),
        json(preview),
      ]);
      await audit(actor, 'train.preview', ds.dataset_id, {
        worker,
        max_price_usd: cap,
        dataset_hash: ds.dataset_hash,
      });
      return preview;
    },
    async train({ dataset_id, target = 'local', preview_id, confirm = false, epochs } = {}, actor) {
      requireHuman(actor, 'Training');
      await requirePermission('training_data', 'Training');
      if (!['local', 'c0mpute'].includes(target))
        throw fail('INVALID_INPUT', 'target is local or c0mpute.');
      const { dataset, bundle } = await liquid.readBundle(dataset_id);
      let preview = null;
      if (target === 'c0mpute') {
        await requirePermission('network_upload', 'Sending a dataset to a c0mpute worker');
        const row =
          preview_id &&
          (await db.get('SELECT * FROM liquid_previews WHERE preview_id = ?', [preview_id]));
        preview = row && parse(row.body);
        if (!preview || preview.dataset_hash !== dataset.dataset_hash)
          throw fail(
            'PREVIEW_REQUIRED',
            'Remote training needs the upload preview for this dataset first.',
            409,
          );
        if (row.used_at || Date.parse(preview.expires_at) < now().getTime())
          throw fail('PREVIEW_REQUIRED', 'That preview expired or was used. Make a new one.', 409);
        if (confirm !== true)
          throw fail(
            'CONFIRMATION_REQUIRED',
            'Confirm the upload preview to send the dataset.',
            409,
          );
        await db.run('UPDATE liquid_previews SET used_at = ? WHERE preview_id = ?', [
          iso(),
          preview_id,
        ]);
      }
      const argv = requireBin(target === 'local' ? 'infernet' : 'c0mpute', spawnEnv);
      const jobId = newId('tj');
      const outDir = join(dataDir ?? resolve('.'), 'models', jobId);
      mkdirSync(outDir, { recursive: true, mode: 0o700 });
      let bundlePath = dataset.path;
      if (target === 'c0mpute') {
        // The copy that leaves carries remote_training consent; the dataset hash is unchanged.
        const remote = {
          ...bundle,
          manifest: {
            ...bundle.manifest,
            consent: {
              ...bundle.manifest.consent,
              purposes: [...new Set([...bundle.manifest.consent.purposes, 'remote_training'])],
            },
          },
        };
        bundlePath = join(outDir, 'upload-bundle.json');
        writeFileSync(bundlePath, JSON.stringify(remote), { mode: 0o600 });
      }
      const trainJob = assertValid(
        {
          schema_version: SCHEMA_VERSION,
          kind: 'train_job',
          job_id: jobId,
          idempotency_key: hashCanonical({
            dataset: dataset.dataset_hash,
            target,
            worker: preview?.worker ?? 'local',
            at: iso(),
          }).slice(7, 39),
          created_at: iso(),
          dataset_hash: dataset.dataset_hash,
          dataset_ref: bundlePath,
          model_type: 'cfc',
          baselines: ['gru'],
          hyperparameters: {
            hidden_units: 32,
            epochs: Number(epochs) || 30,
            learning_rate: 0.005,
            seed: 0,
          },
          resources: { device: 'auto' },
          limits: {
            timeout_s: 6 * 3600,
            max_retries: 0,
            max_budget_usd: preview?.max_price_usd ?? 0,
          },
          privacy: {
            mode: target === 'local' ? 'local_only' : 'trusted_worker',
            upload_approved: target === 'c0mpute',
            ...(preview ? { worker_allowlist: [preview.worker] } : {}),
          },
          retention: {
            artifact_days: dataset.retention.days,
            delete_dataset_after: target === 'c0mpute',
          },
        },
        'train_job',
      );
      const args =
        target === 'local'
          ? [
              'model',
              'train',
              '--dataset',
              bundlePath,
              '--output',
              outDir,
              '--family',
              'cfc',
              '--baseline',
              'gru',
              '--json',
            ]
          : [
              'infernet',
              'temporal',
              'train',
              '--dataset',
              bundlePath,
              '--output',
              outDir,
              '--family',
              'cfc',
              '--baseline',
              'gru',
              '--worker',
              preview.worker,
              '--max-price',
              String(preview.max_price_usd),
              ...(preview.public_url ? ['--public-url', preview.public_url] : []),
              // --yes only now: the person confirmed the preview above.
              '--yes',
              '--json',
            ];
      if (epochs && target === 'local')
        args.splice(args.length - 1, 0, '--epochs', String(Number(epochs)));
      const job = {
        job_id: jobId,
        dataset_id: dataset.dataset_id,
        dataset_hash: dataset.dataset_hash,
        target,
        state: 'running',
        worker: preview?.worker ?? 'local',
        train_job: trainJob,
        created_at: iso(),
      };
      await db.run(
        'INSERT INTO liquid_jobs (job_id, dataset_id, target, state, created_at, updated_at, actor, body) VALUES (?, ?, ?, ?, ?, ?, ?, ?)',
        [jobId, dataset.dataset_id, target, 'running', iso(), iso(), who(actor), json(job)],
      );
      await audit(actor, 'train.start', jobId, {
        target,
        dataset_hash: dataset.dataset_hash,
        worker: job.worker,
        max_price_usd: preview?.max_price_usd ?? null,
      });
      const finish = async (state, extra) => {
        const body = { ...job, ...extra, state, finished_at: iso() };
        await db.run(
          'UPDATE liquid_jobs SET state = ?, updated_at = ?, body = ? WHERE job_id = ?',
          [state, iso(), json(body), jobId],
        );
        await audit(
          { kind: 'system', id: 'trainer' },
          `train.${state}`,
          jobId,
          state === 'failed'
            ? { error: extra.error }
            : { selected: extra.selected, models: extra.models?.map((m) => m.model_hash) },
        );
        if (target === 'c0mpute') rmSync(bundlePath, { force: true });
        return body;
      };
      const promise = runJson(argv, args, { env: spawnEnv })
        .then(async ({ json: output }) => {
          if (output.dataset_hash !== dataset.dataset_hash)
            throw fail(
              'E_HASH_MISMATCH',
              `the trainer reports dataset ${output.dataset_hash}, not ${dataset.dataset_hash}`,
              502,
            );
          const models = await registerModels(
            jobId,
            output,
            target === 'local' ? 'local' : `c0mpute:${preview.worker}`,
            output.receipts ?? [],
          );
          return finish('succeeded', {
            selected: output.selected,
            recommendation:
              output.selected === 'gru'
                ? 'GRU (baseline) won: the liquid family did not beat it, so W3BS recommends the baseline.'
                : `${output.selected.toUpperCase()} beat the GRU baseline on episode success.`,
            models: models.map((m) => ({
              model_id: m.model_id,
              model_hash: m.model_hash,
              family: m.family,
              metrics: m.metrics,
              selected: m.selected,
              source: m.source,
            })),
            receipts: output.receipts ?? [],
          });
        })
        .catch((error) =>
          finish('failed', { error: { code: error.code ?? 'E_RUNTIME', message: error.message } }),
        )
        .finally(() => live.jobs.delete(jobId));
      live.jobs.set(jobId, promise);
      return job;
    },
    async waitJob(jobId) {
      await live.jobs.get(jobId);
      return liquid.getJob(jobId);
    },
    async getJob(jobId) {
      const row = await db.get('SELECT body FROM liquid_jobs WHERE job_id = ?', [jobId]);
      if (!row) throw fail('NOT_FOUND', `No training job ${jobId}.`, 404);
      return parse(row.body);
    },
    async listJobs() {
      return (await db.all('SELECT body FROM liquid_jobs ORDER BY created_at DESC LIMIT 100')).map(
        (r) => parse(r.body),
      );
    },
    async listModels() {
      const out = [];
      for (const r of await db.all('SELECT * FROM liquid_models ORDER BY created_at DESC')) {
        const body = parse(r.body);
        const ev = await passingEval(r.model_hash);
        out.push({ ...body, evaluated: !!ev, eval_pass: !!ev });
      }
      return out;
    },

    // -------------------------------------------------------------- evaluation
    // Closed loop on the mock sites through the guarded executor, Bounded mode, with a
    // scripted careful person (the expert) who answers approvals and takes over when the
    // policy abstains or fails a check. Seen layouts and the renamed layout are reported apart.
    async evaluate(
      {
        model_id,
        per_workflow = 5,
        variants = [0, 1, 2],
        layout_variant = 3,
        seed = 100,
        tasks,
        thresholds = {},
      } = {},
      actor,
    ) {
      requireHuman(actor, 'Evaluating a model');
      const model = await modelRow(model_id);
      const limits = { ...DEFAULT_THRESHOLDS, ...thresholds };
      const runtime = new PolicyRuntime({ env: spawnEnv });
      live.runtimes.add(runtime);
      const workflows =
        tasks ??
        (model.body.tasks.length ? model.body.tasks.filter((t) => TASKS.includes(t)) : TASKS);
      try {
        await runtime.load(model.body.model_dir, model.model_hash);
        const policy = runtimePolicy(runtime, model.model_hash, { prefix: 'eval' });
        const runOne = async (task, variant, s) => {
          const env = createMockEnv(task, { seed: s, variant });
          // The careful person's own next action, worked out once per step from what they see.
          const expert = createExpert(task);
          let prev = null;
          let lastDt = 0;
          let step = 0;
          let cached = { step: -1, action: null };
          const reference = (observation) => {
            if (cached.step !== step)
              cached = {
                step,
                action: expert.next(observation, { dt_ms: lastDt, prev_action: prev }),
              };
            return cached.action;
          };
          return runPolicy({
            env,
            policy,
            task,
            mode: 'bounded',
            gate: {
              killSwitch,
              activeModelHash: async () => model.model_hash,
              permitted: async () => true,
            },
            approve: async ({ action, observation }) => ({
              approved: canonicalize(reference(observation)) === canonicalize(action),
              by: 'evaluator',
            }),
            human: async (observation) => reference(observation),
            // The person watches every step, so their sense of elapsed time stays current.
            onObserve: (observation) => reference(observation),
            audit: async () => {},
            dtFor: (a) => {
              prev = a;
              step += 1;
              lastDt = a?.type === 'wait' ? 1100 : a?.type === 'type' ? 2500 : 700;
              return lastDt;
            },
          });
        };
        const summarise = (reports) => {
          const n = reports.length;
          const lat = reports.flatMap((r) => r.latency_ms);
          const failures = {};
          for (const r of reports)
            for (const [k, v] of Object.entries(r.failures)) failures[k] = (failures[k] ?? 0) + v;
          return {
            episodes: n,
            task_success: n ? reports.filter((r) => r.success).length / n : 0,
            autonomous_success: n ? reports.filter((r) => r.autonomous).length / n : 0,
            interventions: reports.reduce((a, r) => a + r.interventions, 0),
            interventions_per_episode: n ? reports.reduce((a, r) => a + r.interventions, 0) / n : 0,
            approvals_requested: reports.reduce((a, r) => a + r.approvals_requested, 0),
            external_writes_without_approval: reports.reduce(
              (a, r) => a + r.executed.filter((x) => x.result === 'blocked').length,
              0,
            ),
            latency_ms_p50: percentile(lat, 50),
            latency_ms_p95: percentile(lat, 95),
            failure_modes: failures,
          };
        };
        const report = {
          model_id: model.model_id,
          model_hash: model.model_hash,
          family: model.family,
          workflows: {},
          layout_variant: {},
          thresholds: limits,
        };
        for (const task of workflows) {
          const seen = [];
          for (let i = 0; i < per_workflow; i += 1)
            seen.push(await runOne(task, variants[i % variants.length], seed + i));
          const shifted = [];
          for (let i = 0; i < Math.max(1, Math.ceil(per_workflow / 2)); i += 1)
            shifted.push(await runOne(task, layout_variant, seed + 1000 + i));
          report.workflows[task] = summarise(seen);
          report.layout_variant[task] = summarise(shifted);
        }
        const gaps = [];
        for (const task of workflows) {
          if (report.workflows[task].autonomous_success < limits.autonomous_success)
            gaps.push(
              `${task}: autonomous success ${report.workflows[task].autonomous_success.toFixed(2)} < ${limits.autonomous_success}`,
            );
          if (report.layout_variant[task].autonomous_success < limits.layout_autonomous_success)
            gaps.push(
              `${task} (changed layout): ${report.layout_variant[task].autonomous_success.toFixed(2)} < ${limits.layout_autonomous_success}; record more varied demonstrations`,
            );
          if (report.workflows[task].external_writes_without_approval)
            gaps.push(`${task}: an external write reached the site without approval`);
        }
        report.pass = gaps.length === 0;
        report.gaps = gaps;
        report.note = 'Measured on mock sites only. Training loss alone never makes a model ready.';
        const evalId = newId('ev');
        await db.run(
          'INSERT INTO liquid_evals (eval_id, model_id, model_hash, pass, created_at, body) VALUES (?, ?, ?, ?, ?, ?)',
          [evalId, model.model_id, model.model_hash, report.pass ? 1 : 0, iso(), json(report)],
        );
        await audit(actor, 'model.evaluate', model.model_id, {
          eval_id: evalId,
          pass: report.pass,
          gaps,
        });
        return { eval_id: evalId, ...report };
      } finally {
        live.runtimes.delete(runtime);
        await runtime.close();
      }
    },
    async listEvaluations(modelId) {
      return (
        await db.all(
          `SELECT body, eval_id, created_at FROM liquid_evals ${modelId ? 'WHERE model_id = ?' : ''} ORDER BY created_at DESC LIMIT 100`,
          modelId ? [modelId] : [],
        )
      ).map((r) => ({
        eval_id: r.eval_id,
        created_at: r.created_at,
        ...parse(r.body),
      }));
    },

    // -------------------------------------------------------------- activation and modes
    async activate(modelId, { tasks } = {}, actor) {
      requireHuman(actor, 'Activating a model');
      const model = await modelRow(modelId);
      if (!(await passingEval(model.model_hash)))
        throw fail(
          'EVALUATION_REQUIRED',
          'Activate a model only after it passes an evaluation on the mock sites.',
          409,
        );
      const list = tasks?.length ? tasks : model.body.tasks;
      if (!list.length) throw fail('INVALID_INPUT', 'Name the tasks to activate this model for.');
      for (const task of list) {
        if (model.body.tasks.length && !model.body.tasks.includes(task))
          throw fail('INVALID_INPUT', `Model ${model.model_id} was not trained on ${task}.`);
        await db.run(
          'INSERT INTO liquid_activations (task, model_id, model_hash, activated_at, actor) VALUES (?, ?, ?, ?, ?) ON CONFLICT(task) DO UPDATE SET model_id = excluded.model_id, model_hash = excluded.model_hash, activated_at = excluded.activated_at, actor = excluded.actor',
          [task, model.model_id, model.model_hash, iso(), who(actor)],
        );
      }
      await audit(actor, 'model.activate', model.model_id, {
        tasks: list,
        model_hash: model.model_hash,
        source: model.body.source,
      });
      return {
        model_id: model.model_id,
        model_hash: model.model_hash,
        tasks: list,
        source: model.body.source,
      };
    },
    async deactivate(task, actor) {
      await db.run('DELETE FROM liquid_activations WHERE task = ?', [task]);
      await db.run(
        "UPDATE liquid_policies SET mode = 'off', bounds = NULL, updated_at = ?, actor = ? WHERE task = ?",
        [iso(), who(actor), task],
      );
      await audit(actor, 'model.deactivate', task, {});
      return { task, active: null, mode: 'off' };
    },
    // Can this task use a policy? Suggest is recommended only when every check passes.
    async capability(task) {
      const active = await activeFor(task);
      const reasons = [];
      if (!active) reasons.push('no model is activated for this task');
      const model = active && (await modelRow(active.model_id).catch(() => null));
      if (active && !(await passingEval(active.model_hash)))
        reasons.push('the active model has no passing evaluation');
      if (model && !existsSync(join(model.body.model_dir, 'manifest.json')))
        reasons.push('the model files are missing');
      if (!resolveBin('infernet', spawnEnv)) reasons.push('infernet is not installed');
      return {
        task,
        active_model: active
          ? {
              model_id: active.model_id,
              model_hash: active.model_hash,
              source: model?.body.source ?? null,
              family: model?.family ?? null,
            }
          : null,
        checks_pass: reasons.length === 0,
        reasons,
        recommended_mode: reasons.length === 0 ? 'suggest' : 'off',
        mode: (await policyFor(task)).mode,
      };
    },
    async setMode(task, mode, actor, { bounds } = {}) {
      if (!MODES.includes(mode)) throw fail('INVALID_INPUT', `mode is one of ${MODES.join(', ')}.`);
      if (mode !== 'off') {
        requireHuman(actor, `Turning the policy to ${mode}`);
        const cap = await liquid.capability(task);
        if (!cap.checks_pass)
          throw fail(
            'CAPABILITY_CHECK_FAILED',
            `This task cannot use a policy yet: ${cap.reasons.join('; ')}.`,
            409,
            cap.reasons,
          );
        if (mode === 'approve' || mode === 'bounded')
          await requirePermission('auto_execute', 'Letting the agent act');
        if (mode === 'bounded') {
          if (
            !bounds ||
            !Number.isInteger(bounds.max_steps) ||
            bounds.max_steps < 1 ||
            bounds.max_steps > 100
          )
            throw fail(
              'INVALID_INPUT',
              'Bounded automation needs explicit bounds: { max_steps (1-100), action_types }.',
            );
          if (
            bounds.action_types?.some((t) =>
              ['submit', 'send', 'post', 'publish', 'delete'].includes(t),
            )
          )
            throw fail(
              'INVALID_INPUT',
              'External writes cannot be inside the bounds; they always need your approval.',
            );
        }
      }
      await db.run(
        'INSERT INTO liquid_policies (task, mode, bounds, updated_at, actor) VALUES (?, ?, ?, ?, ?) ON CONFLICT(task) DO UPDATE SET mode = excluded.mode, bounds = excluded.bounds, updated_at = excluded.updated_at, actor = excluded.actor',
        [task, mode, mode === 'bounded' ? json(bounds) : null, iso(), who(actor)],
      );
      await audit(actor, 'policy.mode', task, { mode, bounds: mode === 'bounded' ? bounds : null });
      return policyFor(task);
    },
    policyFor,

    // -------------------------------------------------------------- agent runs (mock sites only)
    // `approver` / `person` are for tests and the CLI; the local app answers through asks.
    async startRun(
      { task, seed = 1, variant = 0, account = 'mock-a', faults = {} } = {},
      actor,
      { approver, person } = {},
    ) {
      requireHuman(actor, 'Starting an agent run');
      if (!TASKS.includes(task))
        throw fail('INVALID_INPUT', `The agent runs only on the mock sites: ${TASKS.join(', ')}.`);
      const policyRow = await policyFor(task);
      if (policyRow.mode === 'off')
        throw fail(
          'POLICY_OFF',
          'The policy is Off for this task. Turn on Suggest (or more) first.',
          409,
        );
      const active = await activeFor(task);
      if (!active) throw fail('NO_ACTIVE_MODEL', 'No model is activated for this task.', 409);
      if (await killSwitch()) throw fail('KILL_SWITCH', 'The kill switch is on.', 409);
      const model = await modelRow(active.model_id);
      const runId = newId('run');
      const env = createMockEnv(task, { seed, variant, account });
      const runtime = new PolicyRuntime({ env: spawnEnv });
      live.runtimes.add(runtime);
      const body = {
        run_id: runId,
        task,
        mode: policyRow.mode,
        model_id: model.model_id,
        model_hash: model.model_hash,
        mock: { seed, variant },
        state: 'running',
        started_at: iso(),
      };
      await db.run(
        'INSERT INTO liquid_runs (run_id, task, mode, state, created_at, updated_at, body) VALUES (?, ?, ?, ?, ?, ?, ?)',
        [runId, task, policyRow.mode, 'running', iso(), iso(), json(body)],
      );
      await audit(actor, 'run.start', runId, {
        task,
        mode: policyRow.mode,
        model_hash: model.model_hash,
      });
      const askApproval =
        approver ??
        (async (q) => {
          const a = await ask(runId, 'approval', {
            task,
            sequence_index: q.sequence_index,
            action: q.action,
            external: q.external,
            confidence: q.proposal.confidence,
            observation: q.observation,
          });
          return { approved: a.approved === true, by: a.by ?? null, approval_id: a.ask_id };
        });
      const askPerson =
        person ??
        (async (observation, proposal) => {
          const a = await ask(runId, policyRow.mode === 'suggest' ? 'suggestion' : 'takeover', {
            task,
            observation,
            proposal: proposal?.action ?? null,
            abstain_reason: proposal?.abstain_reason ?? null,
          });
          if (a.stop || a.expired) return null;
          return a.accept ? proposal.action : (a.action ?? null);
        });
      const runAudit = async (kind, detail) =>
        audit(
          kind.startsWith('action.by_person') || kind.startsWith('approval.')
            ? 'human:person'
            : `agent:${model.model_id}`,
          kind,
          runId,
          detail,
        );
      const promise = (async () => {
        try {
          await runtime.load(model.body.model_dir, model.model_hash);
          if (faults.switchAccountAt !== undefined) {
            const apply = env.apply;
            let n = 0;
            env.apply = (...args) => {
              const r = apply(...args);
              if (++n === faults.switchAccountAt) env.switchAccount(`${account}-other`);
              return r;
            };
          }
          if (faults.killAfter !== undefined) {
            const apply = env.apply;
            let n = 0;
            env.apply = (...args) => {
              const r = apply(...args);
              if (++n === faults.killAfter)
                liquid.setKillSwitch(true, { kind: 'system', id: 'fault-injection' });
              return r;
            };
          }
          const report = await runPolicy({
            env,
            policy: runtimePolicy(runtime, model.model_hash, { prefix: runId }),
            task,
            mode: policyRow.mode,
            bounds: policyRow.bounds ?? {},
            maxSteps: policyRow.bounds?.max_steps ?? 40,
            gate: {
              killSwitch,
              activeModelHash: async () => (await activeFor(task))?.model_hash ?? null,
              permitted: async (mode) =>
                (mode === 'suggest' ? true : await permissionState('auto_execute')) &&
                (await policyFor(task)).mode !== 'off',
            },
            approve: askApproval,
            human: askPerson,
            audit: runAudit,
          });
          let correctionEpisode = null;
          if (report.corrections.length && report.trace.length) {
            // A correction is a new, proposed example: never added to a dataset or uploaded
            // until a person accepts and reviews it.
            const episode = toEpisodeV1({
              episode_id: `ep_${runId.slice(4)}`,
              task,
              created_at: body.started_at,
              split_group: runId,
              consent_id: `run:${runId}`,
              source: 'w3bs-desktop',
              app_scope: [APPS[task]],
              steps: report.trace,
              labels: {
                reviewed: false,
                success: report.success,
                risk: RISK[task],
                goal: GOALS[task],
              },
            });
            await storeEpisode(episode, {
              source: 'correction',
              state: 'proposed',
              meta: { corrections: report.corrections.length, run_id: runId },
            });
            correctionEpisode = episode.episode_id;
          }
          const { trace, corrections, ...rest } = report;
          const done = {
            ...body,
            state: 'finished',
            finished_at: iso(),
            report: { ...rest, corrections: corrections.length },
            correction_episode: correctionEpisode,
            external_writes: env.writes,
          };
          await db.run(
            'UPDATE liquid_runs SET state = ?, updated_at = ?, body = ? WHERE run_id = ?',
            ['finished', iso(), json(done), runId],
          );
          await audit({ kind: 'system', id: 'executor' }, 'run.finished', runId, {
            success: report.success,
            stopped_by: report.stopped_by,
            executed: report.executed.length,
          });
          return done;
        } catch (error) {
          const done = {
            ...body,
            state: 'failed',
            finished_at: iso(),
            error: { code: error.code ?? 'FAILED', message: error.message },
          };
          await db.run(
            'UPDATE liquid_runs SET state = ?, updated_at = ?, body = ? WHERE run_id = ?',
            ['failed', iso(), json(done), runId],
          );
          await audit({ kind: 'system', id: 'executor' }, 'run.failed', runId, done.error);
          return done;
        } finally {
          live.runtimes.delete(runtime);
          await runtime.close();
          for (const row of await db.all(
            "SELECT ask_id FROM liquid_asks WHERE run_id = ? AND state = 'pending'",
            [runId],
          ))
            await db.run(
              "UPDATE liquid_asks SET state = 'cancelled', decided_at = ? WHERE ask_id = ?",
              [iso(), row.ask_id],
            );
          live.runs.delete(runId);
        }
      })();
      live.runs.set(runId, promise);
      return body;
    },
    async waitRun(runId) {
      await live.runs.get(runId);
      return liquid.getRun(runId);
    },
    async getRun(runId) {
      const row = await db.get('SELECT body FROM liquid_runs WHERE run_id = ?', [runId]);
      if (!row) throw fail('NOT_FOUND', `No run ${runId}.`, 404);
      return parse(row.body);
    },
    async listRuns() {
      return (await db.all('SELECT body FROM liquid_runs ORDER BY created_at DESC LIMIT 100')).map(
        (r) => parse(r.body),
      );
    },
    async listAsks({ state = 'pending' } = {}) {
      return (
        await db.all('SELECT * FROM liquid_asks WHERE state = ? ORDER BY created_at', [state])
      ).map((r) => ({
        ask_id: r.ask_id,
        run_id: r.run_id,
        kind: r.kind,
        state: r.state,
        created_at: r.created_at,
        ...parse(r.body),
      }));
    },
    // A person answers: { approve: true|false } for approvals; { accept: true } | { action } |
    // { stop: true } for suggestions and take-overs.
    async answerAsk(askId, answer, actor) {
      requireHuman(actor, 'Answering the agent');
      const row = await db.get('SELECT * FROM liquid_asks WHERE ask_id = ?', [askId]);
      if (!row) throw fail('NOT_FOUND', `No request ${askId}.`, 404);
      if (row.state !== 'pending')
        throw fail('INVALID_STATE', `Request ${askId} is ${row.state}.`, 409);
      const value =
        row.kind === 'approval'
          ? { approved: answer?.approve === true, by: who(actor), ask_id: askId }
          : answer?.stop
            ? { stop: true }
            : answer?.accept
              ? { accept: true }
              : { action: answer?.action ?? null };
      await db.run(
        'UPDATE liquid_asks SET state = ?, decided_at = ?, actor = ?, answer = ? WHERE ask_id = ?',
        [
          row.kind === 'approval' ? (value.approved ? 'approved' : 'denied') : 'answered',
          iso(),
          who(actor),
          json(value),
          askId,
        ],
      );
      live.waiters.get(askId)?.(value);
      return { ask_id: askId, ...value };
    },

    async auditLog({ limit = 500, kind } = {}) {
      return (
        await db.all(
          `SELECT * FROM liquid_audit ${kind ? 'WHERE kind LIKE ?' : ''} ORDER BY sequence DESC LIMIT ?`,
          kind ? [`${kind}%`, Math.min(limit, 5000)] : [Math.min(limit, 5000)],
        )
      ).map((r) => ({ ...r, detail: parse(r.detail) }));
    },
    async close() {
      await denyAllPending('closing');
      await Promise.allSettled([...live.runs.values()]);
      for (const rt of live.runtimes) await rt.close();
    },
  };
  return liquid;
}
