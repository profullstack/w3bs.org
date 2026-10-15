// W3BS Desktop Control. Everything comes from the local broker over same-origin requests; text
// from agents (task descriptions, approval summaries) is only ever set with textContent.

const $ = (sel, root = document) => root.querySelector(sel);
const STATES = {
  enabled: ['✓', 'Enabled', 'Agents you authorized may use the mouse, keyboard and screen of this desktop now.'],
  disabled: ['–', 'Disabled', 'No agent can use this desktop. Only you can turn access on.'],
  enabling: ['…', 'Enabling', 'Waiting for the desktop to grant remote control. Answer its permission dialog if one is open.'],
  stopping: ['■', 'Stopping', 'Stopping every agent action and releasing held keys.'],
  blocked: ['!', 'Blocked', ''],
};
const PRESETS = [
  ['observe', 'Observe', 'Windows, accessibility and screenshots. No input.'],
  ['restricted', 'Restricted', 'Pointer and keyboard only in the apps you name.'],
  ['broad', 'Broad desktop', 'Sees and operates every application. Per-app limits cannot be enforced.'],
  ['none', 'Nothing', 'No access until changed again.'],
];

let S = null; // last state
let csrf = '';
let editing = null; // agent id whose editor is open
let events = null;
let busy = false;

function h(tag, attrs = {}, ...kids) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === null || v === undefined || v === false) continue;
    if (k === 'class') el.className = v;
    else if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
    else if (k === 'text') el.textContent = v;
    else el.setAttribute(k, v === true ? '' : v);
  }
  for (const kid of kids.flat()) if (kid !== null && kid !== undefined && kid !== false) el.append(kid instanceof Node ? kid : document.createTextNode(String(kid)));
  return el;
}
const pill = (text, kind = '') => h('span', { class: `pill ${kind}`.trim(), text });

function time(ms) {
  if (!ms) return '';
  const d = new Date(ms);
  const today = new Date().toDateString() === d.toDateString();
  return today ? d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' }) : d.toLocaleString([], { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
}
function ago(ms) {
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  return time(ms);
}
const short = (id) => (id || '').replace(/^([a-z]+_)([0-9a-f]{6})[0-9a-f]+$/, '$1$2');
const agentName = (id) => (S?.agents || []).find((a) => a.id === id)?.name || short(id) || '';

function presetOf(p) {
  const caps = new Set(p.capabilities || []);
  if (!caps.size) return 'none';
  if (p.mode === 'restricted') return 'restricted';
  if (caps.has('pointer') || caps.has('keyboard')) return 'broad';
  if (caps.has('screen_capture')) return 'observe';
  return 'custom';
}
const presetLabel = (k) => (PRESETS.find(([key]) => key === k) || [k, 'Custom'])[1];

function notice(text, kind = '') {
  const n = $('[data-notice]');
  n.textContent = text || '';
  n.dataset.kind = kind;
}

// ---------------------------------------------------------------- network

async function load() {
  let r;
  try {
    r = await fetch('/control/api/state', { cache: 'no-store', credentials: 'same-origin' });
  } catch {
    return gate('BROKER NOT RUNNING', 'The broker is <em>not</em> running.', 'Start it with w3bs desktop daemon (installers start it at login), then open this app again from the tray.');
  }
  if (r.status === 401) return gate();
  if (!r.ok) return notice(`The broker answered ${r.status}.`, 'error');
  S = await r.json();
  csrf = S.csrf;
  render();
}

