// W3BS Demonstration Dataset Format, community draft 0.1.
//
// This module is the single source of the record contract. The JSON Schema
// files in training/schema/json are generated from it (`bun
// training/schema/generate.mjs`) and a test fails when they drift. Nothing here
// is a ratified W3BS standard.
import { z } from 'zod';

export const SCHEMA_VERSION = '0.1';
export const FORMAT_NAME = 'w3bs-demonstration-dataset';

export const CAPTURE_PROFILES = ['visual-review', 'semantic-demonstration', 'action-supervision'];
export const LABEL_SOURCES = [
  'human',
  'device_event',
  'application_event',
  'deterministic_rule',
  'model_inference',
  'imported',
];
export const ACTOR_ORIGINS = ['human', 'agent', 'system', 'mixed', 'unknown'];
export const LABEL_STATUSES = ['proposed', 'confirmed', 'rejected'];
export const OUTCOMES = ['success', 'partial', 'failure', 'interrupted', 'unknown'];
export const ACTION_TYPES = [
  'click',
  'double_click',
  'right_click',
  'drag',
  'hover',
  'scroll',
  'key_shortcut',
  'text_edit',
  'focus',
  'navigate',
  'wait',
  'other',
];
export const PURPOSES = [
  'human_review',
  'private_retrieval',
  'external_retrieval',
  'model_training',
  'evaluation',
  'public_distribution',
];
export const GAP_KINDS = [
  'capture_denied',
  'protected_surface',
  'inaccessible_control',
  'dropped_frames',
  'missing_origin',
  'unsupported_ime',
  'paused',
  'excluded_scope',
  'other',
];
export const MEDIA_TYPES = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  webp: 'image/webp',
  webm: 'video/webm',
  mp4: 'video/mp4',
};

const id = (prefix) =>
  z
    .string()
    .regex(
      new RegExp(`^${prefix}_[A-Za-z0-9_-]{1,64}$`),
      `expected an id starting with ${prefix}_`,
    );
// Nanosecond counters exceed Number.MAX_SAFE_INTEGER within days of uptime, so
// they travel as decimal strings.
const ns = z.string().regex(/^(0|[1-9][0-9]{0,24})$/, 'expected a decimal nanosecond string');
const timestamp = z.iso.datetime({ offset: true });
const text = (max) => z.string().max(max);
const sha256 = z.string().regex(/^[0-9a-f]{64}$/);
// Bundle-relative path: no absolute paths, no parent segments, no backslashes.
export const relativePath = z
  .string()
  .max(240)
  .regex(/^(?!\/)(?!.*(^|\/)\.\.?(\/|$))[A-Za-z0-9._\/-]+$/, 'expected a safe relative path');

export const evidenceRef = z.string().max(80);

export const Label = z
  .object({
    value: z.unknown(),
    source: z.enum(LABEL_SOURCES),
    status: z.enum(LABEL_STATUSES),
    evidence: z.array(evidenceRef).max(64).default([]),
    confidence: z.number().min(0).max(1).optional(),
  })
  .strict();

export const Target = z
  .object({
    app: text(200).optional(),
    window: text(400).optional(),
    role: text(80).optional(),
    name: text(400).optional(),
    identifiers: z.record(z.string().max(80), z.string().max(400)).optional(),
    bounds: z
      .object({ x: z.number(), y: z.number(), width: z.number().min(0), height: z.number().min(0) })
      .strict()
      .optional(),
    source: z.enum([
      'accessibility',
      'browser_metadata',
      'window_manager',
      'visual_inference',
      'imported',
      'unknown',
    ]),
  })
  .strict();

export const Gap = z
  .object({
    kind: z.enum(GAP_KINDS),
    from_ns: ns,
    to_ns: ns.optional(),
    detail: text(500).optional(),
  })
  .strict();

export const Session = z
  .object({
    schema_version: z.literal(SCHEMA_VERSION),
    session_id: id('ses'),
    capture_profile: z.enum(CAPTURE_PROFILES),
    started_at: timestamp,
    ended_at: timestamp.optional(),
    clock: z
      .object({
        wall_clock_anchor: timestamp,
        monotonic_anchor_ns: ns,
        sync_uncertainty_ns: ns.default('0'),
      })
      .strict(),
    device: z
      .object({
        os: text(80).optional(),
        desktop: text(80).optional(),
        locale: text(40).optional(),
        keyboard_layout: text(40).optional(),
        displays: z
          .array(
            z
              .object({
                id: text(40),
                x: z.number(),
                y: z.number(),
                width: z.number().positive(),
                height: z.number().positive(),
                scale: z.number().positive().default(1),
              })
              .strict(),
          )
          .max(16)
          .default([]),
      })
      .strict()
      .default({ displays: [] }),
    scope: z
      .object({
        applications: z.array(text(200)).max(200).default([]),
        origins: z.array(text(300)).max(200).default([]),
        text_values: z.boolean().default(false),
      })
      .strict()
      .default({ applications: [], origins: [], text_values: false }),
    capture_grant: z
      .object({
        grant_id: text(80),
        purpose: text(200),
        expires_at: timestamp.optional(),
        revocation_generation: z.number().int().min(0).default(0),
      })
      .strict()
      .optional(),
    adapters: z
      .array(z.object({ name: text(80), version: text(40) }).strict())
      .max(32)
      .default([]),
    gaps: z.array(Gap).max(10000).default([]),
  })
  .strict();

