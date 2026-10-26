// The guarded executor: the only code that turns a policy's ActionProposalV1 into an action.
// A proposal never authorises anything. Before every action the executor re-checks, against
// the CURRENT observation:
//   - the kill switch (read fresh from the store, so it takes effect before the next action);
//   - the action is in liquid-agent/v1 and the proposal validates;
//   - the session is not stale (sequence index, TTL, the observation did not change under it);
//   - the proposal's model hash is the activated model's hash;
//   - the target is visible now;
//   - step count and wall-time bounds;
//   - the mode: Suggest never executes; Execute with approval asks for every action; Bounded
//     runs only inside its authorised bounds; and EVERY external write (submit, send, post,
//     publish, delete, or a click on a control that does one) waits for a person's approval in
//     every mode, Bounded included.
//
// Executors: the only ones in this change are the mock sites (training/liquid/mock.mjs). The
// Agentic Desktop Layer seam is `desktopLayerEnvironment` below; it refuses until that work
// lands, so nothing here can touch a real application or site.
import { canonicalize, sha256Hex } from './canonical.mjs';
import { ACTION_TYPES, EXTERNAL_WRITE_ACTIONS, SCHEMA_VERSION } from './contract.mjs';
import { maskObservation } from './convert.mjs';
import { EXTERNAL_CONTROL } from './mock.mjs';
import { validate } from './validate.mjs';

export const MODES = ['off', 'suggest', 'approve', 'bounded'];
export const MODE_LABELS = {
  off: 'Off',
  suggest: 'Suggest',
  approve: 'Execute with approval',
  bounded: 'Bounded automation',
};
const TARGETED = new Set([
  'click',
  'double_click',
  'type',
  'select',
  'focus',
  'submit',
  'send',
  'post',
  'publish',
  'delete',
]);

export const isExternalEffect = (action, proposal) =>
  !!action &&
  (EXTERNAL_WRITE_ACTIONS.includes(action.type) ||
    (['click', 'double_click', 'key'].includes(action.type) &&
      EXTERNAL_CONTROL.test(action.target?.name ?? '')) ||
    proposal?.requires_approval === true);

const same = (a, b) => a.role === b.role && a.name === b.name;

/** The checks that need no person. Returns null when the proposal may proceed, else a reason. */
export function checkProposal(
  proposal,
  { request, observation, activeModelHash, stepIndex, maxSteps, startedAt, timeoutMs, now },
) {
  if (!validate(proposal, 'action_proposal').ok) return 'out_of_schema';
  if (
    proposal.session_id !== request.session_id ||
    proposal.sequence_index !== request.sequence_index
  )
    return 'stale_session';
  if (proposal.model_hash !== activeModelHash) return 'model_hash_mismatch';
  if (proposal.abstain_reason) return `abstained:${proposal.abstain_reason}`;
  const action = proposal.action;
  if (!action || !ACTION_TYPES.includes(action.type)) return 'out_of_schema';
  if (canonicalize(observation) !== canonicalize(request.observation)) return 'stale_observation';
  if (TARGETED.has(action.type)) {
    if (!action.target) return 'unknown_target';
    if (!(observation.elements ?? []).some((e) => same(e, action.target)))
      return 'target_not_visible';
  }
  if (stepIndex >= maxSteps) return 'step_limit';
  if (now() - startedAt > timeoutMs) return 'timeout';
  return null;
}

/** Bounded automation: what a person authorised to run without asking (never external writes). */
export function withinBounds(action, bounds = {}) {
  const types = bounds.action_types ?? ['click', 'scroll', 'wait', 'type', 'focus', 'done'];
  return types.includes(action.type) && !isExternalEffect(action);
}

/**
 * runPolicy: closed loop over one environment.
 *
 * policy   { open(scope) -> session, predict(session, request) -> proposal, reset(session, scope) }
 * gate     { killSwitch(), activeModelHash(), permitted(mode) } (read fresh before each action)
 * approve  async (ask) -> { approved, by, approval_id }  a person answers each approval
 * human    async (observation, proposal) -> action | null  the person acting (Suggest mode,
 *          and take-over after an abstention, a failed check or a denial). null stops the run.
 * audit    async (kind, detail)
 */