async function act(action, extra = {}) {
  if (busy) return null;
  busy = true;
  document.body.setAttribute('aria-busy', 'true');
  try {
    const r = await fetch('/control/api/action', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json', 'X-W3BS-CSRF': csrf },
      body: JSON.stringify({ action, ...extra }),
    });
    if (r.status === 401) {
      gate();
      return null;
    }
    const v = await r.json().catch(() => ({}));
    if (!r.ok || v.error) {
      notice(v.error?.message || `Failed (${r.status})`, 'error');
      return null;
    }
    if (v.result?.signed_out || action === 'quit') {
      gate(action === 'quit' ? 'DISABLED AND SIGNED OUT' : 'SIGNED OUT', action === 'quit' ? 'Access is <em>off.</em>' : 'Signed <em>out.</em>', action === 'quit' ? 'Agent access is disabled and this app is signed out. You can close this window.' : 'You can close this window.');
      if (action === 'quit') window.close();
      return v;
    }
    notice(done(action, v.result), '');
    await load();
    return v;
  } catch (e) {
    notice(`Could not reach the broker: ${e.message}`, 'error');
    return null;
  } finally {
    busy = false;
    document.body.removeAttribute('aria-busy');
  }
}

function done(action, r) {
  switch (action) {
    case 'enable': case 'confirm_enable': return 'Agent access enabled.';
    case 'disable': return `Agent access disabled${r?.tasks_cancelled ? `, ${r.tasks_cancelled} task(s) cancelled` : ''}. ${r?.note || ''}`;
    case 'stop': return `Stopped all agent activity (${r?.tasks_cancelled ?? 0} task(s) cancelled). Access stays enabled.`;
    case 'approve': return 'Approved. The agent may retry that exact request.';
    case 'deny': return 'Denied.';
    case 'agent_update': return `Access for ${r?.name || 'the agent'} changed.`;
    case 'agent_revoke': return 'Agent revoked.';
    case 'task_cancel': return 'Task cancelled.';
    case 'audit_delete': return `Audit log deleted (${r?.files_deleted ?? 0} file(s)).`;
    default: return '';
  }
}

function connect() {
  if (events || !('EventSource' in window)) return;
  events = new EventSource('/control/api/events');
  events.addEventListener('open', () => live(true));
  events.addEventListener('change', () => load());
  events.addEventListener('signed_out', () => { events.close(); events = null; gate(); });
  events.addEventListener('error', () => live(false));
}

function live(on) {
  const el = $('#link-state');
  el.dataset.live = on ? 'yes' : 'no';
  $('[data-link-text]').textContent = on ? 'Live' : 'Reconnecting';
}

// ---------------------------------------------------------------- views

function gate(eyebrow = 'SIGNED OUT', title = null, text = null) {
  if (events) { events.close(); events = null; }
  S = null;
  const expired = new URLSearchParams(location.search).get('login') === 'expired';
  $('#app').hidden = true;
  $('#gate').hidden = false;
  $('[data-gate-eyebrow]').textContent = expired && eyebrow === 'SIGNED OUT' ? 'LINK EXPIRED' : eyebrow;
  // Titles are fixed strings from this file (they carry <em> for the accent word).
  $('[data-gate-title]').innerHTML = title || (expired ? 'That link has <em>expired.</em>' : 'Open it from <em>your</em> desktop.');
  $('[data-gate-text]').textContent = text || (expired ? 'Sign-in links work once, for 60 seconds. Get a new one from the tray or the terminal.' : 'This control app signs in with a one-time link from the tray, so a web page or an agent cannot open it for you.');
  live(false);
  $('[data-link-text]').textContent = 'Signed out';
}

function render() {
  $('#gate').hidden = true;
  $('#app').hidden = false;
  connect();
  renderStatus();
  renderTraining();
  renderApprovals();
  renderAgents();
  renderTasks();
  renderActivity();
  renderSettings();
}