export const Segment = z
  .object({ session_id: id('ses'), from_ns: ns, to_ns: ns })
  .strict()
  .refine((value) => BigInt(value.from_ns) <= BigInt(value.to_ns), 'segment ends before it starts');

export const Rights = z
  .object({
    declared_by: text(200),
    source_rights: z.enum(['own_capture', 'licensed', 'authorized', 'unknown']),
    permitted_uses: z.array(z.enum(PURPOSES)).max(PURPOSES.length),
    license: text(200).optional(),
    notes: text(2000).optional(),
  })
  .strict();

export const Episode = z
  .object({
    schema_version: z.literal(SCHEMA_VERSION),
    episode_id: id('ep'),
    goal: Label,
    segments: z.array(Segment).min(1).max(256),
    steps: z.array(id('step')).max(20000).default([]),
    parameters: z
      .array(
        z
          .object({
            name: z.string().regex(/^[a-z][a-z0-9_]{0,63}$/),
            type: z.enum(['string', 'number', 'date', 'url', 'email', 'file', 'secret_ref']),
            placeholder: text(200).optional(),
          })
          .strict(),
      )
      .max(64)
      .default([]),
    prerequisites: z.array(text(500)).max(64).default([]),
    outcome: Label.extend({
      evidence_type: z
        .enum(['ui_confirmation', 'state_change', 'external_check', 'human_assertion', 'none'])
        .default('none'),
    }),
    explanation: z
      .object({ text: text(8000), author: z.literal('human') })
      .strict()
      .optional(),
    workflow_family: text(200).optional(),
    parent_episode_id: id('ep').optional(),
    capture_profile: z.enum(CAPTURE_PROFILES),
  })
  .strict();

export const Observation = z
  .object({
    schema_version: z.literal(SCHEMA_VERSION),
    observation_id: id('obs'),
    session_id: id('ses'),
    monotonic_ns: ns,
    kind: z.enum(['keyframe', 'crop', 'semantic', 'video_segment']),
    artifact_id: id('art').optional(),
    duration_ns: ns.optional(),
    semantic: z
      .object({
        app: text(200).optional(),
        window: text(400).optional(),
        focused: Target.optional(),
        url_origin: text(300).optional(),
      })
      .strict()
      .optional(),
    display: z
      .object({
        width: z.number().positive(),
        height: z.number().positive(),
        scale: z.number().positive().default(1),
      })
      .strict()
      .optional(),
    // True only when the frame was taken from a buffer before the action's
    // event timestamp. A frame requested after the action never qualifies.
    pre_action_verified: z.boolean().default(false),
    visibility: z
      .object({
        masked_regions: z
          .array(
            z
              .object({ x: z.number(), y: z.number(), width: z.number(), height: z.number() })
              .strict(),
          )
          .max(256)
          .default([]),
        restricted: z.boolean().default(false),
      })
      .strict()
      .default({ masked_regions: [], restricted: false }),
  })
  .strict();

export const Action = z
  .object({
    schema_version: z.literal(SCHEMA_VERSION),
    step_id: id('step'),
    session_id: id('ses'),
    sequence: z.number().int().min(0),
    observation_before: id('obs').optional(),
    observation_after: id('obs').optional(),
    action: z
      .object({
        type: z.enum(ACTION_TYPES),
        origin: z.enum(ACTOR_ORIGINS),
        source: z.enum(LABEL_SOURCES),
        status: z.enum(LABEL_STATUSES).default('confirmed'),
        target: Target.optional(),
        coordinate_space: z.enum(['display', 'window', 'none']).default('none'),
        arguments: z
          .object({
            x: z.number().optional(),
            y: z.number().optional(),
            path: z
              .array(z.tuple([z.number(), z.number()]))
              .max(2000)
              .optional(),
            button: z.enum(['left', 'middle', 'right']).optional(),
            keys: z.array(text(40)).max(8).optional(),
            delta_x: z.number().optional(),
            delta_y: z.number().optional(),
            // Present only when the capture scope allowed text values.
            text: text(4000).optional(),
            text_placeholder: text(80).optional(),
            url: text(2000).optional(),
          })
          .strict()
          .default({}),
        monotonic_ns: ns,
      })
      .strict(),
    alignment: z
      .object({
        status: z.enum(['verified', 'uncertain', 'missing']),
        evidence: z.array(evidenceRef).max(16).default([]),
      })
      .strict(),
    raw_event_refs: z.array(evidenceRef).max(16).default([]),
  })
  .strict();

