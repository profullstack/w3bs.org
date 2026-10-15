// W3BS fleet: servers a signed-in person enrolls with `w3bs fleet join <token>`. Each
// one runs `w3bs fleet agent`, which reports its health and picks up actions. Actions
// are a fixed allowlist, queued only by the owner, and only on machines that joined
// with actions allowed; the agent enforces the same list on its side.
import { createHash, randomBytes } from 'node:crypto';
import { W3bsError } from './core.mjs';

const tables = () => `
  CREATE TABLE IF NOT EXISTS fleet_tokens (token_hash TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, name TEXT NOT NULL, allow_actions INTEGER NOT NULL, created_at TEXT NOT NULL, expires_at TEXT NOT NULL, used_at TEXT);
  CREATE TABLE IF NOT EXISTS fleet_machines (id TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, name TEXT NOT NULL, agent_key_hash TEXT NOT NULL UNIQUE, allow_actions INTEGER NOT NULL, info TEXT NOT NULL, created_at TEXT NOT NULL, last_seen_at TEXT);
  CREATE INDEX IF NOT EXISTS fleet_machines_user ON fleet_machines(user_id);
  CREATE TABLE IF NOT EXISTS fleet_actions (id TEXT PRIMARY KEY, machine_id TEXT NOT NULL REFERENCES fleet_machines(id) ON DELETE CASCADE, user_id TEXT NOT NULL, action TEXT NOT NULL, status TEXT NOT NULL, output TEXT NOT NULL, created_at TEXT NOT NULL, started_at TEXT, finished_at TEXT);
  CREATE INDEX IF NOT EXISTS fleet_actions_machine ON fleet_actions(machine_id, created_at);`;
export const FLEET_SCHEMA = { sqlite: tables(), postgres: tables() };

export const ACTIONS = {
  refresh: 'Report now',
  update: 'Update w3bs packages',
  'restart-desktop': 'Restart the desktop layer',
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
  const machineRow = (row, actions = []) => ({
    id: row.id,
    name: row.name,
    hostname: JSON.parse(row.info).hostname ?? null,
    online: Boolean(row.last_seen_at) && Date.now() - Date.parse(row.last_seen_at) < ONLINE_WITHIN,
    last_seen_at: row.last_seen_at,
    created_at: row.created_at,
    allow_actions: Boolean(Number(row.allow_actions)),
    info: JSON.parse(row.info),
    actions,
  });
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
      return {
        machine_id: machine.id,
        agent_key: machine.key,
        name: machine.name,
        interval: INTERVAL,
        allow_actions: Boolean(Number(row.allow_actions)),
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
      for (const action of queued)
        await tx.run("UPDATE fleet_actions SET status = 'running', started_at = ? WHERE id = ?", [
          nowIso(),
          action.id,
        ]);
      return { actions: queued, interval: INTERVAL };
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
    if (!Number(machine.allow_actions))
      throw fail(
        'ACTIONS_OFF',
        'This machine joined without actions. Join it again with actions allowed to use them.',
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
  return {
    createToken,
    join,
    heartbeat,
    actionResult,
    leave,
    machines,
    queueAction,
    removeMachine,
    rename,
    logScreen,
    // For the screen relay: the machine behind an agent key, and an owner's machine.
    machineByKey: async (key) => {
      const row = await agentMachine(key);
      return row && machineRow(row);
    },
    ownedMachine: async (userId, id) => machineRow(await owned(userId, id)),
  };
}