function renderStatus() {
  const st = S.status;
  const [icon, word, detail] = STATES[st.state] || ['?', st.state, ''];
  const panel = $('#status');
  panel.dataset.state = st.state;
  $('[data-state-icon]').textContent = icon;
  $('[data-state-text]').textContent = word;
  $('[data-state-detail]').textContent = st.state === 'blocked' ? `Blocked: ${st.blocked_reason || 'see w3bs desktop doctor'}` : detail;
  const lease = st.lease;
  $('[data-active]').textContent = lease ? agentName(lease.agent) : (st.running_tasks.length ? st.running_tasks.map((t) => agentName(t.agent)).join(', ') : 'None');
  $('[data-lease]').textContent = lease ? `${agentName(lease.agent)} · task ${short(lease.task)}${(st.running_tasks.find((t) => t.id === lease.task) || {}).description ? ` · ${st.running_tasks.find((t) => t.id === lease.task).description}` : ''}` : 'Free';
  document.title = `${word} · Desktop Control`;

  const toggle = $('[data-toggle]');
  const on = st.state === 'enabled' || st.state === 'enabling';
  toggle.textContent = on ? 'Disable agent access' : 'Enable agent access';
  toggle.className = `button big ${on ? 'dark' : 'primary'}`;
  toggle.dataset.action = on ? 'disable' : 'enable';
  toggle.disabled = st.state === 'stopping';

  const p = S.pending_enable;
  $('[data-pending]').hidden = !p;
  if (p) $('[data-pending-text]').textContent = `A terminal asked to turn agent access on (code ${p.code}). Confirm only if you started it.`;

  const sc = S.settings.shortcut;
  const scEl = $('[data-shortcut]');
  scEl.replaceChildren(...(sc ? ['Emergency stop: ', h('kbd', { text: sc }), ' disables access at once, even with this window closed.'] : ['Emergency stop: the shortcut is shown here once the tray or menu bar has bound it. ', h('code', { text: 'w3bs desktop disable' }), ' always works.']));

  const hint = $('[data-hint]');
  const needsControl = S.settings.requires_native_control && S.settings.native_controls === 0;
  hint.hidden = !(needsControl && !on);
  hint.replaceChildren('Enabling needs the tray, the GNOME panel or ', h('code', { text: 'w3bs desktop control' }), ' running: that switch turns access off if it goes away. This app is not that switch, so closing it never turns access off.');
}

function renderApprovals() {
  const pending = (S.approvals || []).filter((a) => a.status === 'pending');
  $('[data-approvals-count]').textContent = `${pending.length} waiting`;
  $('[data-approvals-count]').className = `pill ${pending.length ? 'orange' : ''}`.trim();
  const list = $('[data-approvals]');
  if (!pending.length) return list.replaceChildren(h('li', { class: 'empty', text: 'Nothing is waiting for you.' }));
  list.replaceChildren(...pending.map((a) => h('li', {},
    h('div', { class: 'row-top' }, h('span', { class: 'row-title', text: `${agentName(a.agent)} wants: ${a.summary}` })),
    h('div', { class: 'row-sub' }, `${a.operation} on `, h('code', { text: a.target || 'the desktop' }), ` · task ${short(a.task)} · expires ${time(a.expires_at_ms)}`),
    h('div', { class: 'row-actions' },
      h('button', { type: 'button', class: 'button primary small', onclick: () => act('approve', { approval: a.id }) }, 'Approve'),
      h('button', { type: 'button', class: 'button outline small', onclick: () => act('deny', { approval: a.id }) }, 'Deny')),
  )));
}

const CAP_LABEL = { pointer: 'pointer', keyboard: 'keyboard', screen_capture: 'screenshots', windows: 'windows', accessibility: 'accessibility', clipboard_read: 'clipboard read', clipboard_write: 'clipboard write', browser: 'browser', files: 'files', email: 'email', calendar: 'calendar' };

