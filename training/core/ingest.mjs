// Ingestion: turn an untrusted file into a staged, validated, scrubbed set of
// records. Nothing here persists anything; the service commits a stage in one
// transaction after preview. Imported text is data: it is never executed,
// rendered as HTML, or followed as an instruction.
import { createHash, randomBytes } from 'node:crypto';
import { W3bsError } from '../../src/core.mjs';
import {
  Action,
  Artifact,
  BUNDLE_FILES,
  Episode,
  LabelRevision,
  MEDIA_TYPES,
  Manifest,
  Observation,
  Rights,
  SCHEMA_VERSION,
  Session,
  ACTION_TYPES,
} from '../schema/format.mjs';
import { readZip } from './zip.mjs';
import { scrubRecord } from './privacy.mjs';
import { segmentSteps } from './segment.mjs';

export const newId = (prefix) => `${prefix}_${randomBytes(10).toString('base64url')}`;
export const sha256 = (data) => createHash('sha256').update(data).digest('hex');

const invalid = (message, details) => {
  const error = new W3bsError('IMPORT_INVALID', message, 422);
  error.details = details;
  return error;
};

function parseJsonl(name, text, schema, errors, maxRecords = 200000) {
  const records = [];
  const lines = text.split('\n');
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i].trim();
    if (!line) continue;
    if (records.length >= maxRecords) {
      errors.push(`${name}: more than ${maxRecords} records.`);
      break;
    }
    let value;
    try {
      value = JSON.parse(line);
    } catch {
      errors.push(`${name}:${i + 1}: not JSON.`);
      continue;
    }
    const parsed = schema.safeParse(value);
    if (!parsed.success) {
      errors.push(
        `${name}:${i + 1}: ${parsed.error.issues.map((issue) => `${issue.path.join('.')} ${issue.message}`).join('; ')}`,
      );
      if (errors.length > 50) break;
      continue;
    }
    records.push(parsed.data);
  }
  return records;
}

const DEFAULT_RIGHTS = {
  declared_by: 'unknown',
  source_rights: 'unknown',
  permitted_uses: ['human_review'],
};

