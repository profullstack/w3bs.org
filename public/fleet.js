// w3bs.org/fleet: renders the signed-in person's machines and refreshes every 10 s.
const data = JSON.parse(document.getElementById('fleet-data').textContent);
const list = document.getElementById('fleet-list');
const ACTIONS = data.actions;

async function call(path, { method = 'GET', body } = {}) {
  const response = await fetch(path, {
    method,
    headers: body === undefined ? {} : { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
    credentials: 'same-origin',
  });
  const result = await response.json().catch(() => ({}));
  if (!response.ok)
    throw new Error(result.error?.message || `Request failed (${response.status}).`);
  return result;
}

const el = (tag, { dataset, ...props } = {}, children = []) => {
  const node = Object.assign(document.createElement(tag), props);
  Object.assign(node.dataset, dataset);
  for (const child of [].concat(children)) if (child != null) node.append(child);
  return node;
};
const bytes = (n) => {
  if (!Number.isFinite(n)) return '?';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024;
    i++;
  }
  return `${n.toFixed(n < 10 && i ? 1 : 0)} ${units[i]}`;
};
const pct = (part) =>
  part && part.total ? Math.round(((part.total - part.available) / part.total) * 100) : null;
const ago = (iso) => {
  if (!iso) return 'never';
  const s = Math.max(0, Math.round((Date.now() - Date.parse(iso)) / 1000));
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86400) return `${Math.round(s / 3600)}h ago`;
  return `${Math.round(s / 86400)}d ago`;
};
const uptime = (s) => {
  if (!Number.isFinite(s)) return '?';
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  return d ? `${d}d ${h}h` : `${h}h ${Math.floor((s % 3600) / 60)}m`;
};
const meter = (label, value, detail) =>
  el('div', { className: 'fleet-meter' }, [
    el('span', { className: 'fleet-meter-label', textContent: label }),
    el('span', { className: 'fleet-bar' }, [
      el('span', {
        className: `fleet-fill${value >= 90 ? ' hot' : ''}`,
      }),
    ]),
    el('span', { className: 'fleet-meter-value', textContent: value == null ? '?' : `${value}%` }),
    el('span', { className: 'muted fleet-meter-detail', textContent: detail }),
  ]);

function card(machine) {
  const info = machine.info || {};
  const memPct = pct(info.mem);
  const diskPct = pct(info.disk);
  const meters = [
    meter(
      'Memory',
      memPct,
      info.mem ? `${bytes(info.mem.total - info.mem.available)} of ${bytes(info.mem.total)}` : '',
    ),
    meter(
      'Disk',
      diskPct,
      info.disk
        ? `${bytes(info.disk.total - info.disk.available)} of ${bytes(info.disk.total)} on ${info.disk.mount || '/'}`
        : '',
    ),
  ];
  // Width via the CSSOM, which a style-src 'self' CSP allows (inline style attributes it does not).
  meters.forEach((node, index) => {
    const value = index ? diskPct : memPct;
    node.querySelector('.fleet-fill').style.width = `${Math.min(100, value ?? 0)}%`;
  });
  const facts = [
    ['Host', info.hostname],
    ['OS', info.os],
    ['Kernel', info.kernel && `${info.kernel} ${info.arch || ''}`],
    ['CPUs', info.cpus],
    ['Load', info.load?.map((v) => v.toFixed(2)).join(' · ')],
    ['Uptime', uptime(info.uptime_s)],
    ['w3bs', info.w3bs_version],
    [
      'Desktop layer',
      info.desktop
        ? info.desktop.installed
          ? info.desktop.active
            ? 'running'
            : 'installed, stopped'
          : 'not installed'
        : '?',
    ],
    ['Agent user', info.user],
  ].filter(([, v]) => v != null && v !== '');
  const actionButtons = machine.allow_actions
    ? Object.entries(ACTIONS).map(([action, label]) =>
        el('button', {
          className: 'small-button',
          type: 'button',
          textContent: label,
          dataset: { action: 'queue', id: machine.id, name: action },
        }),
      )
    : [
        el('p', {
          className: 'muted fleet-note',
          textContent: 'Actions are off for this machine (it joined without them).',
        }),
      ];
  const history = machine.actions.length
    ? el('details', { className: 'fleet-history' }, [
        el('summary', { textContent: `Recent actions (${machine.actions.length})` }),
        el(
          'ul',
          {},
          machine.actions.map((a) =>
            el('li', {}, [
              el('span', {
                className: `pill ${a.status === 'done' ? 'green' : a.status === 'failed' ? 'red' : ''}`,
                textContent: a.status,
              }),
              ` ${ACTIONS[a.action] || a.action} · ${ago(a.created_at)}`,
              a.output
                ? el('pre', { className: 'instructions', textContent: a.output.slice(-2000) })
                : null,
            ]),
          ),
        ),
      ])
    : null;
  return el('article', { className: 'inspector-panel fleet-card' }, [
    el('div', { className: 'fleet-head' }, [
      el('h3', {}, [
        el('span', {
          className: `fleet-dot ${machine.online ? 'on' : ''}`,
          title: machine.online ? 'online' : 'offline',
        }),
        ` ${machine.name}`,
      ]),
      el('span', {
        className: 'muted',
        textContent: machine.online
          ? `online · ${ago(machine.last_seen_at)}`
          : `offline · last seen ${ago(machine.last_seen_at)}`,
      }),
    ]),
    el('div', { className: 'fleet-meters' }, meters),
    el(
      'dl',
      { className: 'fleet-facts' },
      facts.flatMap(([k, v]) => [
        el('dt', { textContent: k }),
        el('dd', { textContent: String(v) }),
      ]),
    ),
    el('div', { className: 'fleet-actions' }, [
      ...actionButtons,
      el('button', {
        className: 'small-button fleet-remove',
        type: 'button',
        textContent: 'Remove',
        dataset: { action: 'remove', id: machine.id, name: machine.name },
      }),
    ]),
    history,
  ]);
}