function renderAgents() {
  const agents = [...(S.agents || [])].sort((a, b) => Number(a.revoked) - Number(b.revoked) || a.name.localeCompare(b.name));
  const active = agents.filter((a) => !a.revoked).length;
  $('[data-agents-count]').textContent = `${active} active`;
  const list = $('[data-agents]');
  if (!agents.length) return list.replaceChildren(h('li', { class: 'empty' }, 'No agents yet. Register one with ', h('code', { text: 'w3bs desktop agents add claude-code --preset observe' }), '.'));
  list.replaceChildren(...agents.map((a) => {
    const preset = presetOf(a.policy);
    const caps = [...(a.policy.capabilities || [])];
    const li = h('li', {},
      h('div', { class: 'row-top' }, h('span', { class: 'row-title', text: a.name }), a.revoked ? pill('Revoked', 'red') : pill(presetLabel(preset), preset === 'broad' ? 'orange' : preset === 'none' ? '' : 'green'),
        null),
      h('div', { class: 'row-sub' },
        a.revoked ? 'Its token no longer works. ' : (caps.length ? null : 'No capabilities.'),
        a.policy.apps?.length ? ['Apps: ', h('code', { text: a.policy.apps.join(', ') }), '. '] : null,
        a.policy.approval_required_for?.length ? ['You approve: ', h('code', { text: a.policy.approval_required_for.join(', ') }), '. '] : null,
        a.policy.expires_at_ms ? `Grant expires ${time(a.policy.expires_at_ms)}. ` : null,
        `Added ${time(a.created_at_ms)}.`),
      caps.length && !a.revoked ? h('div', { class: 'caps' }, caps.map((c) => pill(CAP_LABEL[c] || c))) : null,
      !a.revoked ? h('div', { class: 'row-actions' },
        h('button', { type: 'button', class: 'button outline small', 'aria-expanded': String(editing === a.id), onclick: () => { editing = editing === a.id ? null : a.id; renderAgents(); } }, editing === a.id ? 'Close' : 'Change access'),
        h('button', { type: 'button', class: 'button outline danger small', onclick: () => { if (confirm(`Revoke ${a.name}? Its token stops working and its tasks are cancelled.`)) act('agent_revoke', { agent: a.id }); } }, 'Revoke')) : null,
      editing === a.id && !a.revoked ? editor(a, preset) : null);
    return li;
  }));
}

function editor(a, current) {
  const name = `preset-${a.id}`;
  const apps = h('input', { type: 'text', id: `apps-${a.id}`, value: (a.policy.apps || []).join(', '), placeholder: 'org.kde.kate, firefox, code*', autocomplete: 'off', spellcheck: 'false' });
  const warn = h('p', { class: 'warn', hidden: current !== 'broad', text: 'Broad desktop lets this agent see and operate every application. Per-app or draft-only limits cannot be enforced in this mode.' });
  const form = h('form', { class: 'editor', onsubmit: (e) => {
    e.preventDefault();
    const preset = form.querySelector(`input[name="${name}"]:checked`)?.value;
    const list = apps.value.split(',').map((s) => s.trim()).filter(Boolean);
    if (preset === 'restricted' && !list.length) return notice('Restricted needs at least one app id.', 'error');
    act('agent_update', { agent: a.id, preset, apps: list }).then((r) => { if (r) { editing = null; renderAgents(); } });
  } },
  h('fieldset', {}, h('legend', { text: `ACCESS FOR ${a.name.toUpperCase()}` }),
    PRESETS.map(([key, label, help]) => h('label', { class: 'choice' },
      h('input', { type: 'radio', name, value: key, checked: key === current, onchange: () => { warn.hidden = key !== 'broad'; } }),
      h('span', {}, label, h('small', { text: help }))))),
  h('label', { class: 'field', for: `apps-${a.id}` }, 'Apps (restricted mode; comma-separated app ids, prefix* allowed)', apps),
  warn,
  a.policy.approval_required_for?.length ? h('p', { class: 'row-sub', text: `Kept: you still approve ${a.policy.approval_required_for.join(', ')} requests.` }) : null,
  h('div', { class: 'row-actions' }, h('button', { type: 'submit', class: 'button dark small' }, 'Save access'), h('button', { type: 'button', class: 'button outline small', onclick: () => { editing = null; renderAgents(); } }, 'Cancel')));
  return form;
}

