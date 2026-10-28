// w3bs.org/fleet: renders the signed-in person's fleet (joined machines merged with the
// OpenDevice inventory, through @profullstack/fleet on the server) and refreshes every 10 s.
const data = JSON.parse(document.getElementById('fleet-data').textContent);
const list = document.getElementById('fleet-list');
const summaryBox = document.getElementById('fleet-summary');
const devicesBox = document.getElementById('fleet-devices');
const ACTIONS = data.actions;
const HISTORY = data.history;

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

// Device classes are the OpenDevice profiles.
const PROFILES = {
  server: 'Server',
  desktop: 'Desktop',
  phone: 'Phone',
  tablet: 'Tablet',
  other: 'Other',
};
const ICONS = {
  server: ['M4 4h16v6H4z', 'M4 14h16v6H4z', 'M7 7h.01', 'M7 17h.01'],
  desktop: ['M3 4h18v12H3z', 'M8 20h8', 'M12 16v4'],
  phone: ['M7 2h10v20H7z', 'M11 18h2'],
  tablet: ['M4 3h16v18H4z', 'M11 17h2'],
  other: ['M12 3l9 5v8l-9 5-9-5V8z'],
  none: [
    'M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18',
    'M9.5 9.5a2.5 2.5 0 1 1 3 2.4c-.6.2-1 .7-1 1.3v.8',
    'M12 17h.01',
  ],
};
const SVG = 'http://www.w3.org/2000/svg';
function icon(profile) {
  const svg = document.createElementNS(SVG, 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('aria-hidden', 'true');
  svg.setAttribute('class', 'fleet-icon');
  for (const d of ICONS[profile] || ICONS.none) {
    const path = document.createElementNS(SVG, 'path');
    path.setAttribute('d', d);
    svg.append(path);
  }
  return svg;
}
const classLabel = (profile) =>
  el('span', { className: 'fleet-class' }, [
    icon(profile),
    el('span', { textContent: PROFILES[profile] || 'Not in inventory' }),
  ]);
const money = (amount, currency) =>
  new Intl.NumberFormat('en-US', { style: 'currency', currency: currency || 'USD' }).format(amount);
const STATES = {
  unstated: 'unstated',
  grouped: 'shared bill',
  retired: 'retired',
  ended: 'ended',
  once: 'one-off',
};
const monthlyText = (monthly) => {
  if (!monthly) return '—';
  if (monthly.state === 'stated') return money(monthly.amount, monthly.currency);
  return STATES[monthly.state] || monthly.state;
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
  const device = machine.device;
  const facts = [
    ['Device', device && `${PROFILES[device.profile] || device.profile} · ${device.name}`],
    ['Role', device?.role],
    ['Provider', device?.provider],
    ['Location', device?.location],
    [
      'Cost',
      device &&
        `${monthlyText(device.monthly)}${device.monthly?.state === 'stated' ? ' / mo' : ''}`,
    ],
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
    [
      'Screen',
      info.screen &&
        `${info.screen.mode === 'control' ? 'view and control' : 'view only'} · ${
          info.screen.reachable ? 'VNC' : 'no VNC server answering on'
        } ${info.screen.vnc || ''}`,
    ],
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
              ` ${HISTORY[a.action] || a.action} · ${ago(a.created_at)}`,
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
        icon(device?.profile),
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
      info.screen
        ? el('a', {
            className: `small-button fleet-screen-link${machine.screen_listening ? ' live' : ''}`,
            href: `/fleet/${encodeURIComponent(machine.id)}/screen`,
            textContent: 'View screen',
            title: machine.screen_listening
              ? 'Watch this machine\u2019s desktop'
              : 'The agent is not connected for screen sharing right now',
          })
        : null,
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

// One row per device: joined machines first (with or without an inventory device),
// then inventory devices that have no agent.
function deviceTable(view) {
  const rows = [
    ...view.machines.map((m) => ({
      profile: m.device?.profile,
      name: m.device && m.device.name !== m.name ? `${m.name} (${m.device.name})` : m.name,
      provider: m.device?.provider,
      agent: el('span', {
        className: `pill ${m.online ? 'green' : ''}`,
        textContent: m.online ? 'online' : 'offline',
      }),
      monthly: m.device?.monthly,
      inInventory: Boolean(m.device),
    })),
    ...view.devices.map((d) => ({
      profile: d.profile,
      name: d.name,
      provider: d.provider,
      role: d.role,
      agent: el('a', {
        className: 'inline-link fleet-join-hint',
        href: '#fleet-name',
        textContent:
          d.profile === 'server' || d.profile === 'desktop' ? 'no agent: join' : 'no agent',
        title:
          d.profile === 'server' || d.profile === 'desktop'
            ? `Make a join command below and run it on ${d.name}; it matches by name or hostname.`
            : 'Phones and tablets are listed from the inventory; the W3BS app agent is not out yet.',
      }),
      monthly: d.monthly,
      inInventory: true,
    })),
  ];
  if (!rows.length) return null;
  const s = view.summary;
  return el('table', { className: 'fleet-table' }, [
    el('thead', {}, [
      el('tr', {}, [
        el('th', { scope: 'col', textContent: 'Class' }),
        el('th', { scope: 'col', textContent: 'Device' }),
        el('th', { scope: 'col', className: 'fleet-col-provider', textContent: 'Provider' }),
        el('th', { scope: 'col', textContent: 'Agent' }),
        el('th', { scope: 'col', className: 'fleet-num', textContent: '$/mo' }),
      ]),
    ]),
    el(
      'tbody',
      {},
      rows.map((r) =>
        el('tr', { className: r.inInventory ? '' : 'fleet-row-extra' }, [
          el('td', {}, [classLabel(r.profile)]),
          el('td', {}, [
            el('span', { className: 'fleet-device-name', textContent: r.name }),
            r.role
              ? el('span', { className: 'muted fleet-device-role', textContent: r.role })
              : null,
          ]),
          el('td', { className: 'fleet-col-provider', textContent: r.provider || '—' }),
          el('td', {}, [r.agent]),
          el('td', {
            className: `fleet-num${r.monthly?.state === 'stated' ? '' : ' muted'}`,
            textContent: monthlyText(r.monthly),
          }),
        ]),
      ),
    ),
    s.devices
      ? el('tfoot', {}, [
          el('tr', {}, [
            el('th', {
              scope: 'row',
              colSpan: 2,
              className: 'fleet-total-label',
              textContent: 'Monthly total',
            }),
            el('td', { className: 'fleet-col-provider' }),
            el('td', {
              className: 'muted',
              textContent: s.unstated.length ? `${s.unstated.length} unstated` : '',
            }),
            el('td', { className: 'fleet-num fleet-total', textContent: s.monthly_text }),
          ]),
        ])
      : null,
  ]);
}

function summary(view) {
  const s = view.summary;
  const stats = s.devices
    ? [
        [String(s.devices), s.devices === 1 ? 'device' : 'devices'],
        [String(s.machines), 'joined'],
        [String(s.agentless), 'without an agent'],
        [s.monthly_text, 'a month'],
        [String(s.unstated.length), 'cost unstated'],
      ]
    : [[String(s.machines), s.machines === 1 ? 'machine' : 'machines']];
  summaryBox.replaceChildren(
    ...stats.map(([value, label]) =>
      el('div', { className: 'fleet-stat' }, [
        el('strong', { textContent: value }),
        el('span', { className: 'muted', textContent: label }),
      ]),
    ),
  );
  const state = document.getElementById('fleet-inventory-state');
  if (state)
    state.textContent = s.inventory_updated_at
      ? `Inventory: ${s.devices} devices, imported ${ago(s.inventory_updated_at)}. Importing again replaces it.`
      : 'No inventory yet.';
}

function render(view) {
  summary(view);
  const table = deviceTable(view);
  devicesBox.replaceChildren(...(table ? [table] : []));
  list.replaceChildren(
    ...(view.machines.length
      ? view.machines.map(card)
      : view.devices.length
        ? []
        : [
            el('div', { className: 'empty' }, [
              el('h2', { textContent: 'No devices yet.' }),
              el('p', {
                textContent:
                  'Make a join command below and run it on a server, or import an OpenDevice inventory.',
              }),
            ]),
          ]),
  );
}
render(data);

let timer;
async function refresh() {
  try {
    const open = new Set(
      [...list.querySelectorAll('details[open]')].map(
        (d) => d.closest('.fleet-card')?.querySelector('h3')?.textContent,
      ),
    );
    render(await call('/api/fleet'));
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
        screen: values.get('screen') || undefined,
      },
    });
    document.getElementById('fleet-command').textContent = command;
    document.querySelector('.fleet-command').hidden = false;
  } catch (error) {
    status.textContent = error.message;
  }
});

const inventoryForm = document.querySelector('form[data-form="fleet-inventory"]');
inventoryForm?.addEventListener('submit', async (event) => {
  event.preventDefault();
  const status = inventoryForm.querySelector('.form-status');
  status.textContent = '';
  const file = inventoryForm.querySelector('input[type="file"]').files[0];
  if (!file) return;
  try {
    let doc;
    try {
      doc = JSON.parse(await file.text());
    } catch {
      throw new Error(`${file.name} is not JSON.`);
    }
    const saved = await call('/api/fleet/inventory', { method: 'PUT', body: doc });
    status.textContent = `Imported ${saved.devices} devices${saved.warnings.length ? ` (${saved.warnings.length} warnings)` : ''}.`;
    inventoryForm.reset();
    await refresh();
  } catch (error) {
    status.textContent = error.message;
  }
});