// Validates a native W3BS bundle. Returns a stage or throws IMPORT_INVALID.
export function stageBundle(buffer, { limits, allowText = false } = {}) {
  const files = readZip(buffer, limits);
  const errors = [];
  for (const name of files.keys()) {
    if (name in BUNDLE_FILES) continue;
    const match = /^artifacts\/[A-Za-z0-9._-]+\.([a-z0-9]+)$/.exec(name);
    if (!match || !MEDIA_TYPES[match[1]]) errors.push(`${name}: not an allowed bundle file.`);
  }
  if (!files.has('manifest.json')) errors.push('manifest.json is missing.');
  if (errors.length) throw invalid('Bundle failed validation.', errors);
  let manifest;
  try {
    manifest = Manifest.parse(JSON.parse(files.get('manifest.json').toString('utf8')));
  } catch (error) {
    const version = (() => {
      try {
        return JSON.parse(files.get('manifest.json').toString('utf8')).schema_version;
      } catch {
        return undefined;
      }
    })();
    if (version && version !== SCHEMA_VERSION)
      throw new W3bsError(
        'UNSUPPORTED_SCHEMA_VERSION',
        `Bundle schema_version ${version} is not supported; expected ${SCHEMA_VERSION}.`,
        422,
      );
    throw invalid('manifest.json failed validation.', [error.message]);
  }
  const read = (name, schema) =>
    files.has(name) ? parseJsonl(name, files.get(name).toString('utf8'), schema, errors) : [];
  const sessions = read('sessions.jsonl', Session);
  const episodes = read('episodes.jsonl', Episode);
  const observations = read('observations.jsonl', Observation);
  const steps = read('actions.jsonl', Action);
  const labels = read('labels.jsonl', LabelRevision);
  const artifactRecords = read('artifacts.jsonl', Artifact);
  let rights = DEFAULT_RIGHTS;
  if (files.has('rights.json')) {
    const parsed = Rights.safeParse(JSON.parse(files.get('rights.json').toString('utf8')));
    if (parsed.success) rights = parsed.data;
    else errors.push('rights.json failed validation.');
  }
  if (files.has('checksums.json')) {
    const sums = JSON.parse(files.get('checksums.json').toString('utf8'));
    for (const [name, digest] of Object.entries(sums)) {
      if (!files.has(name)) errors.push(`checksums.json lists missing file ${name}.`);
      else if (sha256(files.get(name)) !== digest) errors.push(`${name}: checksum mismatch.`);
    }
  }
  // Referential integrity.
  const sessionIds = new Set(sessions.map((s) => s.session_id));
  const observationIds = new Set(observations.map((o) => o.observation_id));
  const stepIds = new Set(steps.map((s) => s.step_id));
  const artifactIds = new Set(artifactRecords.map((a) => a.artifact_id));
  const unique = (list, key, label) => {
    const seen = new Set();
    for (const item of list) {
      if (seen.has(item[key])) errors.push(`${label}: duplicate ${item[key]}.`);
      seen.add(item[key]);
    }
  };
  unique(sessions, 'session_id', 'sessions');
  unique(episodes, 'episode_id', 'episodes');
  unique(observations, 'observation_id', 'observations');
  unique(steps, 'step_id', 'actions');
  unique(artifactRecords, 'artifact_id', 'artifacts');
  for (const o of observations) {
    if (!sessionIds.has(o.session_id))
      errors.push(`${o.observation_id}: unknown session ${o.session_id}.`);
    if (o.artifact_id && !artifactIds.has(o.artifact_id))
      errors.push(`${o.observation_id}: unknown artifact ${o.artifact_id}.`);
  }
  for (const s of steps) {
    if (!sessionIds.has(s.session_id))
      errors.push(`${s.step_id}: unknown session ${s.session_id}.`);
    for (const ref of [s.observation_before, s.observation_after])
      if (ref && !observationIds.has(ref)) errors.push(`${s.step_id}: unknown observation ${ref}.`);
  }
  for (const e of episodes) {
    for (const seg of e.segments)
      if (!sessionIds.has(seg.session_id))
        errors.push(`${e.episode_id}: unknown session ${seg.session_id}.`);
    for (const step of e.steps)
      if (!stepIds.has(step)) errors.push(`${e.episode_id}: unknown step ${step}.`);
  }
  const artifacts = [];
  for (const record of artifactRecords) {
    const data = files.get(record.path);
    if (!data) {
      errors.push(`${record.artifact_id}: file ${record.path} is missing.`);
      continue;
    }
    if (data.length !== record.bytes || sha256(data) !== record.sha256) {
      errors.push(`${record.artifact_id}: size or digest does not match ${record.path}.`);
      continue;
    }
    if (!sniffMatches(data, record.media_type)) {
      errors.push(`${record.artifact_id}: content is not ${record.media_type}.`);
      continue;
    }
    artifacts.push({ record, data });
  }
  if (errors.length) throw invalid('Bundle failed validation.', errors.slice(0, 50));
  return finishStage({
    kind: 'bundle',
    profile: manifest.capture_profile,
    sessions,
    episodes,
    observations,
    steps,
    labels,
    artifacts,
    rights,
    allowText: allowText && manifest.privacy.text_values_included,
    notes: [],
  });
}

// Magic-number check so a bundle cannot smuggle HTML or a script as an image.
export function sniffMatches(data, mediaType) {
  const head = data.subarray(0, 16);
  switch (mediaType) {
    case 'image/png':
      return head
        .subarray(0, 8)
        .equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]));
    case 'image/jpeg':
      return head[0] === 0xff && head[1] === 0xd8 && head[2] === 0xff;
    case 'image/webp':
      return head.toString('latin1', 0, 4) === 'RIFF' && head.toString('latin1', 8, 12) === 'WEBP';
    case 'video/webm':
      return head.subarray(0, 4).equals(Buffer.from([0x1a, 0x45, 0xdf, 0xa3]));
    case 'video/mp4':
      return head.toString('latin1', 4, 8) === 'ftyp';
    default:
      return false;
  }
}

const get = (object, path) =>
  path
    ? path.split('.').reduce((node, key) => (node == null ? undefined : node[key]), object)
    : undefined;

