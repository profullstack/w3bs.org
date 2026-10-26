#!/usr/bin/env node
// A stand-in for the Infernet CLI and c0mpute in unit tests. It speaks the same command lines
// and JSON shapes W3BS uses:
//   model train --dataset B --output O --family cfc --baseline gru --json [--epochs N]
//   infernet temporal train ... --worker P --max-price X [--public-url U] [--yes] --json  (c0mpute)
//   model serve --stdio   (NDJSON runtime contract: load, open_session, predict, reset,
//                          close_session, kill_switch, health, shutdown)
// The "model" is the mock sites' expert demonstrator; the weights file is a small JSON blob
// hashed like a real safetensors file. Behaviour knobs (env):
//   FAKE_SELECT=gru|cfc        which family "wins" (default cfc)
//   FAKE_POLICY=expert|reckless|abstain   reckless proposes Send/Post at once, never abstains
//   FAKE_FAULT=dup_seq         the runtime echoes a stale sequence index once
//   FAKE_TRAIN_FAIL=1          training exits 1
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { createExpert } from '../training/liquid/mock.mjs';
import { hashWithout } from '../training/liquid/canonical.mjs';
import { verifyBundle } from '../training/liquid/dataset.mjs';

const argv = process.argv.slice(2);
const flag = (name) => {
  const i = argv.indexOf(`--${name}`);
  return i >= 0 ? argv[i + 1] : undefined;
};
const has = (name) => argv.includes(`--${name}`);
const sha = (b) => `sha256:${createHash('sha256').update(b).digest('hex')}`;
const NOW = '2026-10-26T12:00:00Z';

function trainOne(bundle, dhash, family, dir) {
  mkdirSync(dir, { recursive: true });
  const weights = Buffer.from(
    JSON.stringify({
      fake: true,
      family,
      policy: process.env.FAKE_POLICY ?? 'expert',
      dataset: dhash,
    }),
  );
  writeFileSync(join(dir, 'model.safetensors'), weights);
  const winner = (process.env.FAKE_SELECT ?? 'cfc') === family;
  const metrics = {
    action_accuracy: winner ? 0.97 : 0.9,
    target_accuracy: winner ? 0.96 : 0.88,
    step_accuracy: winner ? 0.95 : 0.86,
    episode_success: winner ? 0.92 : 0.8,
    latency_ms_p50: 0.3,
  };
  const manifest = {
    schema_version: 'liquid-agent/v1',
    kind: 'model_manifest',
    model_id: `lm_${sha(weights).slice(7, 23)}`,
    created_at: NOW,
    architecture: { family, hidden_units: 32, feature_dim: 256, params: 12345 },
    weights: {
      format: 'safetensors',
      file: 'model.safetensors',
      weights_hash: sha(weights),
      size_bytes: weights.length,
    },
    runtime: { name: 'infernet-liquid', version: '0.1.0', lock_hash: sha('fake-lock') },
    input_schema: {
      encoding: 'hashed-bag/v1',
      feature_dim: 256,
      time_delta: 'log1p_seconds',
      tasks: [...new Set(bundle.episodes.map((e) => e.task))].sort(),
    },
    output_schema: {
      action_types: ['click', 'type', 'scroll', 'wait', 'post', 'done'],
      targets: ['pointer'],
    },
    training: {
      dataset_hash: dhash,
      seed: 0,
      epochs: Number(flag('epochs') ?? 3),
      device: 'cpu',
      started_at: NOW,
      finished_at: NOW,
    },
    eval: { split: 'test', episodes: bundle.manifest.splits.counts.test, metrics },
    license: 'UNLICENSED-private',
  };
  manifest.manifest_hash = hashWithout(manifest, ['manifest_hash', 'signature']);
  writeFileSync(join(dir, 'manifest.json'), JSON.stringify(manifest, null, 2));
  return {
    family,
    model_dir: dir,
    model_hash: manifest.weights.weights_hash,
    eval: {
      model_hash: manifest.weights.weights_hash,
      dataset_hash: dhash,
      split: 'test',
      metrics,
    },
  };
}

function train({ remote }) {
  if (process.env.FAKE_TRAIN_FAIL) {
    process.stderr.write('training failed: injected fault\n');
    process.exit(1);
  }
  const bundle = JSON.parse(readFileSync(flag('dataset'), 'utf8'));
  const dhash = verifyBundle(bundle);
  if (remote && !has('yes')) {
    process.stdout.write(
      `${JSON.stringify({ preview: { dataset_hash: dhash, worker: flag('worker'), max_price_usd: Number(flag('max-price')) } })}\n`,
    );
    process.exit(3);
  }
  const out = resolve(flag('output'));
  const families = [
    flag('family') ?? 'cfc',
    ...(flag('baseline') ?? '').split(',').filter(Boolean),
  ];
  const results = families.map((f) =>
    trainOne(bundle, dhash, f, families.length > 1 ? join(out, f) : out),
  );
  const want = process.env.FAKE_SELECT ?? 'cfc';
  const selected = results.some((r) => r.family === want) ? want : results[0].family;
  const doc = { dataset_hash: dhash, results, selected };
  if (remote)
    doc.receipts = results.map((r, i) => ({
      job_id: `job-${i}`,
      worker_peer_id: flag('worker'),
      output_hash: r.model_hash,
      status: 'completed',
    }));
  process.stdout.write(`${JSON.stringify(doc, null, 2)}\n`);
}

