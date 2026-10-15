// Builds synthetic demonstration bundles for tests, docs and the tutorial.
// Every value here is invented; no real capture is included.
import { createHash } from 'node:crypto';
import { writeZip } from '../core/zip.mjs';

export const PNG_1X1 = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==',
  'base64',
);
const sha = (b) => createHash('sha256').update(b).digest('hex');
const V = '0.1';

// One session, one episode: draft a release announcement in a browser and an
// editor. `variant` changes ids so several bundles can live in one store.
export function sampleRecords({
  variant = 'a',
  secret = false,
  goal = 'Prepare a release-announcement draft',
  family = 'release-announcement',
  start = '2026-10-15T16:00:00.000Z',
} = {}) {
  const ses = `ses_${variant}1`;
  const frames = [0, 1, 2, 3].map((n) => ({
    artifact: {
      schema_version: V,
      artifact_id: `art_${variant}${n}`,
      path: `artifacts/frame-${n}.png`,
      media_type: 'image/png',
      sha256: sha(PNG_1X1),
      bytes: PNG_1X1.length,
    },
    observation: {
      schema_version: V,
      observation_id: `obs_${variant}${n}`,
      session_id: ses,
      monotonic_ns: String(1000000000 * (n * 5 + 1) - 50000000),
      kind: 'keyframe',
      artifact_id: `art_${variant}${n}`,
      semantic: {
        app: n < 2 ? 'Firefox' : 'Text Editor',
        window: n < 2 ? 'Releases · w3bs.org' : 'announcement.md',
      },
      display: { width: 1920, height: 1080, scale: 1 },
      pre_action_verified: true,
    },
  }));
  const step = (n, type, target, args = {}, extra = {}) => ({
    schema_version: V,
    step_id: `step_${variant}${n}`,
    session_id: ses,
    sequence: n,
    observation_before: `obs_${variant}${n}`,
    observation_after: `obs_${variant}${n + 1}`,
    action: {
      type,
      origin: 'human',
      source: 'device_event',
      status: 'confirmed',
      target: { ...target, source: 'accessibility' },
      coordinate_space: 'display',
      arguments: args,
      monotonic_ns: String(1000000000 * (n * 5 + 1)),
    },
    alignment: {
      status: 'verified',
      evidence: [`event_${n}`, `obs_${variant}${n}`, `obs_${variant}${n + 1}`],
    },
    ...extra,
  });
  const steps = [
    step(
      0,
      'click',
      { app: 'Firefox', role: 'link', name: 'Releases' },
      { x: 120, y: 300, button: 'left' },
    ),
    step(
      1,
      'key_shortcut',
      { app: 'Firefox', role: 'document', name: 'Releases' },
      { keys: ['Control', 'c'] },
    ),
    step(
      2,
      'text_edit',
      {
        app: 'Text Editor',
        role: 'text',
        name: secret ? 'token=ghp_abcdefghijklmnopqrstuvwxyz0123456789' : 'announcement.md',
      },
      { text: 'v0.3.0 is out', text_placeholder: '{{version_line}}' },
    ),
  ];
  return {
    manifest: {
      format: 'w3bs-demonstration-dataset',
      schema_version: V,
      bundle_id: `bun_${variant}`,
      created_at: start,
      producer: { name: 'w3bs-training-fixtures', version: '0.1.0' },
      capture_profile: 'action-supervision',
      counts: { sessions: 1, episodes: 1, observations: 4, actions: 3, labels: 0, artifacts: 4 },
      privacy: { text_values_included: false, redaction_applied: false },
    },
    sessions: [
      {
        schema_version: V,
        session_id: ses,
        capture_profile: 'action-supervision',
        started_at: start,
        clock: { wall_clock_anchor: start, monotonic_anchor_ns: '0' },
        device: {
          os: 'linux',
          desktop: 'KDE Plasma 6.7',
          displays: [{ id: 'DP-1', x: 0, y: 0, width: 1920, height: 1080, scale: 1 }],
        },
        scope: {
          applications: ['Firefox', 'Text Editor'],
          origins: ['https://w3bs.org'],
          text_values: false,
        },
        adapters: [{ name: 'fixture', version: '0.1' }],
        gaps: [],
      },
    ],
    episodes: [
      {
        schema_version: V,
        episode_id: `ep_${variant}1`,
        goal: { value: goal, source: 'deterministic_rule', status: 'proposed' },
        segments: [{ session_id: ses, from_ns: '1000000000', to_ns: '11000000000' }],
        steps: steps.map((s) => s.step_id),
        parameters: [{ name: 'version_line', type: 'string', placeholder: 'release version line' }],
        outcome: {
          value: 'unknown',
          source: 'deterministic_rule',
          status: 'proposed',
          evidence_type: 'none',
        },
        workflow_family: family,
        capture_profile: 'action-supervision',
      },
    ],
    observations: frames.map((f) => f.observation),
    actions: steps,
    artifacts: frames.map((f) => f.artifact),
    rights: {
      declared_by: 'fixture author',
      source_rights: 'own_capture',
      permitted_uses: ['human_review', 'private_retrieval', 'evaluation', 'model_training'],
    },
  };
}

const jsonl = (list) => list.map((x) => JSON.stringify(x)).join('\n') + '\n';

export function bundleFrom(records, { extraFiles = [], checksums = true } = {}) {
  const files = [
    ['manifest.json', JSON.stringify(records.manifest)],
    ['sessions.jsonl', jsonl(records.sessions)],
    ['episodes.jsonl', jsonl(records.episodes)],
    ['observations.jsonl', jsonl(records.observations)],
    ['actions.jsonl', jsonl(records.actions)],
    ['artifacts.jsonl', jsonl(records.artifacts)],
    ['rights.json', JSON.stringify(records.rights)],
    ...records.artifacts.map((a) => [a.path, PNG_1X1]),
    ...extraFiles,
  ];
  if (checksums)
    files.push([
      'checksums.json',
      JSON.stringify(
        Object.fromEntries(
          files.map(([n, c]) => [n, sha(Buffer.isBuffer(c) ? c : Buffer.from(c))]),
        ),
      ),
    ]);
  return writeZip(files);
}

export const sampleBundle = (options) => bundleFrom(sampleRecords(options));