// Maps a generic JSON/JSONL event file. `mapping` names source fields:
// { timestamp, timestamp_unit: 'ms'|'s'|'ns'|'iso', action_type, action_type_map,
//   target_name, target_role, app, window, url, x, y, keys, episode_key, goal,
//   outcome, outcome_map, origin }
export function stageEvents(text, mapping, { allowText = false } = {}) {
  if (!mapping || typeof mapping !== 'object')
    throw invalid('A mapping file is required for JSON/JSONL imports.', []);
  if (!mapping.timestamp || !mapping.action_type)
    throw invalid('The mapping needs at least "timestamp" and "action_type".', []);
  let rows;
  const trimmed = text.trim();
  try {
    rows = trimmed.startsWith('[')
      ? JSON.parse(trimmed)
      : trimmed
          .split('\n')
          .filter(Boolean)
          .map((line) => JSON.parse(line));
  } catch {
    throw invalid('File is not JSON or JSONL.', []);
  }
  if (!Array.isArray(rows) || rows.length === 0) throw invalid('No events found.', []);
  if (rows.length > 200000) throw invalid('More than 200000 events.', []);
  const unit = mapping.timestamp_unit || 'ms';
  const toNs = (value) => {
    if (unit === 'iso') {
      const ms = Date.parse(value);
      if (Number.isNaN(ms)) return null;
      return BigInt(ms) * 1000000n;
    }
    if (typeof value !== 'number' && typeof value !== 'string') return null;
    const scale = { s: 1000000000n, ms: 1000000n, us: 1000n, ns: 1n }[unit];
    if (!scale) return null;
    try {
      return unit === 'ns'
        ? BigInt(value)
        : BigInt(Math.round(Number(value) * 1000)) * (scale / 1000n || 1n);
    } catch {
      return null;
    }
  };
  const cannotInfer = new Set();
  const report = { rows: rows.length, skipped: 0, skipped_reasons: {} };
  const skip = (reason) => {
    report.skipped += 1;
    report.skipped_reasons[reason] = (report.skipped_reasons[reason] || 0) + 1;
  };
  const prepared = [];
  for (const row of rows) {
    const t = toNs(get(row, mapping.timestamp));
    if (t === null) {
      skip('timestamp missing or unparseable');
      continue;
    }
    const rawType = get(row, mapping.action_type);
    const type = mapping.action_type_map?.[rawType] ?? rawType;
    if (!ACTION_TYPES.includes(type)) {
      skip(`unmapped action type ${String(rawType).slice(0, 40)}`);
      continue;
    }
    prepared.push({ t, type, row });
  }
  if (!prepared.length)
    throw invalid('No event could be mapped.', [JSON.stringify(report.skipped_reasons)]);
  prepared.sort((a, b) => (a.t < b.t ? -1 : a.t > b.t ? 1 : 0));
  const anchorNs = prepared[0].t;
  const session = Session.parse({
    schema_version: SCHEMA_VERSION,
    session_id: newId('ses'),
    capture_profile: 'semantic-demonstration',
    started_at: new Date(Number(anchorNs / 1000000n)).toISOString(),
    ended_at: new Date(Number(prepared.at(-1).t / 1000000n)).toISOString(),
    clock: {
      wall_clock_anchor: new Date(Number(anchorNs / 1000000n)).toISOString(),
      monotonic_anchor_ns: '0',
    },
    adapters: [{ name: 'jsonl-mapping', version: '0.1' }],
  });
  if (!mapping.origin) cannotInfer.add('actor origin (recorded as unknown)');
  if (!mapping.target_name && !mapping.target_role) cannotInfer.add('UI targets');
  if (!mapping.goal) cannotInfer.add('task goals');
  if (!mapping.outcome) cannotInfer.add('outcomes');
  cannotInfer.add('before/after observations (no frames in an event file)');
  const steps = prepared.map(({ t, type, row }, index) => {
    const target = {};
    for (const [key, field] of [
      ['name', 'target_name'],
      ['role', 'target_role'],
      ['app', 'app'],
      ['window', 'window'],
    ]) {
      const value = get(row, mapping[field]);
      if (value != null) target[key] = String(value).slice(0, 400);
    }
    const args = {};
    for (const key of ['x', 'y']) {
      const value = Number(get(row, mapping[key]));
      if (Number.isFinite(value)) args[key] = value;
    }
    const keys = get(row, mapping.keys);
    if (Array.isArray(keys)) args.keys = keys.slice(0, 8).map((k) => String(k).slice(0, 40));
    const url = get(row, mapping.url);
    if (url) args.url = String(url).slice(0, 2000);
    const textValue = get(row, mapping.text);
    if (textValue != null) args.text = String(textValue).slice(0, 4000);
    const origin = mapping.origin ? get(row, mapping.origin) : undefined;
    return {
      record: Action.parse({
        schema_version: SCHEMA_VERSION,
        step_id: newId('step'),
        session_id: session.session_id,
        sequence: index,
        action: {
          type,
          origin: ['human', 'agent', 'system', 'mixed'].includes(origin) ? origin : 'unknown',
          source: 'imported',
          status: 'proposed',
          ...(Object.keys(target).length ? { target: { ...target, source: 'imported' } } : {}),
          coordinate_space: 'x' in args ? 'display' : 'none',
          arguments: args,
          monotonic_ns: String(t - anchorNs),
        },
        alignment: { status: 'missing' },
      }),
      key: mapping.episode_key ? get(row, mapping.episode_key) : undefined,
      goal: mapping.goal ? get(row, mapping.goal) : undefined,
      outcome: mapping.outcome ? get(row, mapping.outcome) : undefined,
    };
  });
  // Group into episodes: explicit key first, otherwise idle-gap segmentation.
  const groups = new Map();
  if (mapping.episode_key) {
    for (const step of steps) {
      const key = step.key == null ? '__none__' : String(step.key);
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key).push(step);
    }
  } else {
    segmentSteps(steps.map((s) => s.record)).forEach((group, i) =>
      groups.set(
        `segment-${i}`,
        group.map((record) => steps.find((s) => s.record === record)),
      ),
    );
  }
  const episodes = [];
  for (const group of groups.values()) {
    const goal = group.find((s) => s.goal != null)?.goal;
    const rawOutcome = group.findLast((s) => s.outcome != null)?.outcome;
    const outcome = mapping.outcome_map?.[rawOutcome] ?? rawOutcome;
    episodes.push(
      Episode.parse({
        schema_version: SCHEMA_VERSION,
        episode_id: newId('ep'),
        goal: goal
          ? { value: String(goal).slice(0, 500), source: 'imported', status: 'proposed' }
          : {
              value: suggestGoal(group.map((s) => s.record)),
              source: 'deterministic_rule',
              status: 'proposed',
            },
        segments: [
          {
            session_id: session.session_id,
            from_ns: group[0].record.action.monotonic_ns,
            to_ns: group.at(-1).record.action.monotonic_ns,
          },
        ],
        steps: group.map((s) => s.record.step_id),
        outcome: {
          value: ['success', 'partial', 'failure', 'interrupted', 'unknown'].includes(outcome)
            ? outcome
            : 'unknown',
          source: outcome ? 'imported' : 'deterministic_rule',
          status: 'proposed',
          evidence_type: 'none',
        },
        capture_profile: 'semantic-demonstration',
      }),
    );
  }
  return finishStage({
    kind: 'events',
    profile: steps.some((s) => s.record.action.target) ? 'semantic-demonstration' : 'visual-review',
    sessions: [session],
    episodes,
    observations: [],
    steps: steps.map((s) => s.record),
    labels: [],
    artifacts: [],
    rights: DEFAULT_RIGHTS,
    allowText,
    notes: [...cannotInfer].map((what) => `Cannot infer ${what}.`),
    mappingReport: report,
  });
}