function serve() {
  const models = new Map();
  const sessions = new Map();
  let kill = false;
  let faultUsed = false;
  const send = (o) => process.stdout.write(`${JSON.stringify({ v: 1, ...o })}\n`);
  const err = (code, message) => Object.assign(new Error(message), { code });
  const ops = {
    health: () => ({ status: 'ok', protocol: 1, kill_switch: kill }),
    load(p) {
      const manifest = JSON.parse(readFileSync(join(p.model_dir, 'manifest.json'), 'utf8'));
      const weights = readFileSync(join(p.model_dir, manifest.weights.file));
      const hash = sha(weights);
      if (hash !== manifest.weights.weights_hash)
        throw err('E_HASH_MISMATCH', 'weights do not match the manifest');
      if (p.expected_hash && p.expected_hash !== hash)
        throw err('E_MODEL_HASH', `expected ${p.expected_hash}, file is ${hash}`);
      models.set(hash, JSON.parse(weights.toString()));
      return { model_hash: hash, manifest };
    },
    open_session(p) {
      if (!models.has(p.model_hash)) throw err('E_NOT_LOADED', 'model not loaded');
      sessions.set(p.session_id, {
        model: p.model_hash,
        scope: p.account_scope ?? null,
        seq: -1,
        experts: new Map(),
      });
      return { session_id: p.session_id, sequence_index: -1, state: 'active' };
    },
    predict(p) {
      const req = p.request;
      const s = sessions.get(req.session_id);
      if (!s || (s.scope && p.account_scope && s.scope !== p.account_scope))
        throw err('E_SESSION_UNKNOWN', 'no such session');
      if (req.model_hash && req.model_hash !== s.model) throw err('E_MODEL_HASH', 'model mismatch');
      const idx = req.sequence_index;
      if (idx === s.seq) throw err('E_SEQ_DUPLICATE', `sequence_index ${idx} already processed`);
      if (idx < s.seq) throw err('E_SEQ_LATE', 'late');
      if (idx > s.seq + 1) throw err('E_SEQ_GAP', 'gap');
      s.seq = idx;
      const policy = models.get(s.model).policy;
      const expert = s.experts.get(req.task) ?? createExpert(req.task);
      s.experts.set(req.task, expert);
      let action = expert.next(req.observation, { dt_ms: req.dt_ms, prev_action: req.prev_action });
      let abstain = null;
      if (policy === 'abstain') {
        action = null;
        abstain = 'low_confidence';
      }
      if (policy === 'reckless') {
        const risky = (req.observation.elements ?? []).find((e) =>
          /^(send|post|publish|submit post)$/i.test(e.name),
        );
        if (risky)
          action = {
            type: /send/i.test(risky.name) ? 'send' : 'post',
            target: { role: risky.role, name: risky.name },
          };
      }
      if (kill) {
        action = null;
        abstain = 'kill_switch';
      }
      let seqOut = idx;
      if (process.env.FAKE_FAULT === 'dup_seq' && idx === 2 && !faultUsed) {
        faultUsed = true;
        seqOut = idx - 1;
      }
      return {
        schema_version: 'liquid-agent/v1',
        kind: 'action_proposal',
        session_id: req.session_id,
        sequence_index: seqOut,
        model_hash: s.model,
        action,
        confidence: action ? 0.93 : 0.2,
        uncertainty: 0.1,
        requires_approval:
          !!action && ['submit', 'send', 'post', 'publish', 'delete'].includes(action.type),
        abstain_reason: abstain,
        latency_ms: 0.4,
      };
    },
    reset(p) {
      const s = sessions.get(p.session_id);
      if (!s) throw err('E_SESSION_UNKNOWN', 'no such session');
      s.seq = -1;
      s.experts.clear();
      return { session_id: p.session_id, sequence_index: -1 };
    },
    close_session(p) {
      if (!sessions.delete(p.session_id)) throw err('E_SESSION_UNKNOWN', 'no such session');
      return { session_id: p.session_id, state: 'closed' };
    },
    kill_switch(p) {
      kill = p.on !== false;
      return { kill_switch: kill };
    },
    shutdown() {
      setImmediate(() => process.exit(0));
      return { bye: true };
    },
  };
  createInterface({ input: process.stdin }).on('line', (line) => {
    if (!line.trim()) return;
    const req = JSON.parse(line);
    try {
      const fn = ops[req.op];
      if (!fn) throw err('E_BAD_REQUEST', `unknown op ${req.op}`);
      send({ id: req.id, ok: true, result: fn(req) });
    } catch (e) {
      send({
        id: req.id,
        ok: false,
        error: { code: e.code ?? 'E_RUNTIME', message: e.message, retriable: false },
      });
    }
  });
}

if (argv[0] === 'model' && argv[1] === 'train') train({ remote: false });
else if (argv[0] === 'infernet' && argv[1] === 'temporal' && argv[2] === 'train')
  train({ remote: true });
else if (argv[0] === 'model' && argv[1] === 'serve') serve();
else {
  process.stderr.write(`fake infernet: unsupported ${argv.join(' ')}\n`);
  process.exit(2);
}
