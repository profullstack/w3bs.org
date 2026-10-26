// W3BS records -> liquid-agent/v1 EpisodeV1, with secrets masked at the source.
//
// Two inputs: a step captured by a Teach session (already semantic: app, url pattern, focus,
// visible elements, action) and a format-0.1 episode from the existing Training store
// (getEpisode output: steps + observations). Both go through maskStep, which is the only
// place text survives or not:
//   - a secret field (password, passcode, PIN, OTP, token, secret, card, CVV ...) never
//     carries text: the text is dropped and text_redacted is set;
//   - the strict profile (the default) drops every typed value; standard keeps typed text
//     only when the secret scan finds nothing in it;
//   - element names the secret scan flags become "[redacted]";
//   - no credentials, cookies or raw screenshots: image_ref is a sha256 or nothing, and only
//     when the person chose screenshots for the session.
import { ACTION_TYPES } from './contract.mjs';
import { sealEpisode } from './dataset.mjs';
import { scanText } from './secrets.mjs';
import { assertValid } from './validate.mjs';

export const SECRET_FIELD =
  /pass(word|code|phrase)?|passwd|\bpin\b|\botp\b|one[- ]?time|verification code|security code|2fa|mfa|auth(entication)? code|token|secret|api[ _-]?key|private key|card|cvv|cvc|expiry|iban|routing|account number|\bssn\b|social security/i;
const SECRET_ROLES = new Set(['password', 'passwordfield', 'password_text']);

export const isSecretField = (element) =>
  !!element &&
  (SECRET_ROLES.has(String(element.role).toLowerCase()) || SECRET_FIELD.test(element.name ?? ''));

const short = (s, n) => String(s ?? '').slice(0, n);
const cleanName = (name) => (scanText(String(name ?? '')).length ? '[redacted]' : short(name, 160));
const cleanElement = (e) =>
  e && e.role
    ? {
        role: short(e.role, 64),
        name: cleanName(e.name),
        ...(e.id ? { id: short(e.id, 128) } : {}),
      }
    : undefined;