// Video plus optional annotations [{start_ms, end_ms, goal, outcome}]. Video
// enters the visual-review profile: any action derived from it later stays
// inferred until a human reviews it.
export function stageVideo(data, mediaType, annotations = [], { durationMs } = {}) {
  if (!sniffMatches(data, mediaType)) throw invalid(`File content is not ${mediaType}.`, []);
  const session = Session.parse({
    schema_version: SCHEMA_VERSION,
    session_id: newId('ses'),
    capture_profile: 'visual-review',
    started_at: new Date().toISOString(),
    clock: { wall_clock_anchor: new Date().toISOString(), monotonic_anchor_ns: '0' },
    adapters: [{ name: 'video-import', version: '0.1' }],
  });
  const artifact = {
    record: Artifact.parse({
      schema_version: SCHEMA_VERSION,
      artifact_id: newId('art'),
      path: `artifacts/video.${mediaType === 'video/mp4' ? 'mp4' : 'webm'}`,
      media_type: mediaType,
      sha256: sha256(data),
      bytes: data.length,
    }),
    data,
  };
  const observation = Observation.parse({
    schema_version: SCHEMA_VERSION,
    observation_id: newId('obs'),
    session_id: session.session_id,
    monotonic_ns: '0',
    kind: 'video_segment',
    artifact_id: artifact.record.artifact_id,
    ...(durationMs ? { duration_ns: String(BigInt(Math.round(durationMs)) * 1000000n) } : {}),
  });
  const spans = annotations.length
    ? annotations
    : [{ start_ms: 0, end_ms: durationMs || 0, goal: undefined }];
  const episodes = spans.map((span) =>
    Episode.parse({
      schema_version: SCHEMA_VERSION,
      episode_id: newId('ep'),
      goal: span.goal
        ? { value: String(span.goal).slice(0, 500), source: 'imported', status: 'proposed' }
        : {
            value: 'Untitled video demonstration',
            source: 'deterministic_rule',
            status: 'proposed',
          },
      segments: [
        {
          session_id: session.session_id,
          from_ns: String(BigInt(Math.max(0, Math.round(span.start_ms || 0))) * 1000000n),
          to_ns: String(
            BigInt(Math.max(0, Math.round(span.end_ms || span.start_ms || 0))) * 1000000n,
          ),
        },
      ],
      outcome: {
        value: ['success', 'partial', 'failure', 'interrupted'].includes(span.outcome)
          ? span.outcome
          : 'unknown',
        source: span.outcome ? 'imported' : 'deterministic_rule',
        status: 'proposed',
        evidence_type: 'none',
      },
      capture_profile: 'visual-review',
    }),
  );
  return finishStage({
    kind: 'video',
    profile: 'visual-review',
    sessions: [session],
    episodes,
    observations: [observation],
    steps: [],
    labels: [],
    artifacts: [artifact],
    rights: DEFAULT_RIGHTS,
    allowText: false,
    notes: [
      'Video-only data enters the visual-review profile. No device-observed actions are present.',
    ],
  });
}