export async function runPolicy({
  env,
  policy,
  task,
  mode,
  gate,
  approve,
  human,
  audit = async () => {},
  bounds = {},
  maxSteps = 40,
  timeoutMs = 120000,
  now = () => Date.now(),
  dtFor = (action) => (action?.type === 'wait' ? 1000 : action?.type === 'type' ? 2500 : 700),
  onObserve = () => {},
  maxErrorsInRow = 3,
}) {
  if (!MODES.includes(mode) || mode === 'off')
    throw Object.assign(new Error('The policy is Off for this task.'), {
      code: 'POLICY_OFF',
      status: 409,
    });
  const report = {
    task,
    mode,
    success: false,
    autonomous: false,
    steps: 0,
    executed: [],
    proposals: 0,
    interventions: 0,
    approvals_requested: 0,
    approvals_granted: 0,
    denials: 0,
    latency_ms: [],
    failures: {},
    corrections: [],
    trace: [],
    session_resets: 0,
    stopped_by: null,
  };
  const failure = (reason) => (report.failures[reason] = (report.failures[reason] ?? 0) + 1);
  const startedAt = now();
  // Sessions are scoped to an opaque hash of the account, never the account name itself.
  const scopeOf = (account) => `acct_${sha256Hex(String(account ?? 'default')).slice(0, 24)}`;
  let account = env.account;
  let scope = scopeOf(account);
  let session = await policy.open(scope);
  let seq = 0;
  let prev = null;
  let dt = 0;
  let tTotal = 0;
  let errorsInRow = 0;

  const stop = async (reason) => {
    report.stopped_by = reason;
    await audit('run.stopped', { task, reason, step: report.steps });
  };

  // Take a step as the person: Suggest mode, or a take-over. Recorded as a correction when it
  // differs from what the policy proposed.
  const personActs = async (observation, proposal) => {
    const action = await human(observation, proposal);
    if (!action) return null;
    if (await gate.killSwitch()) return 'killed';
    const result = env.apply(action, { approval: isExternalEffect(action) ? 'person' : undefined });
    if (!proposal?.action || canonicalize(proposal.action) !== canonicalize(action))
      report.corrections.push({
        sequence_index: seq,
        observation,
        proposed: proposal?.action ?? null,
        corrected: action,
      });
    await audit('action.by_person', { task, sequence_index: seq, action, result: result.status });
    return { action, result };
  };

  try {
    while (!env.finished) {
      if (await gate.killSwitch()) {
        await stop('kill_switch');
        break;
      }
      if (report.steps >= maxSteps) {
        failure('step_limit');
        await stop('step_limit');
        break;
      }
      if (now() - startedAt > timeoutMs) {
        failure('timeout');
        await stop('timeout');
        break;
      }
      // A different account means a different person's state: never carry it over.
      if (env.account !== account) {
        await policy.close(session, scope).catch(() => {});
        account = env.account;
        scope = scopeOf(account);
        session = await policy.open(scope);
        seq = 0;
        prev = null;
        report.session_resets += 1;
        await audit('session.reset', { task, reason: 'account_switch' });
      }
      const observation = maskObservation(env.observe());
      onObserve(observation);
      const request = {
        schema_version: SCHEMA_VERSION,
        kind: 'predict_request',
        session_id: session.session_id,
        model_hash: session.model_hash,
        sequence_index: seq,
        dt_ms: dt,
        task,
        observation,
        prev_action: prev,
      };
      let proposal;
      const t0 = now();
      try {
        proposal = await policy.predict(session, request);
      } catch (error) {
        // Duplicate, late or skipped sequence ids and unknown/expired sessions are surfaced,
        // never retried silently: the state behind them cannot be trusted.
        const reason = [
          'E_SEQ_DUPLICATE',
          'E_SEQ_LATE',
          'E_SEQ_GAP',
          'E_SESSION_UNKNOWN',
          'E_SESSION_EXPIRED',
        ].includes(error.code)
          ? 'stale_session'
          : 'runtime_error';
        failure(reason);
        await audit('proposal.error', {
          task,
          sequence_index: seq,
          code: error.code,
          message: error.message,
        });
        await stop(reason);
        report.error = { code: error.code, message: error.message };
        break;
      }
      report.latency_ms.push(proposal.latency_ms ?? now() - t0);
      report.proposals += 1;
      await audit('proposal', {
        task,
        sequence_index: seq,
        model_hash: proposal.model_hash,
        action: proposal.action,
        confidence: proposal.confidence,
        requires_approval: proposal.requires_approval,
        abstain_reason: proposal.abstain_reason,
      });
      report.steps += 1;
      // Everything below decides against the screen as it is NOW.
      const current = maskObservation(env.observe());
      const reason = checkProposal(proposal, {
        request,
        observation: current,
        activeModelHash: await gate.activeModelHash(),
        stepIndex: report.steps - 1,
        maxSteps,
        startedAt,
        timeoutMs,
        now,
      });
      let taken = null;
      if (reason) {
        failure(reason);
        await audit('proposal.rejected', { task, sequence_index: seq, reason });
        if (['stale_session', 'model_hash_mismatch'].includes(reason)) {
          await stop(reason);
          break;
        }
        report.interventions += 1;
        taken = await personActs(current, proposal);
      } else if (mode === 'suggest') {
        // Suggest never executes: the person sees the proposal and acts (or not) themselves.
        taken = await personActs(current, proposal);
      } else {
        const action = proposal.action;
        const external = isExternalEffect(action, proposal);
        const needsPerson = external || mode === 'approve' || !withinBounds(action, bounds);
        let approval;
        let denied = false;
        if (needsPerson) {
          report.approvals_requested += 1;
          await audit('approval.requested', { task, sequence_index: seq, action, external });
          const answer = await approve({
            task,
            sequence_index: seq,
            action,
            external,
            proposal,
            observation: current,
          });
          await audit(answer?.approved ? 'approval.granted' : 'approval.denied', {
            task,
            sequence_index: seq,
            action,
            external,
            by: answer?.by ?? null,
          });
          if (answer?.approved) {
            report.approvals_granted += 1;
            approval = answer.approval_id ?? `apr-${seq}`;
          } else {
            // Denied: the proposed action never runs. The person may act instead, or stop.
            denied = true;
            report.denials += 1;
            report.interventions += 1;
            failure('denied');
            taken = await personActs(current, proposal);
          }
        }
        if (!taken && !denied) {
          // Last look before acting: the kill switch, the permission for this mode, and the
          // screen, all read fresh. A switch thrown during an approval wait wins here.
          if (await gate.killSwitch()) {
            await stop('kill_switch');
            break;
          }
          if (!(await gate.permitted(mode))) {
            failure('permission_revoked');
            await stop('permission_revoked');
            break;
          }
          if (canonicalize(maskObservation(env.observe())) !== canonicalize(current)) {
            failure('stale_observation');
            await audit('proposal.rejected', {
              task,
              sequence_index: seq,
              reason: 'stale_observation',
            });
            report.interventions += 1;
            taken = await personActs(maskObservation(env.observe()), proposal);
          } else {
            const result = env.apply(action, { approval });
            report.executed.push({
              sequence_index: seq,
              action,
              result: result.status,
              approval: approval ?? null,
            });
            await audit('action.executed', {
              task,
              sequence_index: seq,
              action,
              result: result.status,
              approval: approval ?? null,
            });
            if (result.status !== 'ok') {
              // The site refused the action: that is the agent being wrong, a person would step in.
              failure(`env:${result.error ?? result.status}`);
              report.interventions += 1;
              errorsInRow += 1;
            } else errorsInRow = 0;
            taken = { action, result };
          }
        }
      }
      if (errorsInRow >= maxErrorsInRow) {
        await stop('repeated_errors');
        break;
      }
      if (taken === 'killed') {
        await stop('kill_switch');
        break;
      }
      if (!taken) {
        await stop('person_stopped');
        break;
      }
      report.trace.push({
        t_ms: tTotal,
        observation: current,
        action: taken.action,
        result: { status: taken.result.status === 'ok' ? 'ok' : 'error' },
      });
      prev = taken.action;
      seq += 1;
      dt = dtFor(taken.action);
      tTotal += dt;
      env.tick(dt);
    }
  } finally {
    await policy.close(session, scope).catch(() => {});
  }
  report.success = !!env.success;
  report.autonomous = report.success && report.interventions === 0;
  return report;
}

/** Policy adapter over a PolicyRuntime (infernet model serve --stdio) for one loaded model. */
export function runtimePolicy(runtime, modelHash, { prefix = 'w3bs', ttl_s = 600 } = {}) {
  let n = 0;
  return {
    async open(scope) {
      const session_id = `${prefix}-${Date.now().toString(36)}-${++n}`;
      await runtime.openSession(modelHash, session_id, { ttl_s, account_scope: scope });
      return { session_id, model_hash: modelHash, scope };
    },
    predict: (session, request) => runtime.predict(request, session.scope),
    close: (session, scope) => runtime.closeSession(session.session_id, scope),
  };
}

/**
 * The Agentic Desktop Layer seam. A future change implements observe/apply over the ADL
 * (accessibility tree in, leased actions out). Until then it refuses, so the Liquid Agent can
 * only act on the mock sites.
 */
export function desktopLayerEnvironment() {
  throw Object.assign(
    new Error(
      'Acting on the real desktop is not available yet: the Liquid Agent only runs against the mock sites.',
    ),
    {
      code: 'EXECUTOR_UNAVAILABLE',
      status: 501,
    },
  );
}