function renderTasks() {
  const tasks = S.tasks || [];
  const running = tasks.filter((t) => t.state === 'running');
  $('[data-tasks-count]').textContent = `${running.length} running`;
  const list = $('[data-tasks]');
  if (!tasks.length) return list.replaceChildren(h('li', { class: 'empty', text: 'No tasks since the broker started.' }));
  list.replaceChildren(...tasks.slice(0, 15).map((t) => h('li', {},
    h('div', { class: 'row-top' }, h('span', { class: 'row-title', text: t.description || short(t.id) }), pill(t.state[0].toUpperCase() + t.state.slice(1), t.state === 'running' ? 'orange' : t.state === 'completed' ? 'green' : '')),
    h('div', { class: 'row-sub', text: `${agentName(t.agent)} · ${short(t.id)} · started ${ago(t.created_at_ms)}${t.ended_at_ms ? ` · ended ${ago(t.ended_at_ms)}` : ''} · ${t.batches_completed} batch(es)` }),
    t.state === 'running' ? h('div', { class: 'row-actions' }, h('button', { type: 'button', class: 'button outline small', onclick: () => act('task_cancel', { task: t.id }) }, 'Cancel task')) : null)));
}

function detail(r) {
  const m = r.meta || {};
  const bits = [];
  if (m.reason) bits.push(m.reason);
  if (m.via) bits.push(`via ${m.via}`);
  if (m.why) bits.push(m.why);
  if (m.operation) bits.push(m.operation);
  if (m.display) bits.push(m.display);
  if (m.count !== undefined) bits.push(`${m.count} windows`);
  if (r.outcome) bits.push(r.outcome);
  return bits.join(' · ');
}

function renderActivity() {
  $('[data-retention]').textContent = `kept ${S.settings.retention_days} days`;
  const rows = (S.activity || []).slice(0, 25);
  const body = $('[data-activity]');
  if (!rows.length) return body.replaceChildren(h('tr', {}, h('td', { colspan: '5', class: 'muted', text: 'No activity recorded.' })));
  body.replaceChildren(...rows.map((r) => h('tr', {},
    h('td', { text: time(r.ts_ms) }),
    h('td', {}, h('code', { text: r.op })),
    h('td', { text: r.agent ? agentName(r.agent) : '' }),
    h('td', {}, pill(r.decision, r.decision === 'denied' || r.decision === 'blocked' ? 'red' : r.decision === 'approval_required' ? 'orange' : '')),
    h('td', { text: detail(r) }))));
}

function renderSettings() {
  const s = S.settings;
  const st = S.status;
  const row = (k, v, small) => h('div', {}, h('dt', { text: k }), h('dd', {}, v, small ? h('small', {}, small) : null));
  $('[data-settings]').replaceChildren(
    row('Emergency stop shortcut', s.shortcut || 'Not bound yet', s.shortcut ? 'Set by your desktop. Change it in the desktop\'s shortcut settings.' : 'The tray (Linux), the menu-bar item (macOS) or the broker (Windows) binds it.'),
    row('Audit retention', `${s.retention_days} days`, ['Set with ', h('code', { text: 'w3bs desktop daemon --retention-days N' }), '.']),
    row('Native controls running', String(s.native_controls), s.requires_native_control ? 'Enabling needs at least one; access turns off when the last one goes away.' : (s.enabled_by_default ? 'Not required: this broker runs with --enabled-by-default.' : 'Not required by this broker.')),
    row('MCP over HTTP', s.mcp_http || 'Off', s.mcp_http ? 'Agents connect here with their own token. This app never accepts agent tokens.' : ['Start the daemon with ', h('code', { text: '--mcp-http 127.0.0.1:7801' }), ' to serve it.']),
    row('Control app', s.app_url, 'Install it from your browser\'s menu to keep it in your app launcher. It signs in with a one-time link each time.'),
    row('Backend', `${st.adapter} · broker ${st.version}`, null),
    row('Start at login', 'Managed by the installer', 'The installer adds the broker and the tray to your session; its uninstall option removes them.'),
  );
}

