// A reviewed episode as a moshscript (.mosh) for `moshcode run`. A recording holds
// semantic steps only (window focus and titles, no coordinates, keystrokes or
// typed text), so the script replays nothing: it hands the steps to an AI engine
// that drives the desktop through the Agentic Desktop Layer and re-observes the
// screen before every action. Every value from the store goes in as a JSON
// literal, never as code.

const line = (value, max = 160) =>
  String(value ?? '')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, max);

// Runs of the same step (the recorder emits one focus step per refocus) become one.
export function collapseSteps(steps) {
  const out = [];
  for (const s of steps) {
    const key = JSON.stringify([s.type, s.target ?? null, s.keys ?? null, s.text ?? null]);
    const last = out.at(-1);
    if (last?.key === key) {
      last.to = s.n;
      continue;
    }
    out.push({ key, from: s.n, to: s.n, step: s });
  }
  return out.map(({ from, to, step }, i) => ({
    n: i + 1,
    recorded: from === to ? `${from}` : `${from}-${to}`,
    type: step.type,
    ...(step.target?.app ? { app: line(step.target.app, 200) } : {}),
    ...(step.target?.window ? { window: line(step.target.window, 400) } : {}),
    ...(step.target?.role ? { role: line(step.target.role, 100) } : {}),
    ...(step.target?.name ? { name: line(step.target.name, 200) } : {}),
    ...(step.keys ? { keys: step.keys } : {}),
    ...(step.text ? { text: line(step.text, 100) } : {}),
    evidence: step.evidence,
  }));
}

export function moshscriptFilename(goal, episodeId) {
  const slug = line(goal, 60)
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-|-$/g, '');
  return `${slug || 'task'}-${episodeId.replace(/^ep_/, '').slice(0, 8)}.mosh`;
}

// The script carries the OpenTask itself (training/core/opentask.mjs) as its data.
export function toMoshscript(task, { exportedAt, version, filename }) {
  const focusOnly = task.steps.every((s) => s.action === 'focus');
  const captured =
    task.capture?.profile === 'visual-review' && focusOnly
      ? 'window focus changes: no clicks, keystrokes, typed text or coordinates'
      : 'semantic steps (app, control, shortcut): no coordinates, and typed text only as placeholders';
  const json = JSON.stringify(task, null, 2)
    .replace(/\u2028/g, '\\u2028')
    .replace(/\u2029/g, '\\u2029');
  const file = filename ?? `${task.id}.mosh`;
  const from = task.source?.episode
    ? `W3BS Training episode ${line(task.source.episode, 100)}`
    : `OpenTask ${task.id}`;
  return `#!/usr/bin/env moshscript
// ${line(task.title)}
// Exported from ${from} on ${exportedAt.slice(0, 10)} (w3bs-training ${line(version, 40)}).
// The task below is an OpenTask 0.1 document: https://logicsrc.com/docs/opentask
//
//   moshcode run ${file}             opens claude with this task
//   moshcode run ${file} codex       or another engine
//   moshcode run ${file} --dry-run   shows what it would do
//
// The recording holds ${captured}.
// Nothing here is replayed. The engine does the task through
// the W3BS Agentic Desktop Layer (MCP server "w3bs-desktop"), looking at the screen
// before every action and asking you before anything it cannot undo. One-time setup:
//   w3bs desktop agents add claude-code --preset broad
//   w3bs desktop setup claude-code --agent claude-code
// Enable the layer from the tray before running.

const task = ${json};

const engine = (argv[0] || "claude").toLowerCase();
const apps = task.apps.map((a) => a.name);
const gaps = (task.capture && task.capture.gaps) || [];

say(\`🎸 \${task.goal}\`);
say(\`   \${task.steps.length} step(s) across \${apps.join(", ") || "the desktop"}\`);
const describe = (s) =>
  [
    s.action,
    s.app,
    s.window && \`(window: \${s.window})\`,
    s.target && \`[\${[s.target.role, s.target.name].filter(Boolean).join(" ")}]\`,
    s.keys && \`keys \${s.keys.join("+")}\`,
    s.text && \`text \${s.text}\`,
    s.note && \`(\${s.note})\`,
  ]
    .filter(Boolean)
    .join(" ");
for (const s of task.steps) say(\`   \${s.n}. \${describe(s)}\`);

const prompt = [
  \`Do this task on my desktop: \${task.goal}\`,
  task.explanation ? \`In my words: \${task.explanation}\` : "",
  "",
  \`I recorded it once with W3BS Training. The recording holds ${captured},\`,
  "so work out each action from what is on screen. The steps I took:",
  ...task.steps.map((s) => \`\${s.n}. \${describe(s)}\`),
  (task.parameters || []).length
    ? \`Text in {{braces}} is a value you must ask me for: \${task.parameters.map((p) => p.name).join(", ")}.\`
    : "",
  gaps.length
    ? \`Parts were not captured (\${gaps.join(", ")}); do not guess what happened there, ask me.\`
    : "",
  "",
  "Use the w3bs-desktop MCP tools. Observe the screen before every action and never reuse old",
  "coordinates. In a web page use its browser_* tools (browser_snapshot, then click and fill by",
  "uid), not screenshots; if they fail with browser_unavailable, stop and tell me.",
  task.source && task.source.episode
    ? \`If the w3bs-training MCP is connected, training_get_frames with episode_id \${task.source.episode} shows the screenshots I recorded.\`
    : "",
  \`Stop and tell me if: \${task.stop_conditions.join("; ")}.\`,
  task.approvals.length ? \`Ask me \${task.approvals.join("; ")}.\` : "",
  \`It worked when: \${task.success.check}\`,
  "When you finish or stop, say what you did and what is left, and report it with",
  "training_report_run if that tool is available.",
]
  .filter((l, i, all) => l !== "" || all[i - 1] !== "")
  .join("\\n");

const quote = (s) => \`'\${String(s).replace(/'/g, \`'\\\\''\`)}'\`;

if (!shell("command -v w3bs >/dev/null 2>&1").ok) {
  say("w3bs is not installed: curl -fsSL https://w3bs.org/install.sh | sh, then the setup lines at the top of this file.");
} else if (engine === "claude" || engine === "codex") {
  // Interactive, so you see and approve each desktop action as it happens.
  shell(\`\${engine} \${quote(prompt)}\`);
} else {
  say(await ai(prompt, { engine }));
}
`;
}
