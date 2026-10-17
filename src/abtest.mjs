// W3BS A/B testing: hosted OpenABTest 0.1-draft experiments (logicsrc.com/docs/openabtest).
//
// An owner defines an experiment (variants, weights, window) on w3bs.org/ab-testing. Their
// server calls `assign` with an opaque participant id and an `ab_` key; the first call picks
// a variant with an HMAC of the participant under the experiment's own secret and stores it,
// and every later call returns the stored one (sticky, server authority). The server then
// reports exposures (the variant was shown) and conversions (the goal happened). Results
// count unique participants per variant and never name a winner: the spec's winner policy
// is manual review. The manifest and the event stream export as OpenABTest documents.
import { createHash, createHmac, randomBytes } from 'node:crypto';
import { validate } from '@logicsrc/validators';
import { W3bsError } from './core.mjs';

const tables = () => `
  CREATE TABLE IF NOT EXISTS ab_keys (key_hash TEXT PRIMARY KEY, id TEXT NOT NULL UNIQUE, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, name TEXT NOT NULL, created_at TEXT NOT NULL, last_used_at TEXT);
  CREATE INDEX IF NOT EXISTS ab_keys_user ON ab_keys(user_id);
  CREATE TABLE IF NOT EXISTS ab_experiments (id TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, slug TEXT NOT NULL, state TEXT NOT NULL, definition TEXT NOT NULL, secret TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, started_at TEXT, UNIQUE (user_id, slug));
  CREATE TABLE IF NOT EXISTS ab_assignments (experiment_id TEXT NOT NULL REFERENCES ab_experiments(id) ON DELETE CASCADE, participant_id TEXT NOT NULL, id TEXT NOT NULL, revision INTEGER NOT NULL, variant_id TEXT NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY (experiment_id, participant_id));
  CREATE TABLE IF NOT EXISTS ab_events (experiment_id TEXT NOT NULL REFERENCES ab_experiments(id) ON DELETE CASCADE, id TEXT NOT NULL, kind TEXT NOT NULL, participant_id TEXT NOT NULL, assignment_id TEXT NOT NULL, variant_id TEXT NOT NULL, surface TEXT, offer_id TEXT, conversion_id TEXT, value_minor BIGINT, currency TEXT, price TEXT, body TEXT NOT NULL, at TEXT NOT NULL, ingested_at TEXT NOT NULL, PRIMARY KEY (experiment_id, id));
  CREATE UNIQUE INDEX IF NOT EXISTS ab_events_conversion ON ab_events(experiment_id, conversion_id);
  CREATE INDEX IF NOT EXISTS ab_events_kind ON ab_events(experiment_id, kind, variant_id);`;
export const ABTEST_SCHEMA = { sqlite: tables(), postgres: tables() };

// The limits the page states; change them here and the page follows.
export const AB_LIMITS = {
  experiments: 50, // per account
  keys: 10, // per account
  variants: 8, // per experiment
  idLength: 128, // participant, event, conversion and variant ids (the spec's maximum)
  defaultDays: 30, // window length when none is given
  floorPerVariant: 100, // default minimumEligiblePerVariant
  floorDays: 7, // default minimumObservationSeconds, in days
  eventsExport: 50000, // newest events in one export
};
export const STATES = ['draft', 'running', 'paused', 'closed'];
export const UNITS = ['user', 'account', 'session', 'device', 'custom'];
// Which state changes the owner may make. closed is terminal (spec: a closed experiment
// cannot resume; make a new one).
const MOVES = {
  draft: ['running', 'closed'],
  running: ['paused', 'closed'],
  paused: ['running', 'closed'],
  closed: [],
};
export const PRODUCER_ID = 'w3bs.org';
const ALGORITHM = 'hmac-sha256-persisted-v1';
const ID = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/;
const SLUG = /^[a-z0-9][a-z0-9-]{0,63}$/;
const CURRENCY = /^[A-Z]{3}$/;

const random = (bytes = 24) => randomBytes(bytes).toString('base64url');
const sha256 = (value) => createHash('sha256').update(value).digest('hex');
const nowIso = () => new Date().toISOString();
const fail = (code, message, status = 400) => new W3bsError(code, message, status);
const text = (value, max) => (typeof value === 'string' ? value.trim().slice(0, max) : '');