// ---------------------------------------------------------------- training

const TRAINING_WORDS = { idle: 'Not recording', arming: 'Starting', recording: 'Recording', paused: 'Paused', blocked: 'Stopped at size limit', finishing: 'Saving', error: 'Error' };
const draft = { mode: 'teach', goal: '', apps: new Set(), allow: false };
let trainingApps = null;
let trainingView = '';
let trainingFocused = false;

// An action whose result this section shows itself, without the global reload.
async function post(action, extra = {}) {
  const r = await fetch('/control/api/action', {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'Content-Type': 'application/json', 'X-W3BS-CSRF': csrf },
    body: JSON.stringify({ action, ...extra }),
  });
  const v = await r.json().catch(() => ({}));
  if (!r.ok || v.error) throw new Error(v.error?.message || `Failed (${r.status})`);
  return v.result;
}

async function loadApps() {
  try {
    trainingApps = await post('training_apps');
  } catch (e) {
    trainingApps = { error: e.message };
  }
  renderAppPicker();
}

function renderAppPicker() {
  const box = $('[data-training-apps]');
  if (!box) return;
  if (!trainingApps) return box.replaceChildren(h('p', { class: 'muted', text: 'Looking for open applications…' }));
  if (trainingApps.error) return box.replaceChildren(h('p', { class: 'warn', text: trainingApps.error }));
  if (!trainingApps.length) return box.replaceChildren(h('p', { class: 'muted', text: 'No application windows are open.' }));
  box.replaceChildren(...trainingApps.map((a) => h('label', { class: 'choice' },
    h('input', { type: 'checkbox', value: a.app_id, checked: draft.apps.has(a.app_id), onchange: (e) => { if (e.target.checked) draft.apps.add(a.app_id); else draft.apps.delete(a.app_id); } }),
    h('span', {}, h('code', { text: a.app_id }), h('small', { text: `${a.windows} window${a.windows === 1 ? '' : 's'}${a.focused ? ' · focused now' : ''}` })))));
}

function startForm() {
  const enabled = S.status.state === 'enabled';
  const goal = h('textarea', { id: 'training-goal', rows: '2', maxlength: '500', placeholder: 'Prepare the release announcement draft', oninput: (e) => { draft.goal = e.target.value; } });
  goal.value = draft.goal;
  const goalField = h('label', { class: 'field', for: 'training-goal', hidden: draft.mode !== 'teach' }, 'What are you about to do?', goal);
  const mode = (key, label, help) => h('label', { class: 'choice' },
    h('input', { type: 'radio', name: 'training-mode', value: key, checked: draft.mode === key, onchange: () => { draft.mode = key; goalField.hidden = key !== 'teach'; } }),
    h('span', {}, label, h('small', { text: help })));
  const form = h('form', { class: 'editor', onsubmit: async (e) => {
    e.preventDefault();
    if (!draft.apps.size) return notice('Choose at least one application to record.', 'error');
    if (draft.mode === 'teach' && !draft.goal.trim()) return notice('Say what you are about to do.', 'error');
    try {
      await post('training_start', { mode: draft.mode, goal: draft.goal, applications: [...draft.apps], allow_agents: draft.allow });
      draft.goal = '';
      notice(draft.mode === 'teach' ? 'Recording. Do the task, then choose how it went.' : 'Recording your work in the chosen applications.');
      await load();
    } catch (err) {
      notice(err.message, 'error');
    }
  } },
  h('fieldset', {}, h('legend', { text: 'WHAT TO RECORD' }),
    mode('teach', 'Teach this task', 'One deliberate demonstration with a goal. Best for examples an agent will reuse.'),
    mode('observe', 'Observe my work', 'Keeps recording the chosen applications until you finish. Mark boundaries between tasks.')),
  goalField,
  h('fieldset', {}, h('legend', { text: 'APPLICATIONS IT MAY SEE' }), h('div', { 'data-training-apps': '' }),
    h('button', { type: 'button', class: 'button outline small', onclick: () => loadApps() }, 'Refresh list')),
  h('label', { class: 'choice' },
    h('input', { type: 'checkbox', checked: draft.allow, onchange: (e) => { draft.allow = e.target.checked; } }),
    h('span', {}, 'Let agents keep working while I record', h('small', { text: 'Off: agents cannot use the mouse or keyboard until you finish. On: steps are marked as possibly mixed with agent activity.' }))),
  enabled ? null : h('p', { class: 'warn' }, 'Turn agent access on first (above): recording uses the same screen permission. Agents still cannot act while you teach unless you allow it here.'),
  h('div', { class: 'row-actions' }, h('button', { type: 'submit', class: 'button primary', disabled: !enabled }, 'Start recording')));
  return form;
}