export const LabelRevision = z
  .object({
    schema_version: z.literal(SCHEMA_VERSION),
    revision_id: id('rev'),
    subject: z
      .object({
        kind: z.enum(['episode', 'step', 'observation']),
        id: z.string().max(80),
      })
      .strict(),
    field_path: z.string().regex(/^[a-z_]+(\.[a-z_]+)*$/),
    value: z.unknown(),
    source: z.enum(LABEL_SOURCES),
    status: z.enum(LABEL_STATUSES),
    evidence: z.array(evidenceRef).max(64).default([]),
    author: z
      .object({
        kind: z.enum(['human', 'model', 'rule', 'import']),
        id: text(120),
        version: text(80).optional(),
      })
      .strict(),
    confidence: z.number().min(0).max(1).optional(),
    created_at: timestamp,
    supersedes: id('rev').optional(),
  })
  .strict();

export const Artifact = z
  .object({
    schema_version: z.literal(SCHEMA_VERSION),
    artifact_id: id('art'),
    path: relativePath.refine(
      (value) => value.startsWith('artifacts/'),
      'artifacts live in artifacts/',
    ),
    media_type: z.enum([...new Set(Object.values(MEDIA_TYPES))]),
    sha256,
    bytes: z.number().int().min(0),
    redacted: z.boolean().default(false),
  })
  .strict();

export const Manifest = z
  .object({
    format: z.literal(FORMAT_NAME),
    schema_version: z.literal(SCHEMA_VERSION),
    bundle_id: id('bun'),
    created_at: timestamp,
    producer: z.object({ name: text(120), version: text(40) }).strict(),
    capture_profile: z.enum(CAPTURE_PROFILES),
    counts: z
      .object({
        sessions: z.number().int().min(0),
        episodes: z.number().int().min(0),
        observations: z.number().int().min(0),
        actions: z.number().int().min(0),
        labels: z.number().int().min(0),
        artifacts: z.number().int().min(0),
      })
      .strict(),
    privacy: z
      .object({ text_values_included: z.boolean(), redaction_applied: z.boolean() })
      .strict(),
    dataset: z
      .object({
        dataset_id: text(80),
        version: z.number().int().min(1),
        digest: sha256,
        purposes: z.array(z.enum(PURPOSES)),
      })
      .strict()
      .optional(),
  })
  .strict();

export const Splits = z
  .object({
    strategy: text(120),
    seed: text(80),
    train: z.array(id('ep')),
    validation: z.array(id('ep')),
    test_known_workflows: z.array(id('ep')),
    test_heldout_families: z.array(id('ep')),
  })
  .strict();

export const DatasetVersion = z
  .object({
    schema_version: z.literal(SCHEMA_VERSION),
    dataset_id: id('ds'),
    version: z.number().int().min(1),
    created_at: timestamp,
    created_by: text(120),
    episodes: z.array(id('ep')).max(100000),
    label_revisions: z.array(id('rev')).max(1000000),
    transformations: z.array(text(200)).default([]),
    splits: Splits,
    purposes: z.array(z.enum(PURPOSES)).default([]),
    digest: sha256,
  })
  .strict();

export const ConsumerRun = z
  .object({
    schema_version: z.literal(SCHEMA_VERSION),
    run_id: id('run'),
    kind: z.enum(['retrieval', 'execution', 'export', 'training', 'evaluation']),
    consumer: z.object({ name: text(120), version: text(80).optional() }).strict(),
    dataset: z
      .object({ dataset_id: id('ds'), version: z.number().int().min(1) })
      .strict()
      .optional(),
    condition: z.enum(['with_demonstrations', 'baseline']).optional(),
    task: text(4000).optional(),
    task_family: text(200).optional(),
    used_episodes: z.array(id('ep')).max(200).default([]),
    outcome: z.enum(OUTCOMES),
    interventions: z.number().int().min(0).default(0),
    unsafe_attempts: z.number().int().min(0).default(0),
    elapsed_ms: z.number().int().min(0).optional(),
    cost_usd: z.number().min(0).optional(),
    created_at: timestamp,
  })
  .strict();

// Files a bundle may contain. Anything else is rejected at import.
export const BUNDLE_FILES = {
  'manifest.json': Manifest,
  'sessions.jsonl': Session,
  'episodes.jsonl': Episode,
  'observations.jsonl': Observation,
  'actions.jsonl': Action,
  'labels.jsonl': LabelRevision,
  'artifacts.jsonl': Artifact,
  'splits.json': Splits,
  'rights.json': Rights,
  'checksums.json': z.record(relativePath, sha256),
  'DATASET.md': null,
};

export const RECORDS = {
  manifest: Manifest,
  session: Session,
  episode: Episode,
  observation: Observation,
  action: Action,
  label_revision: LabelRevision,
  artifact: Artifact,
  rights: Rights,
  splits: Splits,
  dataset_version: DatasetVersion,
  consumer_run: ConsumerRun,
};

export function jsonSchemas() {
  const out = {};
  for (const [name, schema] of Object.entries(RECORDS)) {
    out[name] = {
      ...z.toJSONSchema(schema, { io: 'input', unrepresentable: 'any' }),
      $id: `https://w3bs.org/schemas/training/${SCHEMA_VERSION}/${name}.json`,
      title: `W3BS Demonstration Dataset ${SCHEMA_VERSION}: ${name}`,
    };
  }
  return out;
}
