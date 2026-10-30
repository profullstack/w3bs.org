// W3BS fleet: servers a signed-in person enrolls with `w3bs fleet join <token>`. Each
// one runs `w3bs fleet agent`, which reports its health and picks up actions. Actions
// are a fixed allowlist, queued only by the owner, and only on machines that joined
// with actions allowed; the agent enforces the same list on its side.
//
// An account can also store an OpenDevice inventory (logicsrc.com/docs/opendevice):
// every device it runs, servers to phones, with what each costs a month. The fleet
// view merges it with the enrolled machines through @profullstack/fleet, so devices
// that never ran `w3bs fleet join` still show up, with their class and cost.
import { createHash, randomBytes } from 'node:crypto';
import {
  fleetCost,
  formatMonthly,
  matchAgents,
  parseInventory,
  validateInventory,
} from '@profullstack/fleet';
import { W3bsError } from './core.mjs';

const tables = () => `
  CREATE TABLE IF NOT EXISTS fleet_tokens (token_hash TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, name TEXT NOT NULL, allow_actions INTEGER NOT NULL, created_at TEXT NOT NULL, expires_at TEXT NOT NULL, used_at TEXT);
  CREATE TABLE IF NOT EXISTS fleet_machines (id TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, name TEXT NOT NULL, agent_key_hash TEXT NOT NULL UNIQUE, allow_actions INTEGER NOT NULL, info TEXT NOT NULL, created_at TEXT NOT NULL, last_seen_at TEXT);
  CREATE INDEX IF NOT EXISTS fleet_machines_user ON fleet_machines(user_id);
  CREATE TABLE IF NOT EXISTS fleet_actions (id TEXT PRIMARY KEY, machine_id TEXT NOT NULL REFERENCES fleet_machines(id) ON DELETE CASCADE, user_id TEXT NOT NULL, action TEXT NOT NULL, status TEXT NOT NULL, output TEXT NOT NULL, created_at TEXT NOT NULL, started_at TEXT, finished_at TEXT);
  CREATE INDEX IF NOT EXISTS fleet_actions_machine ON fleet_actions(machine_id, created_at);
  CREATE TABLE IF NOT EXISTS fleet_machine_permissions (machine_id TEXT NOT NULL REFERENCES fleet_machines(id) ON DELETE CASCADE, permission TEXT NOT NULL, enabled INTEGER NOT NULL, updated_by TEXT NOT NULL, updated_at TEXT NOT NULL, PRIMARY KEY(machine_id, permission));
  CREATE TABLE IF NOT EXISTS fleet_inventories (user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE, document TEXT NOT NULL, updated_at TEXT NOT NULL);`;
export const FLEET_SCHEMA = { sqlite: tables(), postgres: tables() };