function recordingControls(t) {
  const explain = h('textarea', { id: 'training-explain', rows: '2', maxlength: '8000', placeholder: 'Why you did it this way (optional, your own words)' });
  const doing = (action, extra, msg) => async () => {
    try {
      await post(action, extra);
      if (msg) notice(msg);
      await load();
    } catch (e) {
      notice(e.message, 'error');
    }
  };
  const finish = (outcome, label, cls) => h('button', { type: 'button', class: `button small ${cls}`, onclick: doing('training_finish', { outcome }, 'Finished. The recording is in W3BS Training for your review.') }, label);
  return h('div', {},
    h('div', { class: 'row-actions' },
      t.state === 'recording'
        ? h('button', { type: 'button', class: 'button outline small', onclick: doing('training_pause', {}, 'Paused.') }, 'Pause')
        : h('button', { type: 'button', class: 'button primary small', onclick: doing('training_resume', {}, 'Recording again.') }, 'Resume'),
      t.recording?.mode === 'observe' ? h('button', { type: 'button', class: 'button outline small', onclick: doing('training_mark', {}, 'Task boundary marked.') }, 'Mark task boundary') : null),
    h('p', { class: 'row-sub', text: 'How did it go? Finishing hands the recording to W3BS Training.' }),
    h('div', { class: 'row-actions' }, finish('success', 'Complete', 'dark'), finish('partial', 'Partial', 'outline'), finish('failure', 'Failed', 'outline'), finish('unknown', 'Not sure', 'outline'),
      h('button', { type: 'button', class: 'button outline danger small', onclick: () => { if (confirm('Throw this recording away? Nothing is kept.')) doing('training_discard', {}, 'Recording discarded.')(); } }, 'Discard')),
    h('label', { class: 'field', for: 'training-explain' }, 'Add an explanation', explain),
    h('div', { class: 'row-actions' }, h('button', { type: 'button', class: 'button outline small', onclick: async () => {
      if (!explain.value.trim()) return;
      await doing('training_explain', { text: explain.value }, 'Explanation added.')();
      explain.value = '';
    } }, 'Add explanation')));
}