function opaqueId(value, what) {
  if (typeof value !== 'string' || !ID.test(value))
    throw fail(
      'INVALID_ID',
      `${what} must be 1-128 letters, digits, dot, underscore, colon or dash, starting with a letter or digit.`,
    );
  return value;
}
// The spec keeps names, emails and other personal data out of participant ids.
function participantId(value) {
  if (typeof value === 'string' && value.includes('@'))
    throw fail(
      'INVALID_PARTICIPANT',
      'Send an opaque participant id (an internal user id or a hash), not an email address.',
    );
  return opaqueId(value, 'participant');
}
function timestamp(value, what) {
  const ms = Date.parse(value);
  if (typeof value !== 'string' || !Number.isFinite(ms))
    throw fail('INVALID_TIME', `${what} must be an RFC 3339 timestamp.`);
  return new Date(ms).toISOString();
}
function eventTime(value) {
  if (value === undefined) return nowIso();
  const at = timestamp(value, 'at');
  if (Date.parse(at) > Date.now() + 5 * 60 * 1000)
    throw fail('INVALID_TIME', 'at is in the future.');
  return at;
}

// What the owner may set, cleaned. Everything else in the manifest is fixed by the spec.
export function cleanDefinition(input = {}, previous = {}) {
  const source = { ...previous, ...input };
  const name = text(source.name, 120);
  if (!name) throw fail('INVALID_EXPERIMENT', 'Give the experiment a name.');
  const unit = UNITS.includes(source.unit) ? source.unit : 'user';
  const rule =
    text(source.eligibilityRule, 1000) ||
    'Every participant the experiment owner sends to assign while the experiment runs.';
  if (!Array.isArray(source.variants) || source.variants.length < 2)
    throw fail('INVALID_VARIANTS', 'An experiment needs at least two variants.');
  if (source.variants.length > AB_LIMITS.variants)
    throw fail('INVALID_VARIANTS', `An experiment has at most ${AB_LIMITS.variants} variants.`);
  const seen = new Set();
  const variants = source.variants.map((v) => {
    const id = opaqueId(text(v?.id, 128), 'variant id');
    if (seen.has(id)) throw fail('INVALID_VARIANTS', `Two variants are named ${id}.`);
    seen.add(id);
    const weight = Number(v?.weight ?? 1);
    if (!Number.isInteger(weight) || weight < 1 || weight > 1000)
      throw fail('INVALID_VARIANTS', `Variant ${id}: weight must be a whole number, 1 to 1000.`);
    let parameters = v?.parameters;
    if (parameters === undefined || parameters === null || parameters === '')
      parameters = { variant: id };
    if (typeof parameters === 'string') {
      try {
        parameters = JSON.parse(parameters);
      } catch {
        throw fail('INVALID_VARIANTS', `Variant ${id}: parameters must be a JSON object.`);
      }
    }
    if (!parameters || typeof parameters !== 'object' || Array.isArray(parameters))
      throw fail('INVALID_VARIANTS', `Variant ${id}: parameters must be a JSON object.`);
    if (!Object.keys(parameters).length) parameters = { variant: id };
    for (const [key, value] of Object.entries(parameters)) {
      if (!ID.test(key))
        throw fail('INVALID_VARIANTS', `Variant ${id}: parameter name ${key} is not allowed.`);
      if (value !== null && !['string', 'number', 'boolean'].includes(typeof value))
        throw fail(
          'INVALID_VARIANTS',
          `Variant ${id}: parameter ${key} must be a string, number, true/false or null.`,
        );
    }
    return { id, weight, parameters };
  });
  const startsAt = source.startsAt ? timestamp(source.startsAt, 'startsAt') : null;
  const endsAt = source.endsAt ? timestamp(source.endsAt, 'endsAt') : null;
  if (startsAt && endsAt && Date.parse(endsAt) <= Date.parse(startsAt))
    throw fail('INVALID_WINDOW', 'The window must end after it starts.');
  const length = Number(source.days ?? AB_LIMITS.defaultDays);
  if (!Number.isFinite(length) || length <= 0 || length > 3650)
    throw fail('INVALID_WINDOW', 'days must be between 1 and 3650.');
  const floor = Number(source.minimumEligiblePerVariant ?? AB_LIMITS.floorPerVariant);
  const days = Number(source.minimumObservationDays ?? AB_LIMITS.floorDays);
  if (!Number.isInteger(floor) || floor < 1 || floor > 10_000_000)
    throw fail(
      'INVALID_EXPERIMENT',
      'minimumEligiblePerVariant must be a whole number, 1 or more.',
    );
  if (!Number.isFinite(days) || days < 0 || days > 3650)
    throw fail('INVALID_EXPERIMENT', 'minimumObservationDays must be between 0 and 3650.');
  return {
    name,
    unit,
    eligibilityRule: rule,
    variants,
    startsAt,
    endsAt,
    days: length,
    minimumEligiblePerVariant: floor,
    minimumObservationDays: days,
  };
}