// URL -> a pattern with no query, fragment, credentials or long ids.
export function urlPattern(url) {
  if (!url) return undefined;
  const s = String(url);
  try {
    const u = new URL(s);
    const path = u.pathname
      .split('/')
      .map((p) => (/^[0-9a-f-]{8,}$/i.test(p) || /^\d{3,}$/.test(p) ? '*' : p))
      .join('/');
    return short(
      `${u.protocol === 'http:' || u.protocol === 'https:' ? '' : `${u.protocol}//`}${u.host}${path}`,
      256,
    );
  } catch {
    return short(s.split(/[?#]/)[0].replace(/\/\/[^/@]*@/, '//'), 256);
  }
}

export function maskObservation(obs, { screenshots = false } = {}) {
  const out = { app: short(obs?.app || 'unknown', 64) || 'unknown' };
  const url = obs?.url_pattern ?? urlPattern(obs?.url);
  if (url) out.url_pattern = short(url, 256);
  if (obs?.ui_tree_ref !== undefined)
    out.ui_tree_ref = /^sha256:[0-9a-f]{64}$/.test(obs.ui_tree_ref ?? '') ? obs.ui_tree_ref : null;
  if (screenshots && /^sha256:[0-9a-f]{64}$/.test(obs?.image_ref ?? ''))
    out.image_ref = obs.image_ref;
  const focus = cleanElement(obs?.focus);
  if (focus) out.focus = focus;
  if (Array.isArray(obs?.elements))
    out.elements = obs.elements.slice(0, 256).map(cleanElement).filter(Boolean);
  if (obs?.loading) out.loading = true;
  return out;
}

export function maskAction(action, { observation, profile = 'strict' } = {}) {
  const type = ACTION_TYPES.includes(action?.type) ? action.type : null;
  if (!type) {
    const err = new Error(`action type ${action?.type} is not in liquid-agent/v1`);
    err.code = 'E_SCHEMA';
    err.status = 422;
    throw err;
  }
  const out = { type };
  const target = cleanElement(action.target);
  if (target) out.target = target;
  if (action.key) out.key = short(action.key, 64);
  if (action.url_pattern || action.url)
    out.url_pattern = action.url_pattern ? short(action.url_pattern, 256) : urlPattern(action.url);
  if (typeof action.amount === 'number' && Number.isFinite(action.amount))
    out.amount = action.amount;
  const hadText = typeof action.text === 'string' && action.text.length > 0;
  if (hadText || action.text_redacted) {
    // The field typed into: the target, or the focused control when there is no target.
    const secret = isSecretField(action.target ?? observation?.focus);
    const keep = hadText && !secret && profile === 'standard' && scanText(action.text).length === 0;
    if (keep) out.text = short(action.text, 2000);
    else out.text_redacted = true;
  }
  return out;
}

export const maskStep = (step, { profile = 'strict', screenshots = false } = {}) => {
  const observation = maskObservation(step.observation, { screenshots });
  return {
    t_ms: Math.max(0, Math.round(step.t_ms ?? 0)),
    observation,
    action: maskAction(step.action, { observation: step.observation, profile }),
    result: {
      status: ['ok', 'error', 'blocked', 'skipped'].includes(step.result?.status)
        ? step.result.status
        : 'ok',
      ...(step.result?.error ? { error: short(step.result.error, 500) } : {}),
    },
  };
};

export const taskId = (value) => {
  const slug = String(value ?? '')
    .toLowerCase()
    .replace(/[^a-z0-9.]+/g, '_')
    .replace(/^[._]+|[._]+$/g, '')
    .replace(/\.{2,}/g, '.');
  if (/^[a-z0-9_]+(\.[a-z0-9_]+)+$/.test(slug)) return slug.slice(0, 96);
  return `w3bs.${slug.replace(/\./g, '_') || 'recorded'}`.slice(0, 96);
};

export const episodeIdFor = (id) => {
  const core = String(id)
    .replace(/^ep_/, '')
    .replace(/[^A-Za-z0-9_-]/g, '_');
  return `ep_${core.padEnd(4, '0').slice(0, 64)}`;
};

/** Build and seal one EpisodeV1. Throws E_SCHEMA when the result is not valid. */
export function toEpisodeV1({
  episode_id,
  task,
  created_at,
  split_group,
  variant,
  consent_id,
  profile = 'strict',
  source = 'w3bs-desktop',
  app_scope,
  screenshots = false,
  steps,
  labels = {},
}) {
  const episode = sealEpisode({
    episode_id: episodeIdFor(episode_id),
    task: taskId(task),
    created_at,
    ...(split_group ? { split_group: short(split_group, 128) } : {}),
    ...(variant ? { variant: short(variant, 64) } : {}),
    capture: {
      consent_id: short(consent_id, 128),
      redaction_profile: profile,
      device_local: true,
      source,
      ...(app_scope?.length ? { app_scope: app_scope.slice(0, 32).map((a) => short(a, 64)) } : {}),
    },
    steps: steps.slice(0, 512).map((s) => maskStep(s, { profile, screenshots })),
    labels: {
      success: labels.success === true,
      reviewed: labels.reviewed === true,
      risk: ['low', 'medium', 'high'].includes(labels.risk) ? labels.risk : 'low',
      ...(labels.goal ? { goal: short(labels.goal, 500) } : {}),
      ...(labels.failure_reason ? { failure_reason: short(labels.failure_reason, 500) } : {}),
    },
  });
  return assertValid(episode, 'episode');
}

// ------------------------------------------------------------ format 0.1 (Training store)
const FORMAT01_ACTIONS = {
  click: 'click',
  double_click: 'double_click',
  right_click: 'click',
  scroll: 'scroll',
  key_shortcut: 'key',
  text_edit: 'type',
  focus: 'focus',
  navigate: 'navigate',
  wait: 'wait',
  hover: 'focus',
};

/**
 * A format-0.1 episode (the `getEpisode` result of the Training service) as EpisodeV1.
 * Visible elements are what format 0.1 knows: the action target and the focused control.
 * Steps whose action has no liquid-agent/v1 equivalent (drag, other) are dropped and counted.
 */
export function fromFormat01(detail, { consent_id, profile = 'strict', task, risk = 'low' } = {}) {
  const { episode, steps, observations = [], state } = detail;
  const obsById = new Map(observations.map((o) => [o.observation_id, o]));
  const first = steps.length ? BigInt(steps[0].action.monotonic_ns) : 0n;
  let dropped = 0;
  const out = [];
  for (const s of steps) {
    const type = FORMAT01_ACTIONS[s.action.type];
    if (!type) {
      dropped += 1;
      continue;
    }
    const sem = obsById.get(s.observation_before)?.semantic ?? {};
    const t = s.action.target ?? {};
    const target = t.role && t.name ? { role: t.role, name: t.name } : undefined;
    const focus =
      sem.focused?.role && sem.focused?.name
        ? { role: sem.focused.role, name: sem.focused.name }
        : undefined;
    const elements = [target, focus]
      .filter(Boolean)
      .filter((e, i, a) => a.findIndex((x) => x.role === e.role && x.name === e.name) === i);
    const args = s.action.arguments ?? {};
    out.push({
      t_ms: Number((BigInt(s.action.monotonic_ns) - first) / 1000000n),
      observation: {
        app: sem.app ?? t.app ?? 'unknown',
        ...(sem.url_origin ? { url_pattern: urlPattern(sem.url_origin) } : {}),
        ...(focus ? { focus } : {}),
        elements,
      },
      action: {
        type,
        ...(type !== 'wait' && type !== 'scroll' && target ? { target } : {}),
        ...(type === 'key' && args.keys ? { key: args.keys.join('+') } : {}),
        ...(type === 'scroll' && typeof args.delta_y === 'number' ? { amount: args.delta_y } : {}),
        ...(type === 'navigate' && args.url ? { url: args.url } : {}),
        ...(type === 'type'
          ? args.text !== undefined
            ? { text: args.text }
            : { text_redacted: true }
          : {}),
      },
      result: { status: s.action.status === 'rejected' ? 'skipped' : 'ok' },
    });
  }
  if (!out.length) {
    const err = new Error('This episode has no steps liquid-agent/v1 can express.');
    err.code = 'INVALID_STATE';
    err.status = 409;
    throw err;
  }
  const apps = [...new Set(out.map((s) => s.observation.app))];
  return {
    dropped,
    episode: toEpisodeV1({
      episode_id: episode.episode_id,
      task: task ?? episode.workflow_family ?? episode.goal?.value,
      created_at: detail.captured_from ?? new Date().toISOString(),
      split_group: episode.segments?.[0]?.session_id,
      consent_id,
      profile,
      source: 'import',
      app_scope: apps,
      steps: out,
      labels: {
        goal: typeof episode.goal?.value === 'string' ? episode.goal.value : undefined,
        success: episode.outcome?.value === 'success',
        reviewed: false,
        risk,
      },
    }),
  };
}
