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

export function toMoshscript({
  episodeId,
  goal,
  family,
  explanation,
  steps,
  gaps = [],
  stopConditions,
  humanApprovals,
  exportedAt,
  version,
}) {
  const collapsed = collapseSteps(steps);
  const focusOnly = collapsed.every((s) => s.type === 'focus');
  const captured = focusOnly
    ? 'window focus changes and titles: no clicks, keystrokes, typed text or coordinates'
    : 'semantic steps (app, control, shortcut): no coordinates, and typed text only as placeholders';
  const task = {
    episode_id: episodeId,
    goal: line(goal, 500),
    ...(family ? { workflow_family: line(family, 200) } : {}),
    ...(explanation ? { explanation: String(explanation).slice(0, 4000) } : {}),
    apps: [...new Set(collapsed.map((s) => s.app).filter(Boolean))],
    steps: collapsed,
    captured,
    capture_gaps: [...new Set(gaps.map((g) => g.kind))],
    stop_conditions: stopConditions,
    human_approvals: humanApprovals,
  };
  const json = JSON.stringify(task, null, 2)
    .replace(/\u2028/g, '\\u2028')
    .replace(/\u2029/g, '\\u2029');
  return `#!/usr/bin/env moshscript
// ${line(goal)}
// Exported from W3BS Training episode ${episodeId} on ${exportedAt.slice(0, 10)} (w3bs-training ${version}).
//
//   moshcode run ${moshscriptFilename(goal, episodeId)}             opens claude with this task
//   moshcode run ${moshscriptFilename(goal, episodeId)} codex       or another engine
//   moshcode run ${moshscriptFilename(goal, episodeId)} --dry-run   shows what it would do
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

say(\`🎸 \${task.goal}\`);
say(\`   \${task.steps.length} step(s) across \${task.apps.join(", ") || "the desktop"}\`);
const describe = (s) =>
  [
    s.type,
    s.app,
    s.window && \`(window: \${s.window})\`,
    (s.role || s.name) && \`[\${[s.role, s.name].filter(Boolean).join(" ")}]\`,
    s.keys && \`keys \${s.keys.join("+")}\`,
    s.text && \`text <\${s.text}>\`,
  ]
    .filter(Boolean)
    .join(" ");
for (const s of task.steps) say(\`   \${s.n}. \${describe(s)}\`);

const prompt = [
  \`Do this task on my desktop: \${task.goal}\`,
  task.explanation ? \`In my words: \${task.explanation}\` : "",
  "",
  \`I recorded it once with W3BS Training. The recording holds \${task.captured},\`,
  "so work out each action from what is on screen. The steps I took:",
  ...task.steps.map((s) => \`\${s.n}. \${describe(s)}\`),
  task.steps.some((s) => s.text)
    ? "Text in <angle brackets> is a placeholder, not what I typed: ask me for the value."
    : "",
  task.capture_gaps.length
    ? \`Parts were not captured (\${task.capture_gaps.join(", ")}); do not guess what happened there, ask me.\`
    : "",
  "",
  "Use the w3bs-desktop MCP tools. Observe the screen before every action and never reuse old",
  "coordinates. If the w3bs-training MCP is connected, training_get_frames with episode_id",
  \`\${task.episode_id} shows the screenshots I recorded.\`,
  \`Stop and tell me if: \${task.stop_conditions.join("; ")}.\`,
  \`Ask me \${task.human_approvals.join("; ")}.\`,
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