// The OpenABTest manifest for an experiment. A draft has no fixed window yet; it is shown
// from now for the default length, and fixed when the experiment starts.
export function toManifest(row) {
  const d = JSON.parse(row.definition);
  const startsAt = d.startsAt ?? nowIso();
  const endsAt =
    d.endsAt ??
    new Date(Date.parse(startsAt) + (d.days ?? AB_LIMITS.defaultDays) * 86400000).toISOString();
  return {
    openabtest: '0.1-draft',
    id: row.slug,
    revision: 1,
    name: d.name,
    state: row.state,
    cohort: { id: 'eligible', eligibilityRule: d.eligibilityRule, purchaseScope: 'custom' },
    assignment: {
      unit: d.unit,
      authority: 'server',
      keyVersion: '1',
      algorithm: ALGORITHM,
      persistence: 'sticky',
    },
    variants: d.variants,
    window: { startsAt, endsAt },
    metrics: {
      primary: 'conversion-rate',
      denominator: 'unique-eligible-participants',
      conversion: 'unique-participants-with-confirmed-purchase',
      profit: 'reconciled-net-retained-profit',
      minimumEligiblePerVariant: d.minimumEligiblePerVariant,
      minimumObservationSeconds: Math.round(d.minimumObservationDays * 86400),
      winnerPolicy: 'manual-review',
    },
    guardrails: {
      preserveAcceptedOffers: true,
      preserveAccruedCommissions: true,
      requireReconciledCostsForPayout: true,
      onBreach: 'pause-new-assignments',
    },
  };
}
function checkManifest(manifest) {
  const result = validate('openabtest-manifest', manifest);
  if (!result.ok)
    throw fail(
      'INVALID_EXPERIMENT',
      `Not a valid OpenABTest manifest: ${result.errors.map((x) => `${x.instancePath || '/'} ${x.message}`).join('; ')}`,
    );
}

// hmac-sha256-persisted-v1: HMAC-SHA256(secret, "<slug>\n<revision>\n<participant>"), the
// first 6 bytes as an unsigned integer, modulo the total weight, walked over the variants
// in manifest order. Only the first call computes it; the stored row is what counts.
export function bucketVariant(secret, slug, revision, participant, variants) {
  const digest = createHmac('sha256', secret)
    .update(`${slug}\n${revision}\n${participant}`)
    .digest();
  const total = variants.reduce((sum, v) => sum + v.weight, 0);
  let point = digest.readUIntBE(0, 6) % total;
  for (const v of variants) {
    if (point < v.weight) return v;
    point -= v.weight;
  }
  return variants[variants.length - 1];
}

// Wilson score interval, 95%.
export function wilson(successes, n) {
  if (!n) return null;
  const z = 1.959963984540054;
  const p = successes / n;
  const denom = 1 + (z * z) / n;
  const centre = (p + (z * z) / (2 * n)) / denom;
  const half = (z * Math.sqrt((p * (1 - p)) / n + (z * z) / (4 * n * n))) / denom;
  // At 0 or n successes the bound is exactly 0 or 1; floating point would put it a hair inside.
  return [
    successes === 0 ? 0 : Math.max(0, centre - half),
    successes === n ? 1 : Math.min(1, centre + half),
  ];
}
// Two-sided p-value of a pooled two-proportion z-test against the first variant.
export function twoProportionP(c1, n1, c2, n2) {
  if (!n1 || !n2) return null;
  const pooled = (c1 + c2) / (n1 + n2);
  const se = Math.sqrt(pooled * (1 - pooled) * (1 / n1 + 1 / n2));
  if (!se) return null;
  const z = Math.abs(c2 / n2 - c1 / n1) / se;
  // Abramowitz-Stegun 26.2.17 for the normal tail.
  const t = 1 / (1 + 0.2316419 * z);
  const tail =
    Math.exp((-z * z) / 2) *
    0.3989422804014327 *
    t *
    (0.31938153 + t * (-0.356563782 + t * (1.781477937 + t * (-1.821255978 + t * 1.330274429))));
  return Math.min(1, 2 * tail);
}