export const ACTIONS = {
  refresh: 'Report now',
  update: 'Update w3bs packages',
  'restart-desktop': 'Restart the desktop layer',
};
// Every Fleet task is checked against a named permission on the server. New grants
// are off unless an owner explicitly enables them. `available` separates permissions
// enforced by today's agent from future tools that must not be advertised as working.
export const PERMISSIONS = {
  'system.refresh': {
    label: 'Report machine health',
    description: 'Ask this machine to send a fresh Fleet report.',
    available: true,
  },
  'system.update': {
    label: 'Update w3bs packages',
    description: 'Install w3bs package updates on this machine.',
    available: true,
  },
  'desktop.restart': {
    label: 'Restart the desktop layer',
    description: 'Restart the local Agentic Desktop Layer service.',
    available: true,
  },
  'desktop.view': {
    label: 'View the desktop',
    description:
      'Watch this machine through its configured screen relay. A control-mode relay also requires the control grant because RFB is passed through unchanged.',
    available: true,
  },
  'desktop.control': {
    label: 'Control the desktop',
    description: 'Send keyboard and pointer input through the configured screen relay.',
    available: true,
  },
  'terminal.shell': {
    label: 'Terminal shell (HQTerm)',
    description: 'Run a scoped terminal task as the machine agent user.',
    available: false,
  },
  'files.read': {
    label: 'Read files',
    description: 'Read approved files from this machine.',
    available: false,
  },
  'files.write': {
    label: 'Write files',
    description: 'Change approved files on this machine.',
    available: false,
  },
  'browser.devtools': {
    label: 'Browser DevTools',
    description: 'Use browser debugging tools through the Fleet permission broker.',
    available: false,
  },
  'desktop.apps': {
    label: 'Use desktop apps',
    description: 'Run approved desktop applications through the local desktop broker.',
    available: false,
  },
};
const ACTION_PERMISSION = {
  refresh: 'system.refresh',
  update: 'system.update',
  'restart-desktop': 'desktop.restart',
};
// Labels for the history list: the actions above plus what the relay logs.
export const HISTORY = { ...ACTIONS, screen: 'Screen session' };
export const SCREEN_MODES = ['view', 'control'];
// The limits /pricing states; change them here and the page follows.
export const FLEET_LIMITS = {
  machines: 50, // per account
  intervalSeconds: 30, // the agent reports this often
  offlineAfterSeconds: 90, // no report for this long reads as offline
  tokenMinutes: 60, // a join token works once, for this long
  pendingActions: 5, // queued or running, per machine
  actionTimeoutMinutes: 20,
  actionHistory: 20, // newest actions and screen sessions kept per machine
  actionOutputKB: 16, // the tail of an action's output that is kept
  inventoryDevices: 500, // devices in one OpenDevice inventory
  inventoryKB: 256, // the largest inventory document stored
};
const INTERVAL = FLEET_LIMITS.intervalSeconds;
const ONLINE_WITHIN = FLEET_LIMITS.offlineAfterSeconds * 1000;
const TOKEN_TTL = FLEET_LIMITS.tokenMinutes * 60 * 1000;
const ACTION_TIMEOUT = FLEET_LIMITS.actionTimeoutMinutes * 60 * 1000;
const MAX_MACHINES = FLEET_LIMITS.machines;
const HISTORY_KEPT = FLEET_LIMITS.actionHistory;

const random = (bytes = 32) => randomBytes(bytes).toString('base64url');
const sha256 = (value) => createHash('sha256').update(value).digest('hex');
const nowIso = () => new Date().toISOString();
const later = (ms) => new Date(Date.now() + ms).toISOString();
const fail = (code, message, status = 400) => new W3bsError(code, message, status);

function cleanName(value, fallback) {
  const name = String(value ?? '')
    .replace(/[^\p{L}\p{N} ._-]/gu, '')
    .trim()
    .slice(0, 60);
  return name || fallback;
}
// Only the fields the page shows, with types forced, so an agent cannot store
// arbitrary blobs or markup.
function cleanInfo(info) {
  const source = info && typeof info === 'object' ? info : {};
  const text = (value, max = 120) => (typeof value === 'string' ? value.slice(0, max) : undefined);
  const num = (value) => (Number.isFinite(value) && value >= 0 ? value : undefined);
  const pair = (value, keys) =>
    value && typeof value === 'object'
      ? Object.fromEntries(
          keys.map((key) => [key, num(value[key])]).filter(([, v]) => v !== undefined),
        )
      : undefined;
  const load = Array.isArray(source.load) ? source.load.slice(0, 3).map(num) : undefined;
  return {
    hostname: text(source.hostname, 253),
    os: text(source.os),
    kernel: text(source.kernel),
    arch: text(source.arch, 32),
    cpus: num(source.cpus),
    uptime_s: num(source.uptime_s),
    load: load?.every((v) => v !== undefined) ? load : undefined,
    mem: pair(source.mem, ['total', 'available']),
    disk: source.disk && {
      mount: text(source.disk.mount, 64),
      ...pair(source.disk, ['total', 'available']),
    },
    w3bs_version: text(source.w3bs_version, 32),
    desktop:
      source.desktop && typeof source.desktop === 'object'
        ? { installed: Boolean(source.desktop.installed), active: Boolean(source.desktop.active) }
        : undefined,
    user: text(source.user, 64),
    screen:
      source.screen && SCREEN_MODES.includes(source.screen.mode)
        ? {
            mode: source.screen.mode,
            vnc: text(source.screen.vnc, 64),
            reachable: Boolean(source.screen.reachable),
          }
        : undefined,
  };
}