function renderTraining() {
  const t = S.training;
  const pill = $('[data-training-pill]');
  pill.textContent = TRAINING_WORDS[t.state] || t.state;
  pill.className = `pill ${t.state === 'recording' ? 'red' : t.state === 'paused' || t.state === 'blocked' ? 'orange' : ''}`.trim();
  const box = $('[data-training]');
  const active = ['recording', 'paused', 'blocked', 'arming', 'finishing'].includes(t.state);
  const view = active ? `rec:${t.state}` : `idle:${S.status.state}`;
  if (view !== trainingView) {
    trainingView = view;
    if (active) {
      box.replaceChildren(h('dl', { class: 'facts', 'data-training-facts': '' }), recordingControls(t));
    } else {
      box.replaceChildren(startForm());
      renderAppPicker();
      if (S.status.state === 'enabled') loadApps();
    }
  }
  if (active && t.recording) {
    const r = t.recording;
    const fact = (k, v) => h('div', {}, h('dt', { text: k }), h('dd', { text: v }));
    $('[data-training-facts]').replaceChildren(
      fact(r.mode === 'teach' ? 'Teaching' : 'Observing', r.mode === 'teach' ? r.goal : 'my work'),
      fact('Applications', r.applications.join(', ')),
      fact('Captured', `${r.frames} frame(s), ${r.steps} step(s) in ${Math.round(r.elapsed_ms / 60000)} min`),
      fact('Right now', t.state === 'paused' ? 'Paused' : r.excluded_now ? 'Not capturing: the focused window is not one you approved' : 'Capturing the focused window'),
      fact('Agents', t.agent_leases));
    if (t.reason) $('[data-training-facts]').append(fact('Note', t.reason));
  }
  const after = $('[data-training-after]');
  const pending = t.pending_handoffs || [];
  const last = t.last_finished;
  after.replaceChildren(...[
    pending.length ? h('div', { class: 'banner' }, h('p', {}, h('strong', { text: `${pending.length} recording(s) waiting for W3BS Training.` }), ` ${pending[0].error}`),
      h('div', { class: 'banner-actions' }, h('button', { type: 'button', class: 'button primary small', onclick: async () => { try { const r = await post('training_handoff'); notice(`${r.imported} handed over, ${r.still_pending} still waiting.`); await load(); } catch (e) { notice(e.message, 'error'); } } }, 'Try again'),
        h('span', { class: 'muted', text: 'Held in memory only; lost if the broker stops.' }))) : null,
    last ? h('p', { class: 'row-sub', text: `Last recording: ${last.summary.frames} frame(s), ${last.summary.episodes} task(s)${last.summary.interrupted ? ', stopped when the desktop layer closed' : ''}. ${last.handoff === 'imported' ? 'In W3BS Training for review.' : 'Waiting to be handed over.'}` }) : null,
    h('div', { class: 'row-actions' },
      h('button', { type: 'button', class: 'button outline small', onclick: async () => { try { await post('training_open_app'); notice('W3BS Training is opening in your browser.'); } catch (e) { notice(e.message, 'error'); } } }, 'Open W3BS Training'),
      h('button', { type: 'button', class: 'button outline danger small', onclick: async () => {
        if (!confirm('Delete everything recorded in the last 10 minutes that is still here? Recordings already in W3BS Training are deleted there (w3bs training forget).')) return;
        try { const r = await post('training_forget', { minutes: 10 }); notice(`Deleted ${r.frames_deleted} frame(s). ${r.next}`); await load(); } catch (e) { notice(e.message, 'error'); }
      } }, 'Delete last 10 minutes')),
  ].filter(Boolean));
  if (!trainingFocused && location.hash === '#training') {
    trainingFocused = true;
    $('#training').scrollIntoView({ block: 'start' });
    $('#training').focus({ preventScroll: true });
  }
}

// ---------------------------------------------------------------- wiring

document.addEventListener('click', (e) => {
  const b = e.target.closest('button[data-act], button[data-toggle]');
  if (!b) return;
  const action = b.dataset.act || b.dataset.action;
  if (b.dataset.confirm && !confirm(b.dataset.confirm)) return;
  if (action === 'enable') notice('Enabling. If your desktop shows a permission dialog, allow it there.');
  act(action);
});

if (new URLSearchParams(location.search).has('login')) {
  // Keep the flag for this view, but never leave a sign-in token or flag in history.
  setTimeout(() => history.replaceState(null, '', `/control/${location.hash}`), 0);
}
if ('serviceWorker' in navigator) navigator.serviceWorker.register('/control/sw.js', { scope: '/control/' }).catch(() => {});
load();
setInterval(() => { if (S && document.visibilityState === 'visible') load(); }, 5000);
document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible') load(); });
