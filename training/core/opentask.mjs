// A reviewed episode as an OpenTask 0.1 (logicsrc.com/docs/opentask): the goal, the apps,
// the parameters by name only, and the steps as meaning, never coordinates or typed text.
// The moshscript export and w3bs.org/tasks both start from this. Window titles are what a
// person saw and they leak (an inbox title carries an address), so a published task drops
// them unless the person chose to keep them.
import { collapseSteps } from './moshscript.mjs';

const line = (value, max) =>
  String(value ?? '')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, max);
// Free text never carries a {{placeholder}} the task did not declare.
const plain = (value, max) => line(value, max).replace(/\{\{|\}\}/g, '');

export function taskSlug(goal) {
  return (
    line(goal, 60)
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-|-$/g, '') || 'task'
  );
}

const ACTION = {
  click: 'click',
  double_click: 'click',
  right_click: 'click',
  drag: 'drag',
  hover: 'observe',
  scroll: 'scroll',
  key_shortcut: 'key',
  text_edit: 'type',
  focus: 'focus',
  navigate: 'open',
  wait: 'wait',
  other: 'other',
};
const SENSITIVITY = {
  number: 'public',
  date: 'public',
  url: 'public',
  string: 'personal',
  email: 'personal',
  file: 'personal',
  secret_ref: 'secret',
};
const SUCCESS = {
  ui_confirmation: 'screen',
  state_change: 'screen',
  external_check: 'artifact',
};

const appKind = (name) =>
  /chrom|firefox|browser|safari|edge|brave|vivaldi/i.test(name)
    ? 'browser'
    : /term|konsole|shell|alacritty|kitty|wezterm|tmux/i.test(name)
      ? 'terminal'
      : 'desktop';

export function toOpenTask(
  {
    episodeId,
    goal,
    explanation,
    parameters = [],
    steps,
    gaps = [],
    profile,
    recordedAt,
    outcome,
    stopConditions,
    humanApprovals,
    version,
  },
  { windowTitles = false } = {},
) {
  const params = parameters.map((p) => ({
    name: p.name,
    sensitivity: SENSITIVITY[p.type] ?? 'personal',
  }));
  const declared = new Set(params.map((p) => p.name));
  const collapsed = collapseSteps(steps);
  const out = collapsed.map((s) => {
    const step = { n: s.n, action: ACTION[s.type] ?? 'other' };
    if (s.app) step.app = plain(s.app, 200);
    if (windowTitles && s.window) step.window = plain(s.window, 400);
    if (s.role || s.name)
      step.target = Object.fromEntries(
        [
          ['role', s.role && plain(s.role, 100)],
          ['name', s.name && plain(s.name, 200)],
        ].filter(([, v]) => v),
      );
    if (step.target && !Object.keys(step.target).length) delete step.target;
    if (s.keys?.length) step.keys = s.keys.slice(0, 10).map((k) => line(k, 40));
    const name = /^\{\{([a-z][a-z0-9_]{0,63})\}\}$/.exec(s.text ?? '')?.[1];
    if (name) {
      step.text = `{{${name}}}`;
      if (!declared.has(name)) {
        declared.add(name);
        params.push({ name, sensitivity: 'personal' });
      }
    } else if (s.text)
      step.note = 'Text was typed here; the recording did not keep it. Ask for it.';
    step.evidence = s.evidence === 'observed' ? 'observed' : 'inferred';
    if (s.recorded !== String(s.n)) step.recorded = s.recorded;
    return step;
  });
  if (!out.length)
    out.push({
      n: 1,
      action: 'observe',
      note: 'The recording kept no steps; work from the goal.',
      evidence: 'inferred',
    });
  const apps = [...new Set(out.map((s) => s.app).filter(Boolean))].map((name) => ({
    name,
    kind: appKind(name),
  }));
  return {
    type: 'logicsrc.opentask',
    version: '0.1',
    id: taskSlug(goal),
    title: plain(goal, 200) || 'Task',
    goal: plain(goal, 500) || 'Task',
    ...(explanation ? { explanation: plain(explanation, 4000) } : {}),
    apps: apps.length ? apps : [{ name: 'Desktop', kind: 'desktop' }],
    ...(params.length ? { parameters: params } : {}),
    steps: out,
    capture: {
      profile: profile === 'visual-review' ? 'visual-review' : 'semantic',
      window_titles: windowTitles ? 'kept' : 'removed',
      gaps: [...new Set(gaps.map((g) => g.kind))],
    },
    stop_conditions: stopConditions,
    approvals: humanApprovals.map((a) => (/^before /i.test(a) ? a : `before ${a}`)),
    success: {
      check: `The goal is met: ${plain(goal, 450)}`,
      evidence: SUCCESS[outcome?.evidence_type] ?? 'human_assertion',
    },
    source: {
      kind: 'demonstration',
      recorder: `w3bs-training ${version}`,
      episode: episodeId,
      ...(recordedAt ? { recorded: recordedAt } : {}),
      reviewed: true,
      outcome: 'success',
    },
    status: 'untested',
  };
}