export function createFleet(db, { events } = {}) {
  const machineRow = (row, actions = [], permissions = {}) => ({
    id: row.id,
    name: row.name,
    hostname: JSON.parse(row.info).hostname ?? null,
    online: Boolean(row.last_seen_at) && Date.now() - Date.parse(row.last_seen_at) < ONLINE_WITHIN,
    last_seen_at: row.last_seen_at,
    created_at: row.created_at,
    allow_actions: Boolean(Number(row.allow_actions)),
    permissions,
    info: JSON.parse(row.info),
    actions,
  });
  async function permissionMap(machine, query = db) {
    const stored = await query.all(
      'SELECT permission, enabled FROM fleet_machine_permissions WHERE machine_id = ?',
      [machine.id],
    );
    const saved = new Map(stored.map((row) => [row.permission, Boolean(Number(row.enabled))]));
    const info = JSON.parse(machine.info);
    return Object.fromEntries(
      Object.keys(PERMISSIONS).map((permission) => {
        if (saved.has(permission)) return [permission, saved.get(permission)];
        // Compatibility for machines enrolled before per-task policies shipped. The
        // old owner opt-in covers only the three fixed actions. A configured screen
        // is an explicit local opt-in for its selected view/control mode.
        if (machine.allow_actions && Object.values(ACTION_PERMISSION).includes(permission))
          return [permission, true];
        if (permission === 'desktop.view') return [permission, Boolean(info.screen?.mode)];
        if (permission === 'desktop.control') return [permission, info.screen?.mode === 'control'];
        return [permission, false];
      }),
    );
  }
  const actionRow = (row) => ({
    id: row.id,
    action: row.action,
    status: row.status,
    output: row.output,
    created_at: row.created_at,
    started_at: row.started_at,
    finished_at: row.finished_at,
  });
  async function expireActions() {
    await db.run(
      "UPDATE fleet_actions SET status = 'failed', output = 'No result from the machine in 20 minutes.', finished_at = ? WHERE status = 'running' AND started_at < ?",
      [nowIso(), later(-ACTION_TIMEOUT)],
    );
  }

  async function createToken(userId, { name, allowActions }) {
    const count = await db.get('SELECT COUNT(*) AS n FROM fleet_machines WHERE user_id = ?', [
      userId,
    ]);
    if (Number(count.n) >= MAX_MACHINES)
      throw fail('TOO_MANY_MACHINES', `A fleet holds up to ${MAX_MACHINES} machines.`, 409);
    const token = `fj_${random(24)}`;
    await db.run('DELETE FROM fleet_tokens WHERE expires_at < ?', [nowIso()]);
    await db.run('INSERT INTO fleet_tokens VALUES (?, ?, ?, ?, ?, ?, NULL)', [
      sha256(token),
      userId,
      cleanName(name, ''),
      allowActions ? 1 : 0,
      nowIso(),
      later(TOKEN_TTL),
    ]);
    return { token, expiresAt: later(TOKEN_TTL) };
  }
  async function join({ token, name, info }) {
    if (typeof token !== 'string' || !token.startsWith('fj_') || token.length > 100)
      throw fail('INVALID_TOKEN', 'That join token is not valid. Make a new one on /fleet.', 401);
    return db.transaction(async (tx) => {
      const row = await tx.get(
        'SELECT * FROM fleet_tokens WHERE token_hash = ? AND used_at IS NULL AND expires_at > ?',
        [sha256(token), nowIso()],
      );
      if (!row)
        throw fail(
          'INVALID_TOKEN',
          'That join token is used or expired. Make a new one on /fleet.',
          401,
        );
      await tx.run('UPDATE fleet_tokens SET used_at = ? WHERE token_hash = ?', [
        nowIso(),
        row.token_hash,
      ]);
      const clean = cleanInfo(info);
      const machine = {
        id: `m_${random(9)}`,
        name: cleanName(name, '') || row.name || cleanName(clean.hostname, 'server'),
        key: `fm_${random(32)}`,
      };
      await tx.run('INSERT INTO fleet_machines VALUES (?, ?, ?, ?, ?, ?, ?, ?)', [
        machine.id,
        row.user_id,
        machine.name,
        sha256(machine.key),
        row.allow_actions,
        JSON.stringify(clean),
        nowIso(),
        nowIso(),
      ]);
      const permissions = Object.fromEntries(Object.keys(PERMISSIONS).map((key) => [key, false]));
      const grants = [];
      if (Number(row.allow_actions))
        grants.push('system.refresh', 'system.update', 'desktop.restart');
      if (clean.screen?.mode) grants.push('desktop.view');
      if (clean.screen?.mode === 'control') grants.push('desktop.control');
      for (const permission of grants) {
        permissions[permission] = true;
        await tx.run('INSERT INTO fleet_machine_permissions VALUES (?, ?, 1, ?, ?)', [
          machine.id,
          permission,
          row.user_id,
          nowIso(),
        ]);
      }
      return {
        machine_id: machine.id,
        agent_key: machine.key,
        name: machine.name,
        interval: INTERVAL,
        allow_actions: Boolean(Number(row.allow_actions)),
        permissions,
      };
    });
  }
  const agentMachine = async (key) =>
    typeof key === 'string' && key.startsWith('fm_') && key.length < 100
      ? db.get('SELECT * FROM fleet_machines WHERE agent_key_hash = ?', [sha256(key)])
      : undefined;
  async function heartbeat(key, info) {
    const machine = await agentMachine(key);
    if (!machine) throw fail('UNKNOWN_MACHINE', 'This machine is not in a fleet.', 401);
    await expireActions();
    const reply = await db.transaction(async (tx) => {
      await tx.run('UPDATE fleet_machines SET info = ?, last_seen_at = ? WHERE id = ?', [
        JSON.stringify(cleanInfo(info)),
        nowIso(),
        machine.id,
      ]);
      const queued = await tx.all(
        "SELECT id, action FROM fleet_actions WHERE machine_id = ? AND status = 'queued' ORDER BY created_at",
        [machine.id],
      );
      const grants = await permissionMap(machine, tx);
      const actions = [];
      for (const action of queued) {
        const permission = ACTION_PERMISSION[action.action];
        if (!permission || !grants[permission]) {
          await tx.run(
            "UPDATE fleet_actions SET status = 'failed', output = ?, finished_at = ? WHERE id = ?",
            [
              `Permission ${permission ?? 'unknown'} was off when the agent checked in.`,
              nowIso(),
              action.id,
            ],
          );
          continue;
        }
        await tx.run("UPDATE fleet_actions SET status = 'running', started_at = ? WHERE id = ?", [
          nowIso(),
          action.id,
        ]);
        actions.push(action);
      }
      return { actions, interval: INTERVAL };
    });
    // Activation: the agent on one of this account's machines is reporting.
    await events?.firstReport(machine.user_id);
    return reply;
  }
  async function actionResult(key, id, { status, output }) {
    const machine = await agentMachine(key);
    if (!machine) throw fail('UNKNOWN_MACHINE', 'This machine is not in a fleet.', 401);
    const finished = status === 'done' ? 'done' : 'failed';
    await db.run(
      "UPDATE fleet_actions SET status = ?, output = ?, finished_at = ? WHERE id = ? AND machine_id = ? AND status = 'running'",
      [
        finished,
        String(output ?? '').slice(-FLEET_LIMITS.actionOutputKB * 1024),
        nowIso(),
        String(id),
        machine.id,
      ],
    );
    return { ok: true };
  }
  async function leave(key) {
    const machine = await agentMachine(key);
    if (machine) await db.run('DELETE FROM fleet_machines WHERE id = ?', [machine.id]);
    return { ok: true };
  }

  async function machines(userId) {
    await expireActions();
    const rows = await db.all(
      'SELECT * FROM fleet_machines WHERE user_id = ? ORDER BY name, created_at',
      [userId],
    );
    return Promise.all(
      rows.map(async (row) =>
        machineRow(
          row,
          (
            await db.all(
              'SELECT * FROM fleet_actions WHERE machine_id = ? ORDER BY created_at DESC LIMIT 10',
              [row.id],
            )
          ).map(actionRow),
          await permissionMap(row),
        ),
      ),
    );
  }
  async function owned(userId, id) {
    const row = await db.get('SELECT * FROM fleet_machines WHERE id = ? AND user_id = ?', [
      String(id),
      userId,
    ]);
    if (!row) throw fail('NOT_FOUND', 'No such machine in your fleet.', 404);
    return row;
  }
  async function queueAction(userId, id, action) {
    const machine = await owned(userId, id);
    if (!Object.hasOwn(ACTIONS, action))
      throw fail('UNKNOWN_ACTION', `Actions: ${Object.keys(ACTIONS).join(', ')}.`);
    const permission = ACTION_PERMISSION[action];
    const grants = await permissionMap(machine);
    if (!grants[permission])
      throw fail(
        'ACTIONS_OFF',
        `The “${PERMISSIONS[permission].label}” permission is off for this machine. Enable it in Fleet permissions first.`,
        409,
      );
    const pending = await db.get(
      "SELECT COUNT(*) AS n FROM fleet_actions WHERE machine_id = ? AND status IN ('queued', 'running')",
      [machine.id],
    );
    if (Number(pending.n) >= FLEET_LIMITS.pendingActions)
      throw fail(
        'BUSY',
        `${FLEET_LIMITS.pendingActions} actions are already waiting on this machine.`,
        429,
      );
    const row = { id: `a_${random(9)}`, action };
    await db.run("INSERT INTO fleet_actions VALUES (?, ?, ?, ?, 'queued', '', ?, NULL, NULL)", [
      row.id,
      machine.id,
      userId,
      action,
      nowIso(),
    ]);
    await db.run(
      `DELETE FROM fleet_actions WHERE machine_id = ? AND id NOT IN (SELECT id FROM fleet_actions WHERE machine_id = ? ORDER BY created_at DESC LIMIT ${HISTORY_KEPT})`,
      [machine.id, machine.id],
    );
    return { ...row, status: 'queued' };
  }
  async function setPermission(userId, id, permission, enabled) {
    const machine = await owned(userId, id);
    const definition = PERMISSIONS[permission];
    if (!definition) throw fail('UNKNOWN_PERMISSION', 'That Fleet permission does not exist.');
    if (!definition.available)
      throw fail(
        'PERMISSION_UNAVAILABLE',
        `${definition.label} is not supported by the current Fleet agent yet.`,
        409,
      );
    if (typeof enabled !== 'boolean')
      throw fail('INVALID_PERMISSION', 'Set enabled to true or false.');
    const screenMode = JSON.parse(machine.info).screen?.mode;
    if (enabled && permission === 'desktop.view' && !screenMode)
      throw fail(
        'SCREEN_NOT_SHARED',
        'Enable screen sharing on the machine before granting desktop viewing.',
        409,
      );
    if (enabled && permission === 'desktop.control' && screenMode !== 'control')
      throw fail(
        'SCREEN_NOT_CONTROL',
        'Set the machine screen mode to control before granting desktop control.',
        409,
      );
    await db.run(
      'INSERT INTO fleet_machine_permissions VALUES (?, ?, ?, ?, ?) ON CONFLICT(machine_id, permission) DO UPDATE SET enabled = excluded.enabled, updated_by = excluded.updated_by, updated_at = excluded.updated_at',
      [machine.id, permission, enabled ? 1 : 0, userId, nowIso()],
    );
    return { machine_id: machine.id, permissions: await permissionMap(machine) };
  }
  // A finished (or refused) screen session, kept with the machine's action history.
  async function logScreen(machineId, userId, { status, output, startedAt }) {
    await db.run("INSERT INTO fleet_actions VALUES (?, ?, ?, 'screen', ?, ?, ?, ?, ?)", [
      `a_${random(9)}`,
      machineId,
      userId,
      status === 'done' ? 'done' : 'failed',
      String(output ?? '').slice(0, 2000),
      startedAt,
      startedAt,
      nowIso(),
    ]);
    await db.run(
      `DELETE FROM fleet_actions WHERE machine_id = ? AND id NOT IN (SELECT id FROM fleet_actions WHERE machine_id = ? ORDER BY created_at DESC LIMIT ${HISTORY_KEPT})`,
      [machineId, machineId],
    );
  }
  async function removeMachine(userId, id) {
    const machine = await owned(userId, id);
    await db.run('DELETE FROM fleet_machines WHERE id = ?', [machine.id]);
    return { ok: true };
  }
  async function rename(userId, id, name) {
    const machine = await owned(userId, id);
    await db.run('UPDATE fleet_machines SET name = ? WHERE id = ?', [
      cleanName(name, machine.name),
      machine.id,
    ]);
    return { ok: true };
  }

  // ---------------------------------------------------------------- inventory
  async function inventoryRow(userId) {
    return db.get('SELECT document, updated_at FROM fleet_inventories WHERE user_id = ?', [userId]);
  }
  async function setInventory(userId, input) {
    let doc = input;
    if (typeof doc === 'string') {
      try {
        doc = JSON.parse(doc);
      } catch {
        throw fail('INVALID_INVENTORY', 'The inventory is not JSON.');
      }
    }
    if (!doc || typeof doc !== 'object' || Array.isArray(doc))
      throw fail(
        'INVALID_INVENTORY',
        'Send an OpenDevice inventory: an object with a "devices" array.',
      );
    const checked = validateInventory(doc);
    if (!checked.valid)
      throw Object.assign(
        fail(
          'INVALID_INVENTORY',
          `The inventory has ${checked.errors.length} error(s): ${checked.errors.slice(0, 5).join('; ')}`,
        ),
        { details: { errors: checked.errors, warnings: checked.warnings } },
      );
    if (doc.devices.length > FLEET_LIMITS.inventoryDevices)
      throw fail(
        'INVENTORY_TOO_LARGE',
        `An inventory holds up to ${FLEET_LIMITS.inventoryDevices} devices.`,
        413,
      );
    const text = JSON.stringify(doc);
    if (text.length > FLEET_LIMITS.inventoryKB * 1024)
      throw fail(
        'INVENTORY_TOO_LARGE',
        `An inventory is at most ${FLEET_LIMITS.inventoryKB} KB.`,
        413,
      );
    const at = nowIso();
    await db.transaction(async (tx) => {
      await tx.run('DELETE FROM fleet_inventories WHERE user_id = ?', [userId]);
      await tx.run('INSERT INTO fleet_inventories VALUES (?, ?, ?)', [userId, text, at]);
    });
    return { ok: true, devices: doc.devices.length, warnings: checked.warnings, updated_at: at };
  }
  async function inventory(userId) {
    const row = await inventoryRow(userId);
    return row ? { document: JSON.parse(row.document), updated_at: row.updated_at } : null;
  }
  async function deleteInventory(userId) {
    await db.run('DELETE FROM fleet_inventories WHERE user_id = ?', [userId]);
    return { ok: true };
  }
  // Only what the fleet page shows about an inventory device; the stored document
  // keeps everything (GET /api/fleet/inventory returns it whole).
  const deviceView = (device, monthly) => ({
    id: device.id,
    name: device.name,
    profile: device.profile,
    status: device.status,
    role: typeof device.role === 'string' ? device.role : null,
    provider: device.provider?.name ?? null,
    location:
      [device.location?.region, device.location?.country].filter(Boolean).join(', ') || null,
    hostnames: device.hostnames,
    monthly,
  });
  // The whole fleet: enrolled machines (each with its inventory device, if one
  // matches by name, hostname or id), the inventory devices without an agent, and
  // the monthly run rate. Costs stay per currency; unstated ones are never guessed.
  async function overview(userId) {
    const list = await machines(userId);
    const stored = await inventoryRow(userId);
    const summary = {
      machines: list.length,
      devices: 0,
      matched: 0,
      agentless: 0,
      monthly: {},
      monthly_text: formatMonthly({}),
      counted: 0,
      unstated: [],
      by_profile: {},
      inventory_updated_at: stored?.updated_at ?? null,
    };
    if (!stored)
      return { machines: list.map((m) => ({ ...m, device: null })), devices: [], summary };
    const inv = parseInventory(stored.document);
    const cost = fleetCost(inv);
    const monthlyOf = new Map(cost.devices.map((d) => [d.id, d.monthly]));
    // A device can name its machine outright: `agents: [{ "name": "w3bs", "id": "m_…" }]`.
    // Those pair first; the rest match by name or hostname.
    const pinned = new Map();
    for (const device of inv.devices)
      for (const agent of Array.isArray(device.agents) ? device.agents : [])
        if (agent?.name === 'w3bs' && typeof agent.id === 'string') pinned.set(agent.id, device.id);
    const ids = new Set(list.map((m) => m.id));
    const joined = matchAgents(
      inv,
      list.filter((m) => !pinned.has(m.id)),
      { hostname: (m) => m.info?.hostname, name: (m) => m.name, ip: () => null },
    );
    const deviceOf = new Map();
    const agentless = [];
    for (const { device, agents } of joined.devices) {
      const view = deviceView(device, monthlyOf.get(device.id));
      const mine = [...pinned].filter(([id, d]) => d === device.id && ids.has(id));
      for (const [id] of mine) deviceOf.set(id, view);
      if (agents.length) for (const m of agents) deviceOf.set(m.id, view);
      else if (!mine.length) agentless.push(view);
    }
    const byProfile = {};
    for (const d of inv.devices) byProfile[d.profile] = (byProfile[d.profile] ?? 0) + 1;
    return {
      machines: list.map((m) => ({ ...m, device: deviceOf.get(m.id) ?? null })),
      devices: agentless,
      summary: {
        ...summary,
        devices: inv.devices.length,
        matched: inv.devices.length - agentless.length,
        agentless: agentless.length,
        monthly: cost.monthly,
        monthly_text: formatMonthly(cost.monthly),
        counted: cost.counted,
        unstated: cost.unstated,
        by_profile: byProfile,
      },
    };
  }
  return {
    createToken,
    join,
    heartbeat,
    actionResult,
    leave,
    machines,
    queueAction,
    setPermission,
    permissions: PERMISSIONS,
    removeMachine,
    rename,
    logScreen,
    setInventory,
    inventory,
    deleteInventory,
    overview,
    // For the screen relay: the machine behind an agent key, and an owner's machine.
    machineByKey: async (key) => {
      const row = await agentMachine(key);
      return row && machineRow(row, [], await permissionMap(row));
    },
    ownedMachine: async (userId, id) => {
      const row = await owned(userId, id);
      return machineRow(row, [], await permissionMap(row));
    },
  };
}