function render(machines) {
  list.replaceChildren(
    ...(machines.length
      ? machines.map(card)
      : [
          el('div', { className: 'empty' }, [
            el('h2', { textContent: 'No servers yet.' }),
            el('p', { textContent: 'Make a join command below and run it on a server.' }),
          ]),
        ]),
  );
}
render(data.machines);

let timer;
async function refresh() {
  try {
    const open = new Set(
      [...list.querySelectorAll('details[open]')].map(
        (d) => d.closest('.fleet-card')?.querySelector('h3')?.textContent,
      ),
    );
    render(await call('/api/fleet/machines'));
    for (const d of list.querySelectorAll('details'))
      if (open.has(d.closest('.fleet-card')?.querySelector('h3')?.textContent)) d.open = true;
  } catch {
    // Keep showing the last state; the next tick tries again.
  }
  clearTimeout(timer);
  timer = setTimeout(refresh, 10_000);
}
timer = setTimeout(refresh, 10_000);

document.addEventListener('click', async (event) => {
  const button = event.target.closest('button[data-action]');
  if (!button) return;
  const { action, id, name } = button.dataset;
  if (action === 'copy-command') {
    await navigator.clipboard
      ?.writeText(document.getElementById('fleet-command').textContent)
      .catch(() => {});
    button.textContent = 'Copied';
    return;
  }
  if (action === 'remove' && !confirm(`Remove ${name} from your fleet? Its agent stops reporting.`))
    return;
  button.disabled = true;
  try {
    if (action === 'queue')
      await call(`/api/fleet/machines/${encodeURIComponent(id)}/actions`, {
        method: 'POST',
        body: { action: name },
      });
    if (action === 'remove')
      await call(`/api/fleet/machines/${encodeURIComponent(id)}`, { method: 'DELETE' });
    await refresh();
  } catch (error) {
    alert(error.message);
  } finally {
    button.disabled = false;
  }
});

const form = document.querySelector('form[data-form="fleet-token"]');
form.addEventListener('submit', async (event) => {
  event.preventDefault();
  const status = form.querySelector('.form-status');
  status.textContent = '';
  try {
    const values = new FormData(form);
    const { command } = await call('/api/fleet/tokens', {
      method: 'POST',
      body: {
        name: values.get('name') || undefined,
        allowActions: values.get('allowActions') === '1',
      },
    });
    document.getElementById('fleet-command').textContent = command;
    document.querySelector('.fleet-command').hidden = false;
  } catch (error) {
    status.textContent = error.message;
  }
});