export function suggestGoal(steps) {
  const apps = [...new Set(steps.map((s) => s.action.target?.app).filter(Boolean))];
  const named = steps.map((s) => s.action.target?.name).filter(Boolean);
  const last = named.at(-1);
  if (last && apps.length) return `${last} in ${apps.slice(0, 2).join(' and ')}`;
  if (last) return `Work ending with "${last}"`;
  if (apps.length) return `Work in ${apps.slice(0, 2).join(' and ')}`;
  return 'Untitled task';
}

// Shared tail: privacy scrub, per-episode warnings, preview report.
function finishStage(stage) {
  const warnings = [];
  const stepEpisode = new Map();
  for (const episode of stage.episodes)
    for (const step of episode.steps) stepEpisode.set(step, episode.episode_id);
  const sessionEpisodes = new Map();
  for (const episode of stage.episodes)
    for (const seg of episode.segments) {
      if (!sessionEpisodes.has(seg.session_id)) sessionEpisodes.set(seg.session_id, new Set());
      sessionEpisodes.get(seg.session_id).add(episode.episode_id);
    }
  const sessionText = new Map(
    stage.sessions.map((s) => [s.session_id, s.scope?.text_values === true]),
  );
  const attach = (findings, episodeIds) => {
    for (const finding of findings)
      for (const episode_id of episodeIds) warnings.push({ episode_id, ...finding });
  };
  for (const step of stage.steps) {
    const allow = stage.allowText && sessionText.get(step.session_id);
    const findings = scrubRecord(step, step.step_id, { allowText: allow });
    const owner = stepEpisode.get(step.step_id);
    attach(findings, owner ? [owner] : [...(sessionEpisodes.get(step.session_id) || [])]);
  }
  for (const observation of stage.observations)
    attach(scrubRecord(observation, observation.observation_id), [
      ...(sessionEpisodes.get(observation.session_id) || []),
    ]);
  for (const episode of stage.episodes)
    attach(scrubRecord(episode, episode.episode_id, { allowText: true }), [episode.episode_id]);
  for (const label of stage.labels) {
    const owner =
      label.subject.kind === 'episode' ? label.subject.id : stepEpisode.get(label.subject.id);
    attach(scrubRecord(label, label.revision_id, { allowText: true }), owner ? [owner] : []);
  }
  const gaps = stage.sessions.flatMap((s) =>
    s.gaps.map((g) => ({ session_id: s.session_id, ...g })),
  );
  stage.warnings = warnings;
  stage.preview = {
    kind: stage.kind,
    capture_profile: stage.profile,
    counts: {
      sessions: stage.sessions.length,
      episodes: stage.episodes.length,
      observations: stage.observations.length,
      actions: stage.steps.length,
      labels: stage.labels.length,
      artifacts: stage.artifacts.length,
      artifact_bytes: stage.artifacts.reduce((sum, a) => sum + a.data.length, 0),
    },
    rights: stage.rights,
    privacy: {
      blocking_warnings: warnings.filter((w) => w.blocking).length,
      text_values_removed: warnings.filter((w) => w.kind === 'text_value_removed').length,
      kinds: [...new Set(warnings.map((w) => w.kind))],
    },
    gaps: gaps.length,
    alignment: {
      verified: stage.steps.filter((s) => s.alignment.status === 'verified').length,
      uncertain: stage.steps.filter((s) => s.alignment.status !== 'verified').length,
    },
    episodes: stage.episodes
      .slice(0, 20)
      .map((e) => ({ episode_id: e.episode_id, goal: e.goal.value, steps: e.steps.length })),
    notes: stage.notes,
    ...(stage.mappingReport ? { mapping: stage.mappingReport } : {}),
  };
  return stage;
}