export function createAbTest(db) {
  const n = (value) => Number(value ?? 0);
  const experimentRow = async (userId, slug) => {
    const row = await db.get('SELECT * FROM ab_experiments WHERE user_id = ? AND slug = ?', [
      userId,
      String(slug ?? ''),
    ]);
    if (!row) throw fail('NO_SUCH_EXPERIMENT', `No experiment ${slug}.`, 404);
    return row;
  };
  const summary = (row) => {
    const manifest = toManifest(row);
    return {
      slug: row.slug,
      name: manifest.name,
      state: row.state,
      unit: manifest.assignment.unit,
      variants: manifest.variants,
      window:
        row.state === 'draft' && !JSON.parse(row.definition).startsAt ? null : manifest.window,
      created_at: row.created_at,
      updated_at: row.updated_at,
      started_at: row.started_at,
    };
  };

  // ---- keys
  async function createKey(userId, name) {
    const count = await db.get('SELECT COUNT(*) AS n FROM ab_keys WHERE user_id = ?', [userId]);
    if (n(count.n) >= AB_LIMITS.keys)
      throw fail('TOO_MANY_KEYS', `An account holds up to ${AB_LIMITS.keys} keys.`, 409);
    const key = `ab_${random(32)}`;
    const id = `k_${random(9)}`;
    await db.run('INSERT INTO ab_keys VALUES (?, ?, ?, ?, ?, NULL)', [
      sha256(key),
      id,
      userId,
      text(name, 60) || 'server',
      nowIso(),
    ]);
    return { id, key, name: text(name, 60) || 'server' };
  }
  const keys = async (userId) =>
    db.all(
      'SELECT id, name, created_at, last_used_at FROM ab_keys WHERE user_id = ? ORDER BY created_at',
      [userId],
    );
  async function removeKey(userId, id) {
    const row = await db.get('SELECT id FROM ab_keys WHERE user_id = ? AND id = ?', [userId, id]);
    if (!row) throw fail('NO_SUCH_KEY', 'No such key.', 404);
    await db.run('DELETE FROM ab_keys WHERE id = ?', [id]);
    return { removed: true };
  }
  async function keyUser(key) {
    if (typeof key !== 'string' || !key.startsWith('ab_') || key.length > 100) return null;
    const row = await db.get('SELECT user_id, last_used_at FROM ab_keys WHERE key_hash = ?', [
      sha256(key),
    ]);
    if (!row) return null;
    // At most one write a minute per key.
    if (!row.last_used_at || Date.now() - Date.parse(row.last_used_at) > 60000)
      await db.run('UPDATE ab_keys SET last_used_at = ? WHERE key_hash = ?', [
        nowIso(),
        sha256(key),
      ]);
    return row.user_id;
  }

  // ---- experiments
  async function create(userId, input = {}) {
    const slug = text(input.slug, 64).toLowerCase();
    if (!SLUG.test(slug))
      throw fail(
        'INVALID_EXPERIMENT',
        'The id must be 1-64 lowercase letters, digits or dashes, starting with a letter or digit.',
      );
    const count = await db.get('SELECT COUNT(*) AS n FROM ab_experiments WHERE user_id = ?', [
      userId,
    ]);
    if (n(count.n) >= AB_LIMITS.experiments)
      throw fail(
        'TOO_MANY_EXPERIMENTS',
        `An account holds up to ${AB_LIMITS.experiments} experiments. Delete a closed one first.`,
        409,
      );
    if (
      await db.get('SELECT id FROM ab_experiments WHERE user_id = ? AND slug = ?', [userId, slug])
    )
      throw fail('EXPERIMENT_EXISTS', `You already have an experiment ${slug}.`, 409);
    const row = {
      id: `x_${random(9)}`,
      user_id: userId,
      slug,
      state: 'draft',
      definition: JSON.stringify(cleanDefinition(input)),
      secret: random(32),
      created_at: nowIso(),
      updated_at: nowIso(),
      started_at: null,
    };
    checkManifest(toManifest(row));
    await db.run('INSERT INTO ab_experiments VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)', [
      row.id,
      row.user_id,
      row.slug,
      row.state,
      row.definition,
      row.secret,
      row.created_at,
      row.updated_at,
      row.started_at,
    ]);
    return summary(row);
  }
  const list = async (userId) =>
    (
      await db.all('SELECT * FROM ab_experiments WHERE user_id = ? ORDER BY created_at DESC', [
        userId,
      ])
    ).map(summary);
  async function update(userId, slug, input = {}) {
    const row = await experimentRow(userId, slug);
    // Spec: variants, weights, eligibility and metrics are immutable within a revision.
    if (row.state !== 'draft')
      throw fail(
        'EXPERIMENT_FROZEN',
        'Only a draft can be edited. A started experiment keeps its definition; make a new one to test something else.',
        409,
      );
    const definition = JSON.stringify(cleanDefinition(input, JSON.parse(row.definition)));
    const next = { ...row, definition, updated_at: nowIso() };
    checkManifest(toManifest(next));
    await db.run('UPDATE ab_experiments SET definition = ?, updated_at = ? WHERE id = ?', [
      definition,
      next.updated_at,
      row.id,
    ]);
    return summary(next);
  }
  async function setState(userId, slug, state) {
    const row = await experimentRow(userId, slug);
    if (!STATES.includes(state)) throw fail('INVALID_STATE', `Unknown state ${state}.`);
    if (state === row.state) return summary(row);
    if (!MOVES[row.state].includes(state))
      throw fail('INVALID_STATE', `A ${row.state} experiment cannot become ${state}.`, 409);
    const now = nowIso();
    let definition = row.definition;
    let startedAt = row.started_at;
    if (row.state === 'draft' && state === 'running') {
      // Starting fixes the window for good.
      const d = JSON.parse(definition);
      d.startsAt ??= now;
      d.endsAt ??= new Date(
        Date.parse(d.startsAt) + (d.days ?? AB_LIMITS.defaultDays) * 86400000,
      ).toISOString();
      if (Date.parse(d.endsAt) <= Date.now())
        throw fail('INVALID_WINDOW', 'The window has already ended. Edit it before starting.', 409);
      definition = JSON.stringify(d);
      startedAt = now;
    }
    await db.run(
      'UPDATE ab_experiments SET state = ?, definition = ?, started_at = ?, updated_at = ? WHERE id = ?',
      [state, definition, startedAt, now, row.id],
    );
    return summary({ ...row, state, definition, started_at: startedAt, updated_at: now });
  }
  async function remove(userId, slug) {
    const row = await experimentRow(userId, slug);
    await db.transaction(async (tx) => {
      await tx.run('DELETE FROM ab_events WHERE experiment_id = ?', [row.id]);
      await tx.run('DELETE FROM ab_assignments WHERE experiment_id = ?', [row.id]);
      await tx.run('DELETE FROM ab_experiments WHERE id = ?', [row.id]);
    });
    return { removed: true, slug: row.slug };
  }

  // ---- producer calls
  const assignmentOf = (row, participant) =>
    db.get('SELECT * FROM ab_assignments WHERE experiment_id = ? AND participant_id = ?', [
      row.id,
      participant,
    ]);
  const answer = (row, a, fresh) => {
    const variant = toManifest(row).variants.find((v) => v.id === a.variant_id);
    return {
      experiment: row.slug,
      revision: a.revision,
      state: row.state,
      participant: a.participant_id,
      assignmentId: a.id,
      variant: a.variant_id,
      parameters: variant?.parameters ?? {},
      new: fresh,
    };
  };
  async function insertEvent(tx, row, event) {
    const body = JSON.stringify(event.body);
    const existing = await tx.get('SELECT body FROM ab_events WHERE experiment_id = ? AND id = ?', [
      row.id,
      event.id,
    ]);
    if (existing) {
      if (existing.body !== body)
        throw fail(
          'EVENT_CONFLICT',
          `Event ${event.id} was already recorded with other content.`,
          409,
        );
      return false;
    }
    if (event.conversion_id) {
      const same = await tx.get(
        'SELECT id FROM ab_events WHERE experiment_id = ? AND conversion_id = ?',
        [row.id, event.conversion_id],
      );
      if (same) {
        // The same conversion under a new callback id is a retry, not a second purchase.
        const prior = await tx.get(
          'SELECT body FROM ab_events WHERE experiment_id = ? AND id = ?',
          [row.id, same.id],
        );
        const strip = (b) => JSON.stringify({ ...JSON.parse(b), id: undefined });
        if (strip(prior.body) !== strip(body))
          throw fail(
            'EVENT_CONFLICT',
            `Conversion ${event.conversion_id} was already recorded with other content.`,
            409,
          );
        return false;
      }
    }
    await tx.run('INSERT INTO ab_events VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)', [
      row.id,
      event.id,
      event.kind,
      event.participant_id,
      event.assignment_id,
      event.variant_id,
      event.surface ?? null,
      event.offer_id ?? null,
      event.conversion_id ?? null,
      event.value_minor ?? null,
      event.currency ?? null,
      event.price ? JSON.stringify(event.price) : null,
      body,
      event.at,
      nowIso(),
    ]);
    return true;
  }

  async function assign(userId, { experiment, participant } = {}) {
    const row = await experimentRow(userId, experiment);
    const pid = participantId(participant);
    const existing = await assignmentOf(row, pid);
    // Paused and closed experiments still honour assignments already made.
    if (existing) return answer(row, existing, false);
    const manifest = toManifest(row);
    const now = Date.now();
    const reason =
      row.state !== 'running'
        ? `the experiment is ${row.state}`
        : now < Date.parse(manifest.window.startsAt)
          ? 'the experiment window has not started'
          : now >= Date.parse(manifest.window.endsAt)
            ? 'the experiment window has ended'
            : null;
    if (reason)
      return {
        experiment: row.slug,
        state: row.state,
        participant: pid,
        variant: null,
        parameters: null,
        reason: `Not enrolled: ${reason}. Show your default experience.`,
      };
    const variant = bucketVariant(row.secret, row.slug, 1, pid, manifest.variants);
    const at = nowIso();
    const a = {
      experiment_id: row.id,
      participant_id: pid,
      id: `a_${random(12)}`,
      revision: 1,
      variant_id: variant.id,
      created_at: at,
    };
    const created = await db
      .transaction(async (tx) => {
        // The primary key is the uniqueness constraint; a racing first call loses here.
        const race = await tx.get(
          'SELECT * FROM ab_assignments WHERE experiment_id = ? AND participant_id = ?',
          [row.id, pid],
        );
        if (race) return null;
        await tx.run('INSERT INTO ab_assignments VALUES (?, ?, ?, ?, ?, ?)', [
          a.experiment_id,
          a.participant_id,
          a.id,
          a.revision,
          a.variant_id,
          a.created_at,
        ]);
        for (const kind of ['eligibility', 'assignment'])
          await insertEvent(tx, row, {
            id: `${kind}:${a.id}`,
            kind,
            participant_id: pid,
            assignment_id: a.id,
            variant_id: a.variant_id,
            at,
            body: { kind },
          });
        return a;
      })
      .catch(async (error) => {
        // Postgres reports the race as a unique violation instead.
        if (error?.code === '23505' || /UNIQUE/i.test(error?.message ?? '')) return null;
        throw error;
      });
    return created ? answer(row, created, true) : answer(row, await assignmentOf(row, pid), false);
  }

  async function assigned(userId, { experiment, participant, variant }) {
    const row = await experimentRow(userId, experiment);
    const pid = participantId(participant);
    const a = await assignmentOf(row, pid);
    if (!a)
      throw fail(
        'NOT_ASSIGNED',
        `Participant ${pid} has no assignment in ${row.slug}. Call assign first, and only report what an enrolled participant did.`,
        409,
      );
    if (variant !== undefined && variant !== a.variant_id)
      throw fail(
        'WRONG_VARIANT',
        `Participant ${pid} is assigned ${a.variant_id}, not ${variant}.`,
        409,
      );
    return { row, a };
  }

  async function exposure(userId, input = {}) {
    const { row, a } = await assigned(userId, input);
    const surface = text(input.surface, 64) || 'default';
    const offer = input.offerId === undefined ? a.id : opaqueId(input.offerId, 'offerId');
    const id =
      input.id === undefined ? `exposure:${a.id}:${surface}:${offer}` : opaqueId(input.id, 'id');
    const at = eventTime(input.at);
    // A retry without an explicit time is the same exposure, so the time is not part of
    // what a duplicate must match unless the producer sent one.
    const fresh = await db.transaction((tx) =>
      insertEvent(tx, row, {
        id,
        kind: 'exposure',
        participant_id: a.participant_id,
        assignment_id: a.id,
        variant_id: a.variant_id,
        surface,
        offer_id: offer,
        at,
        body: { kind: 'exposure', surface, offer, ...(input.at === undefined ? {} : { at }) },
      }),
    );
    return { recorded: fresh, duplicate: !fresh, id, variant: a.variant_id };
  }

  async function conversion(userId, input = {}) {
    const { row, a } = await assigned(userId, input);
    const conversionId =
      input.conversionId === undefined
        ? `conversion:${a.participant_id}`
        : opaqueId(input.conversionId, 'conversionId');
    const id = input.id === undefined ? `conversion:${conversionId}` : opaqueId(input.id, 'id');
    let value = null;
    let currency = null;
    let price = null;
    if (input.price !== undefined) {
      // A purchase: the OpenABTest price block, checked by the reference validator.
      price = input.price;
      const check = validate('openabtest-event', {
        openabtest: '0.1-draft',
        id,
        producerId: PRODUCER_ID,
        manifestId: row.slug,
        manifestRevision: a.revision,
        participantId: a.participant_id,
        at: nowIso(),
        kind: 'conversion',
        payload: { assignmentId: a.id, variantId: a.variant_id, conversionId, price },
      });
      if (!check.ok)
        throw fail(
          'INVALID_PRICE',
          `Not a valid OpenABTest price: ${check.errors.map((x) => `${x.instancePath || '/'} ${x.message}`).join('; ')}`,
        );
      value = price.totalMinor;
      currency = price.currency;
    } else if (input.valueMinor !== undefined) {
      value = Number(input.valueMinor);
      if (!Number.isSafeInteger(value) || value < 0)
        throw fail('INVALID_VALUE', 'valueMinor must be a whole number of minor units, 0 or more.');
      currency = text(input.currency, 3);
      if (!CURRENCY.test(currency))
        throw fail('INVALID_VALUE', 'Give a three-letter currency code with valueMinor.');
    }
    const at = eventTime(input.at);
    const fresh = await db.transaction((tx) =>
      insertEvent(tx, row, {
        id,
        kind: 'conversion',
        participant_id: a.participant_id,
        assignment_id: a.id,
        variant_id: a.variant_id,
        conversion_id: conversionId,
        value_minor: value,
        currency,
        price,
        at,
        body: { kind: 'conversion', id, conversionId, value, currency, price },
      }),
    );
    return { recorded: fresh, duplicate: !fresh, id, conversionId, variant: a.variant_id };
  }

  // ---- results
  async function results(userId, slug) {
    const row = await experimentRow(userId, slug);
    const manifest = toManifest(row);
    const counts = async (sql) =>
      Object.fromEntries((await db.all(sql, [row.id])).map((r) => [r.variant_id, n(r.n)]));
    const assignedBy = await counts(
      'SELECT variant_id, COUNT(*) AS n FROM ab_assignments WHERE experiment_id = ? GROUP BY variant_id',
    );
    const exposedBy = await counts(
      "SELECT variant_id, COUNT(DISTINCT participant_id) AS n FROM ab_events WHERE experiment_id = ? AND kind = 'exposure' GROUP BY variant_id",
    );
    const convertedBy = await counts(
      "SELECT variant_id, COUNT(DISTINCT participant_id) AS n FROM ab_events WHERE experiment_id = ? AND kind = 'conversion' GROUP BY variant_id",
    );
    const unexposedBy = await counts(
      "SELECT c.variant_id, COUNT(DISTINCT c.participant_id) AS n FROM ab_events c WHERE c.experiment_id = ? AND c.kind = 'conversion' AND NOT EXISTS (SELECT 1 FROM ab_events x WHERE x.experiment_id = c.experiment_id AND x.kind = 'exposure' AND x.participant_id = c.participant_id) GROUP BY c.variant_id",
    );
    const values = await db.all(
      "SELECT variant_id, currency, SUM(value_minor) AS total, COUNT(*) AS n FROM ab_events WHERE experiment_id = ? AND kind = 'conversion' AND value_minor IS NOT NULL GROUP BY variant_id, currency",
      [row.id],
    );
    const conversions = await counts(
      "SELECT variant_id, COUNT(*) AS n FROM ab_events WHERE experiment_id = ? AND kind = 'conversion' GROUP BY variant_id",
    );
    const floor = manifest.metrics.minimumEligiblePerVariant;
    const control = manifest.variants[0].id;
    const c0 = convertedBy[control] ?? 0;
    const n0 = assignedBy[control] ?? 0;
    const variants = manifest.variants.map((v) => {
      const assignedN = assignedBy[v.id] ?? 0;
      const converted = convertedBy[v.id] ?? 0;
      const rate = assignedN ? converted / assignedN : null;
      return {
        id: v.id,
        weight: v.weight,
        parameters: v.parameters,
        assigned: assignedN,
        exposed: exposedBy[v.id] ?? 0,
        converted,
        convertedWithoutExposure: unexposedBy[v.id] ?? 0,
        conversions: conversions[v.id] ?? 0,
        value: values
          .filter((x) => x.variant_id === v.id)
          .map((x) => ({ currency: x.currency, totalMinor: n(x.total), conversions: n(x.n) })),
        conversionRate: rate,
        interval95: wilson(converted, assignedN),
        vsControl:
          v.id === control || rate === null || !n0
            ? null
            : { difference: rate - c0 / n0, pValue: twoProportionP(c0, n0, converted, assignedN) },
        floorMet: assignedN >= floor,
      };
    });
    const observed = row.started_at
      ? Math.floor((Date.now() - Date.parse(row.started_at)) / 1000)
      : 0;
    const total = variants.reduce((s, v) => s + v.assigned, 0);
    return {
      experiment: summary(row),
      manifestRevision: 1,
      unit: manifest.assignment.unit,
      control,
      denominator: 'unique participants with a stored assignment',
      totals: {
        assigned: total,
        exposed: variants.reduce((s, v) => s + v.exposed, 0),
        converted: variants.reduce((s, v) => s + v.converted, 0),
      },
      variants,
      floors: {
        minimumEligiblePerVariant: floor,
        minimumObservationSeconds: manifest.metrics.minimumObservationSeconds,
        observedSeconds: observed,
        met:
          variants.every((v) => v.floorMet) &&
          observed >= manifest.metrics.minimumObservationSeconds,
      },
      winnerPolicy: 'manual-review',
      note: total
        ? 'Not a decision. Meeting the floors is not statistical significance; p-values are a pooled two-proportion z-test against the first variant, without correction for repeated looks. A person decides.'
        : 'No data yet.',
      at: nowIso(),
    };
  }

  // ---- OpenABTest export
  async function manifest(userId, slug) {
    return toManifest(await experimentRow(userId, slug));
  }
  async function exportEvents(userId, slug) {
    const row = await experimentRow(userId, slug);
    const rows = await db.all(
      `SELECT * FROM ab_events WHERE experiment_id = ? ORDER BY at, id LIMIT ${AB_LIMITS.eventsExport}`,
      [row.id],
    );
    const events = [];
    let withoutPrice = 0;
    for (const r of rows) {
      // OpenABTest 0.1 conversions are purchases with a price; goal-only conversions stay
      // in the W3BS results and are counted here instead.
      if (r.kind === 'conversion' && !r.price) {
        withoutPrice++;
        continue;
      }
      const bound = { assignmentId: r.assignment_id, variantId: r.variant_id };
      const payload = {
        eligibility: () => ({ cohortId: 'eligible', eligible: true }),
        assignment: () => ({ cohortId: 'eligible', ...bound }),
        exposure: () => ({ ...bound, offerId: r.offer_id, surface: r.surface }),
        conversion: () => ({ ...bound, conversionId: r.conversion_id, price: JSON.parse(r.price) }),
      }[r.kind]();
      events.push({
        openabtest: '0.1-draft',
        id: r.id,
        producerId: PRODUCER_ID,
        manifestId: row.slug,
        manifestRevision: 1,
        participantId: r.participant_id,
        at: r.at,
        kind: r.kind,
        payload,
      });
    }
    return {
      manifest: toManifest(row),
      events,
      omitted: { conversionsWithoutPrice: withoutPrice },
      truncated: rows.length >= AB_LIMITS.eventsExport,
    };
  }

  return {
    createKey,
    keys,
    removeKey,
    keyUser,
    create,
    list,
    update,
    setState,
    remove,
    assign,
    exposure,
    conversion,
    results,
    manifest,
    exportEvents,
  };
}
