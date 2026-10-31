import { createNetRecords } from './net-records.mjs';
import { createHash, randomUUID } from 'node:crypto';

export const RESERVED_NET_TLDS = new Set(['w3bs', 'www', 'api', 'net', 'search', 'admin', 'root']);

const now = () => new Date().toISOString();
const json = (value) => JSON.stringify(value);
const parse = (value) => {
  try {
    return JSON.parse(value);
  } catch {
    return null;
  }
};

const TABLES = (sequence) => `
  CREATE TABLE IF NOT EXISTS net_tlds (
    tld TEXT PRIMARY KEY,
    owner_id TEXT,
    status TEXT NOT NULL,
    policy TEXT NOT NULL,
    source TEXT NOT NULL,
    created_at TEXT NOT NULL,
    moderated_at TEXT,
    moderation_note TEXT
  );
  CREATE INDEX IF NOT EXISTS net_tlds_status ON net_tlds(status, tld);
  CREATE TABLE IF NOT EXISTS net_names (
    tld TEXT NOT NULL,
    label TEXT NOT NULL,
    owner_id TEXT,
    target TEXT,
    status TEXT NOT NULL,
    source TEXT NOT NULL,
    created_at TEXT NOT NULL,
    moderated_at TEXT,
    moderation_note TEXT,
    PRIMARY KEY (tld, label)
  );
  CREATE INDEX IF NOT EXISTS net_names_status ON net_names(status, tld, label);
  CREATE TABLE IF NOT EXISTS net_dns_records (
    id TEXT PRIMARY KEY, tld TEXT NOT NULL, label TEXT NOT NULL, type TEXT NOT NULL,
    value TEXT NOT NULL, ttl INTEGER NOT NULL, priority INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL,
    UNIQUE(tld,label,type,value,priority)
  );
  CREATE INDEX IF NOT EXISTS net_dns_name ON net_dns_records(tld,label);
  CREATE TABLE IF NOT EXISTS net_events (
    seq ${sequence},
    event_id TEXT NOT NULL UNIQUE,
    type TEXT NOT NULL,
    subject TEXT NOT NULL,
    payload TEXT NOT NULL,
    actor_id TEXT,
    instance TEXT NOT NULL,
    created_at TEXT NOT NULL,
    public_event INTEGER NOT NULL
  );
  CREATE INDEX IF NOT EXISTS net_events_public ON net_events(public_event, seq);
  CREATE TABLE IF NOT EXISTS net_peers (
    id TEXT PRIMARY KEY,
    endpoint TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    status TEXT NOT NULL,
    inbound_hash TEXT,
    outbound_token TEXT,
    cursor INTEGER NOT NULL DEFAULT 0,
    inbound_cursor INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    last_seen_at TEXT,
    last_error TEXT
  );
  CREATE TABLE IF NOT EXISTS net_outbox (
    id ${sequence},
    peer_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT NOT NULL,
    delivered_at TEXT,
    last_error TEXT,
    UNIQUE(peer_id, event_id)
  );
  CREATE INDEX IF NOT EXISTS net_outbox_due ON net_outbox(delivered_at, next_attempt_at);
  CREATE TABLE IF NOT EXISTS net_foreign_records (
    source TEXT NOT NULL,
    kind TEXT NOT NULL,
    foreign_id TEXT NOT NULL,
    payload TEXT NOT NULL,
    imported_at TEXT NOT NULL,
    PRIMARY KEY(source, kind, foreign_id)
  );
  CREATE INDEX IF NOT EXISTS net_foreign_kind ON net_foreign_records(kind, source);
  CREATE TABLE IF NOT EXISTS net_import_state (
    source TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY(source, key)
  );`;

export const NET_SCHEMA = {
  sqlite: TABLES('INTEGER PRIMARY KEY AUTOINCREMENT'),
  postgres: TABLES('BIGSERIAL PRIMARY KEY'),
};

const hash = (value) => createHash('sha256').update(value).digest('hex');
const safeTld = (value) => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(String(value ?? '').toLowerCase());
const safeName = (value) => String(value ?? '').toLowerCase().split('.').length === 2 && String(value ?? '').toLowerCase().split('.').every((part) => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(part));
const publicTarget = (value) => {
  try {
    const url = new URL(value);
    return url.protocol === 'https:' && !url.username && !url.password ? url.href : null;
  } catch {
    return null;
  }
};

export function createNetworkStore(db, { instance = 'https://w3bs.org' } = {}) {
  async function federationEvents() {
    const rows = await db.all(
      "SELECT source, payload, imported_at FROM net_foreign_records WHERE kind = 'federation-event'",
    );
    rows.sort((left, right) => {
      const leftEvent = parse(left.payload);
      const rightEvent = parse(right.payload);
      const leftTime = Date.parse(leftEvent?.createdAt) || Date.parse(left.imported_at) || 0;
      const rightTime = Date.parse(rightEvent?.createdAt) || Date.parse(right.imported_at) || 0;
      return leftTime - rightTime ||
        String(left.source).localeCompare(String(right.source)) ||
        (Number(leftEvent?.sequence) || 0) - (Number(rightEvent?.sequence) || 0);
    });
    return rows;
  }

  const eventFromRow = (row) => ({
    id: row.event_id,
    sequence: Number(row.seq),
    type: row.type,
    subject: row.subject,
    data: parse(row.payload) ?? {},
    actor: row.actor_id ?? null,
    instance: row.instance,
    createdAt: row.created_at,
  });

  async function announce(tx, { type, subject, data, actor = null, publicEvent = true }) {
    const createdAt = now();
    const publicActor = actor ? hash(`w3bs-net-actor:${actor}`).slice(0, 16) : null;
    const body = { type, subject, data, actor: publicActor, instance, createdAt };
    const eventId = hash(json(body));
    await tx.run(
      'INSERT INTO net_events(event_id, type, subject, payload, actor_id, instance, created_at, public_event) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (event_id) DO NOTHING',
      [eventId, type, subject, json(data), publicActor, instance, createdAt, publicEvent ? 1 : 0],
    );
    const event = await tx.get('SELECT * FROM net_events WHERE event_id = ?', [eventId]);
    if (publicEvent) {
      const peers = await tx.all("SELECT id FROM net_peers WHERE kind = 'w3bs' AND status = 'active'");
      for (const peer of peers)
        await tx.run(
          'INSERT INTO net_outbox(peer_id, event_id, next_attempt_at) VALUES (?, ?, ?) ON CONFLICT (peer_id, event_id) DO NOTHING',
          [peer.id, eventId, createdAt],
        );
    }
    return eventFromRow(event);
  }

  async function ensureDefaults() {
    const createdAt = now();
    await db.transaction(async (tx) => {
      const selfAnnouncement = await tx.get(
        'SELECT event_id FROM net_events WHERE type = ? AND subject = ?',
        ['instance.announce', instance],
      );
      if (!selfAnnouncement)
        await announce(tx, {
          type: 'instance.announce',
          subject: instance,
          data: {
            instance,
            protocol: 'W3BS-NET-1-draft-1',
            registry: `${instance}/api/net/v1/registry`,
            events: `${instance}/api/net/v1/events`,
          },
        });
      for (const tld of ['agent', 'whatever']) {
        const namespace = await tx.get('SELECT tld FROM net_tlds WHERE tld = ?', [tld]);
        if (namespace) continue;
        const policy = { open: true, noSales: true };
        await tx.run(
          'INSERT INTO net_tlds(tld, owner_id, status, policy, source, created_at, moderated_at) VALUES (?, ?, ?, ?, ?, ?, ?)',
          [tld, null, 'active', json(policy), 'w3bs', createdAt, createdAt],
        );
        await announce(tx, {
          type: 'namespace.activated',
          subject: `.${tld}`,
          data: { tld, policy },
        });
      }
      for (const label of ['riotcoder', 'cha0spunk']) {
        const name = await tx.get('SELECT label FROM net_names WHERE tld = ? AND label = ?', [
          'agent',
          label,
        ]);
        if (!name) {
          await tx.run(
            'INSERT INTO net_names(tld, label, owner_id, target, status, source, created_at, moderated_at, moderation_note) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)',
            [
              'agent',
              label,
              null,
              null,
              'active',
              'w3bs-operator-seed',
              createdAt,
              createdAt,
              'Operator-seeded at the network launch; claim ownership through W3BS Net.',
            ],
          );
          await announce(tx, {
            type: 'name.activated',
            subject: `${label}.agent`,
            data: { name: `${label}.agent`, namespace: 'agent', claimed: false },
          });
        }
      }
    });
  }

  async function registry({ query = '', limit = 100 } = {}) {
    const q = String(query).trim().toLowerCase();
    const safeLimit = Math.max(1, Math.min(200, Number(limit) || 100));
    const local = await db.all(
      `SELECT n.tld, n.label, n.target, n.status, n.source, n.created_at,
              t.policy, n.owner_id
         FROM net_names n JOIN net_tlds t ON t.tld = n.tld
        WHERE n.status = 'active' AND t.status = 'active'
          AND (? = '' OR lower(n.label || '.' || n.tld) LIKE ?)
        ORDER BY n.tld, n.label LIMIT ?`,
      [q, `%${q}%`, safeLimit],
    );
    const reservedNames = await db.all('SELECT tld, label FROM net_names');
    const localKeys = new Set(reservedNames.map((row) => `${row.label}.${row.tld}`));
    const remaining = Math.max(0, safeLimit - local.length);
    const imported = remaining
      ? await db.all(
          `SELECT source, foreign_id, payload, imported_at FROM net_foreign_records f
            WHERE kind = 'moshpit-name' AND (? = '' OR lower(foreign_id) LIKE ?)
              AND NOT EXISTS (SELECT 1 FROM net_names n
                               WHERE (n.label || '.' || n.tld)=f.foreign_id)
            ORDER BY foreign_id LIMIT ?`,
          [q, `%${q}%`, remaining],
        )
      : [];
    const namespaces = await db.all(
      "SELECT tld, status, source, created_at, policy FROM net_tlds WHERE status = 'active' ORDER BY tld",
    );
    const localTlds = await db.all('SELECT tld, status FROM net_tlds');
    const localTldSet = new Set(localTlds.filter((row) => row.status !== 'active').map((row) => row.tld));
    const importedNamespaces = await db.all(
      "SELECT source, foreign_id, payload, imported_at FROM net_foreign_records WHERE kind = 'moshpit-tld' ORDER BY foreign_id LIMIT ?",
      [safeLimit],
    );
    const federationRows = await federationEvents();
    const remoteNames = new Map();
    const remoteNamespaces = new Map();
    for (const row of federationRows) {
      const event = parse(row.payload);
      if (!event) continue;
      if (event.type === 'namespace.activated' || event.type === 'namespace.suspended') {
        const tld = event.data?.tld || event.subject?.replace(/^\./, '');
        if (safeTld(tld)) remoteNamespaces.set(tld, { tld, source: row.source, status: event.type === 'namespace.activated' ? 'federated' : 'suspended', createdAt: event.createdAt });
      }
      if (event.type === 'name.activated' || event.type === 'name.suspended') {
        const name = event.data?.name || event.subject;
        if (safeName(name)) remoteNames.set(name, { event, source: row.source, importedAt: row.imported_at });
      }
    }
    const suspendedTlds = new Set([...remoteNamespaces.values()].filter((row) => row.status === 'suspended').map((row) => row.tld));
    const remote = [...remoteNames.entries()]
      .filter(([name, value]) => value.event.type === 'name.activated' && !localKeys.has(name) && !suspendedTlds.has(name.split('.')[1]) && !localTldSet.has(name.split('.')[1]))
      .filter(([name]) => !q || name.toLowerCase().includes(q))
      .map(([name, value]) => ({
        name,
        target: publicTarget(value.event.data?.target),
        source: value.source,
        importedAt: value.importedAt,
        status: 'federated',
        claimed: Boolean(value.event.data?.claimed),
      }));
    const moshpit = imported.filter((row) => !localTldSet.has(row.foreign_id.split('.')[1])).map((row) => ({
      ...(parse(row.payload) ?? {}),
      name: row.foreign_id,
      source: row.source,
      importedAt: row.imported_at,
      status: 'backfill',
      claimed: true,
    }));
    const remoteKeys = new Set(remote.map((row) => row.name));
    const backfill = [...remote, ...moshpit.filter((row) => !localKeys.has(row.name) && !remoteKeys.has(row.name))].slice(0, remaining);
    return {
      instance,
      namespaces: [
        ...namespaces.map((row) => ({
          tld: row.tld,
          status: row.status,
          source: row.source,
          createdAt: row.created_at,
          policy: parse(row.policy) ?? {},
        })),
        ...importedNamespaces
          .filter((row) => !localTlds.some((localTld) => localTld.tld === row.foreign_id))
          .map((row) => ({ ...(parse(row.payload) ?? {}), tld: row.foreign_id, source: row.source, status: 'backfill', importedAt: row.imported_at })),
        ...[...remoteNamespaces.values()].filter((row) => row.status === 'federated' && !localTlds.some((localTld) => localTld.tld === row.tld)),
      ],
      names: [
        ...local.map((row) => ({
          name: `${row.label}.${row.tld}`,
          tld: row.tld,
          label: row.label,
          target: row.target,
          status: row.status,
          source: 'w3bs',
          claimed: Boolean(row.owner_id),
          createdAt: row.created_at,
        })),
        ...backfill,
      ],
    };
  }

  async function resolveName(name) {
    const [label, tld] = String(name ?? '').toLowerCase().split('.');
    const local = await db.get(
      `SELECT n.tld, n.label, n.target, n.status, n.owner_id, n.created_at, t.policy
         FROM net_names n JOIN net_tlds t ON t.tld = n.tld
        WHERE n.tld = ? AND n.label = ? AND n.status = 'active' AND t.status = 'active'`,
      [tld, label],
    );
    if (local)
      return {
        name: `${label}.${tld}`,
        source: 'w3bs',
        status: 'active',
        target: local.target,
        claimed: Boolean(local.owner_id),
        createdAt: local.created_at,
      };
    const reserved = await db.get('SELECT status, owner_id, target, created_at FROM net_names WHERE tld = ? AND label = ?', [tld, label]);
    if (reserved)
      return {
        name: `${label}.${tld}`,
        source: 'w3bs',
        status: reserved.status,
        target: null,
        claimed: Boolean(reserved.owner_id),
        createdAt: reserved.created_at,
      };
    const localNamespace = await db.get('SELECT status FROM net_tlds WHERE tld = ?', [tld]);
    if (localNamespace && localNamespace.status !== 'active')
      return { name: `${label}.${tld}`, source: 'w3bs', status: localNamespace.status, target: null, claimed: false };
    const remoteRows = await federationEvents();
    let remote = null;
    let remoteNamespaceStatus = null;
    for (const row of remoteRows) {
      const event = parse(row.payload);
      const remoteTld = event?.data?.tld || event?.subject?.replace(/^\./, '');
      if (remoteTld === tld && (event.type === 'namespace.activated' || event.type === 'namespace.suspended'))
        remoteNamespaceStatus = event.type === 'namespace.activated' ? 'active' : 'suspended';
      if (event?.subject?.toLowerCase() === `${label}.${tld}` && (event.type === 'name.activated' || event.type === 'name.suspended'))
        remote = { event, source: row.source, importedAt: row.imported_at };
    }
    if (remoteNamespaceStatus !== 'suspended' && remote?.event?.type === 'name.activated')
      return {
        name: `${label}.${tld}`,
        source: remote.source,
        status: 'federated',
        target: publicTarget(remote.event.data?.target),
        claimed: Boolean(remote.event.data?.claimed),
        importedAt: remote.importedAt,
      };
    const imported = await db.get(
      'SELECT source, payload, imported_at FROM net_foreign_records WHERE kind = ? AND foreign_id = ?',
      ['moshpit-name', `${label}.${tld}`],
    );
    if (imported)
      return {
        ...(parse(imported.payload) ?? {}),
        name: `${label}.${tld}`,
        source: imported.source,
        status: 'backfill',
        importedAt: imported.imported_at,
        claimed: true,
      };
    return null;
  }

  async function requestTld({ tld, ownerId, policy = {} }) {
    const createdAt = now();
    await db.run(
      "INSERT INTO net_tlds(tld, owner_id, status, policy, source, created_at) VALUES (?, ?, 'pending', ?, 'w3bs', ?) ON CONFLICT (tld) DO NOTHING",
      [tld, ownerId, json({ ...policy, noSales: true }), createdAt],
    );
    const row = await db.get('SELECT * FROM net_tlds WHERE tld = ?', [tld]);
    if (row.owner_id !== ownerId || row.status !== 'pending') return { created: false, row };
    await db.transaction((tx) =>
      announce(tx, {
        type: 'namespace.requested',
        subject: `.${tld}`,
        data: { tld, status: 'pending', policy: parse(row.policy) ?? {} },
        actor: ownerId,
        publicEvent: false,
      }),
    );
    return { created: true, row };
  }

  async function requestName({ tld, label, ownerId, ownerName = '', target = null, requestNamespace = false }) {
    return db.transaction(async (tx) => {
      let namespace = await tx.get('SELECT * FROM net_tlds WHERE tld = ?', [tld]);
      let namespaceRequested = false;
      const createdAt = now();
      if (!namespace && requestNamespace) {
        if (RESERVED_NET_TLDS.has(tld)) return { error: 'reserved_namespace' };
        const inserted = await tx.get(
          "INSERT INTO net_tlds(tld, owner_id, status, policy, source, created_at) VALUES (?, ?, 'pending', ?, 'w3bs', ?) ON CONFLICT (tld) DO NOTHING RETURNING *",
          [tld, ownerId, json({ open: false, noSales: true }), createdAt],
        );
        namespace = inserted || await tx.get('SELECT * FROM net_tlds WHERE tld = ?', [tld]);
        if (inserted) {
          namespaceRequested = true;
          await announce(tx, {
            type: 'namespace.requested', subject: `.${tld}`,
            data: { tld, status: 'pending', policy: parse(namespace.policy) },
            actor: ownerId, publicEvent: false,
          });
        }
      }
      if (!namespace) return { error: 'namespace_missing' };
      const ownPending = namespace.status === 'pending' && namespace.owner_id === ownerId;
      if (namespace.status !== 'active' && !ownPending)
        return { error: 'namespace_unavailable', namespaceStatus: namespace.status };
      if (namespace.owner_id && namespace.owner_id !== ownerId && !(parse(namespace.policy) ?? {}).open)
        return { error: 'namespace_owner_required' };
      const seed = await tx.get('SELECT * FROM net_names WHERE tld = ? AND label = ?', [tld, label]);
      const claim = seed?.source === 'w3bs-operator-seed' && !seed.owner_id && seed.status === 'active';
      if (claim) {
        if (String(ownerName).toLowerCase() !== label) return { error: 'seed_owner_required' };
        await tx.run(
          "UPDATE net_names SET owner_id = ?, target = ?, status = 'pending', source = 'w3bs', created_at = ? WHERE tld = ? AND label = ? AND owner_id IS NULL AND status = 'active'",
          [ownerId, target, createdAt, tld, label],
        );
      } else {
        await tx.run(
          "INSERT INTO net_names(tld, label, owner_id, target, status, source, created_at) VALUES (?, ?, ?, ?, 'pending', 'w3bs', ?) ON CONFLICT (tld, label) DO NOTHING",
          [tld, label, ownerId, target, createdAt],
        );
      }
      const row = await tx.get('SELECT * FROM net_names WHERE tld = ? AND label = ?', [tld, label]);
      if (row.owner_id !== ownerId || row.status !== 'pending') return { created: false, row };
      await announce(tx, {
        type: claim ? 'name.claim.requested' : 'name.requested', subject: `${label}.${tld}`,
        data: { name: `${label}.${tld}`, target, status: 'pending', ...(claim ? { claim: true } : {}) },
        actor: ownerId, publicEvent: false,
      });
      return { created: true, row, namespaceRequested, namespacePending: namespace.status === 'pending' };
    });
  }

  async function pending() {
    const [tlds, names] = await Promise.all([
      db.all("SELECT * FROM net_tlds WHERE status = 'pending' ORDER BY created_at"),
      db.all("SELECT * FROM net_names WHERE status = 'pending' ORDER BY created_at"),
    ]);
    return {
      namespaces: tlds.map((row) => ({
        tld: row.tld,
        owner: row.owner_id,
        policy: parse(row.policy) ?? {},
        createdAt: row.created_at,
      })),
      names: names.map((row) => ({
        name: `${row.label}.${row.tld}`,
        owner: row.owner_id,
        target: row.target,
        createdAt: row.created_at,
      })),
    };
  }

  async function managed() {
    const [tlds, names] = await Promise.all([
      db.all("SELECT tld, status, owner_id, created_at FROM net_tlds WHERE source = 'w3bs' AND status IN ('active', 'suspended') ORDER BY tld"),
      db.all("SELECT tld, label, status, owner_id, target, created_at FROM net_names WHERE source IN ('w3bs', 'w3bs-operator-seed') AND status IN ('active', 'suspended') ORDER BY tld, label"),
    ]);
    return {
      namespaces: tlds.map((row) => ({ tld: row.tld, status: row.status, owner: row.owner_id, createdAt: row.created_at })),
      names: names.map((row) => ({ name: `${row.label}.${row.tld}`, status: row.status, owner: row.owner_id, target: row.target, createdAt: row.created_at })),
    };
  }

  async function moderate({ kind, subject, action, actor, note = '' }) {
    const status = action === 'approve' || action === 'restore' ? 'active' : action === 'suspend' ? 'suspended' : 'rejected';
    const moderatedAt = now();
    if (kind === 'namespace') {
      const tld = String(subject).replace(/^\./, '').toLowerCase();
      const row = await db.get('SELECT * FROM net_tlds WHERE tld = ?', [tld]);
      if (!row) return { error: 'not_found' };
      await db.run('UPDATE net_tlds SET status = ?, moderated_at = ?, moderation_note = ? WHERE tld = ?', [
        status,
        moderatedAt,
        note.slice(0, 500),
        tld,
      ]);
      if (status === 'active' || status === 'suspended') {
        await db.transaction((tx) =>
          announce(tx, {
            type: status === 'active' ? 'namespace.activated' : 'namespace.suspended',
            subject: `.${tld}`,
            data: { tld, policy: parse(row.policy) ?? {}, status },
            actor,
          }),
        );
      }
      return { kind, subject: `.${tld}`, status };
    }
    if (kind !== 'name') return { error: 'invalid_kind' };
    const [label, tld] = String(subject).toLowerCase().split('.');
    const row = await db.get('SELECT * FROM net_names WHERE tld = ? AND label = ?', [tld, label]);
    if (!row) return { error: 'not_found' };
    if (status === 'active') {
      const namespace = await db.get('SELECT status FROM net_tlds WHERE tld = ?', [tld]);
      if (namespace?.status !== 'active') return { error: 'namespace_not_active' };
    }
    await db.run(
      'UPDATE net_names SET status = ?, moderated_at = ?, moderation_note = ? WHERE tld = ? AND label = ?',
      [status, moderatedAt, note.slice(0, 500), tld, label],
    );
    if (status === 'active' || status === 'suspended') {
      await db.transaction((tx) =>
        announce(tx, {
          type: status === 'active' ? 'name.activated' : 'name.suspended',
          subject: `${label}.${tld}`,
          data: {
            name: `${label}.${tld}`,
            target: row.target,
            status,
            claimed: Boolean(row.owner_id),
          },
          actor,
        }),
      );
    }
    return { kind, subject: `${label}.${tld}`, status };
  }

  async function events({ since = 0, limit = 100 } = {}) {
    const rows = await db.all(
      'SELECT * FROM net_events WHERE public_event = 1 AND seq > ? ORDER BY seq LIMIT ?',
      [Math.max(0, Number(since) || 0), Math.max(1, Math.min(500, Number(limit) || 100))],
    );
    return rows.map(eventFromRow);
  }

  async function peerList() {
    return (await db.all('SELECT id, endpoint, kind, status, cursor, inbound_cursor, created_at, last_seen_at, last_error FROM net_peers ORDER BY kind, endpoint')).map((row) => ({
      id: row.id,
      endpoint: row.endpoint,
      kind: row.kind,
      status: row.status,
      cursor: Number(row.cursor) || 0,
      inboundCursor: Number(row.inbound_cursor) || 0,
      createdAt: row.created_at,
      lastSeenAt: row.last_seen_at,
      lastError: row.last_error,
    }));
  }

  async function knownInstances() {
    const direct = (await peerList()).filter((peer) => peer.kind === 'w3bs' && peer.status === 'active');
    const directByEndpoint = new Map(direct.map((peer) => [peer.endpoint, peer]));
    const known = new Map([
      [instance, {
        instance,
        protocol: 'W3BS-NET-1-draft-1',
        registry: `${instance}/api/net/v1/registry`,
        events: `${instance}/api/net/v1/events`,
        local: true,
        optedIn: false,
      }],
    ]);
    for (const peer of direct) known.set(peer.endpoint, {
      instance: peer.endpoint,
      protocol: null,
      registry: `${peer.endpoint}/api/net/v1/registry`,
      events: `${peer.endpoint}/api/net/v1/events`,
      local: false,
      optedIn: true,
      lastSeenAt: peer.lastSeenAt,
    });
    for (const row of await federationEvents()) {
      const event = parse(row.payload);
      if (event?.type !== 'instance.announce') continue;
      const endpoint = event.data?.instance;
      let origin;
      try { origin = new URL(endpoint).origin; } catch { continue; }
      if (origin !== endpoint || endpoint !== event.instance || endpoint !== event.subject) continue;
      if (endpoint === instance) continue;
      const peer = directByEndpoint.get(endpoint);
      known.set(endpoint, {
        instance: endpoint,
        protocol: event.data?.protocol ?? null,
        registry: event.data?.registry ?? `${endpoint}/api/net/v1/registry`,
        events: event.data?.events ?? `${endpoint}/api/net/v1/events`,
        local: false,
        optedIn: Boolean(peer),
        lastSeenAt: peer?.lastSeenAt ?? row.imported_at,
      });
    }
    return [...known.values()].sort((left, right) => Number(right.local) - Number(left.local) || left.instance.localeCompare(right.instance));
  }

  async function addPeer({ endpoint, outboundToken }) {
    const inboundToken = randomUUID() + randomUUID();
    const existing = await db.get('SELECT id FROM net_peers WHERE endpoint = ?', [endpoint]);
    const id = existing?.id ?? randomUUID();
    if (existing) {
      await db.run(
        "UPDATE net_peers SET kind = 'w3bs', status = 'active', inbound_hash = ?, outbound_token = ?, cursor = 0, inbound_cursor = 0, last_seen_at = NULL, last_error = NULL WHERE id = ?",
        [hash(inboundToken), outboundToken, id],
      );
      await db.run('DELETE FROM net_outbox WHERE peer_id = ?', [id]);
    } else {
      await db.run(
        "INSERT INTO net_peers(id, endpoint, kind, status, inbound_hash, outbound_token, created_at) VALUES (?, ?, 'w3bs', 'active', ?, ?, ?)",
        [id, endpoint, hash(inboundToken), outboundToken, now()],
      );
    }
    let cursor = 0;
    for (;;) {
      const historical = await events({ since: cursor, limit: 500 });
      for (const event of historical)
        await db.run(
          'INSERT INTO net_outbox(peer_id, event_id, next_attempt_at) VALUES (?, ?, ?) ON CONFLICT (peer_id, event_id) DO NOTHING',
          [id, event.id, now()],
        );
      if (historical.length < 500) break;
      cursor = historical.at(-1).sequence;
    }
    return { id, inboundToken };
  }

  async function peersForWorker() {
    return db.all("SELECT * FROM net_peers WHERE status = 'active' AND kind = 'w3bs'");
  }

  async function dueOutbox(limit = 50) {
    return db.all(
      `SELECT o.id AS outbox_id, o.peer_id, o.attempts, p.endpoint, p.outbound_token,
              e.event_id, e.type, e.subject, e.payload, e.actor_id, e.instance, e.created_at, e.seq
         FROM net_outbox o JOIN net_peers p ON p.id = o.peer_id
         JOIN net_events e ON e.event_id = o.event_id
        WHERE o.delivered_at IS NULL AND o.next_attempt_at <= ? AND p.status = 'active'
        ORDER BY o.id LIMIT ?`,
      [now(), Math.max(1, Math.min(100, Number(limit) || 50))],
    );
  }

  async function markDelivered(id) {
    const at = now();
    await db.run('UPDATE net_outbox SET delivered_at = ?, last_error = NULL WHERE id = ?', [at, id]);
  }

  async function markDeliveryFailed(row, error) {
    const attempts = Number(row.attempts) + 1;
    const delay = Math.min(6 * 60 * 60 * 1000, 1000 * 2 ** Math.min(14, attempts));
    await db.run(
      'UPDATE net_outbox SET attempts = ?, next_attempt_at = ?, last_error = ? WHERE id = ?',
      [attempts, new Date(Date.now() + delay).toISOString(), String(error).slice(0, 500), row.outbox_id],
    );
    await db.run('UPDATE net_peers SET last_error = ? WHERE id = ?', [String(error).slice(0, 500), row.peer_id]);
  }

  async function markPeerSeen(id, cursor, error = null) {
    await db.run(
      'UPDATE net_peers SET cursor = ?, last_seen_at = ?, last_error = ? WHERE id = ?',
      [Number(cursor) || 0, now(), error, id],
    );
  }

  async function markPeerReceived(id, cursor) {
    await db.run(
      'UPDATE net_peers SET inbound_cursor = ?, last_seen_at = ?, last_error = NULL WHERE id = ?',
      [Number(cursor) || 0, now(), id],
    );
  }

  async function markPeerActivity(id) {
    await db.run('UPDATE net_peers SET last_seen_at = ?, last_error = NULL WHERE id = ?', [now(), id]);
  }

  async function disablePeer(id) {
    await db.run("UPDATE net_peers SET status = 'disabled' WHERE id = ?", [id]);
    await db.run('DELETE FROM net_outbox WHERE peer_id = ?', [id]);
  }

  async function incomingPeer(token) {
    if (!token) return null;
    return db.get("SELECT * FROM net_peers WHERE status = 'active' AND inbound_hash = ?", [hash(token)]);
  }

  async function receiveEvents(peer, eventsBatch) {
    let accepted = 0;
    let cursor = Number(peer.inbound_cursor) || 0;
    for (const event of eventsBatch) {
      const sequence = Number(event?.sequence);
      if (Number.isSafeInteger(sequence) && sequence >= 0) cursor = Math.max(cursor, sequence);
      let sourceMatches = false;
      try {
        const source = new URL(event?.instance ?? '');
        sourceMatches = source.protocol === 'https:' && source.origin === event.instance;
      } catch {
        sourceMatches = false;
      }
      if (
        !event ||
        typeof event.id !== 'string' ||
        !/^[a-f0-9]{64}$/.test(event.id) ||
        typeof event.type !== 'string' ||
        !['instance.announce', 'namespace.activated', 'namespace.suspended', 'name.activated', 'name.suspended'].includes(event.type) ||
        typeof event.subject !== 'string' ||
        !Number.isSafeInteger(sequence) ||
        sequence < 0 ||
        typeof event.createdAt !== 'string' ||
        !Number.isFinite(Date.parse(event.createdAt)) ||
        (event.actor !== null && typeof event.actor !== 'string') ||
        !event.data || typeof event.data !== 'object' || Array.isArray(event.data) ||
        !sourceMatches ||
        event.subject.length > 300
      )
        continue;
      const tld = event.type.startsWith('namespace.')
        ? (event.data.tld || event.subject.replace(/^\./, ''))
        : null;
      const name = event.type.startsWith('name.')
        ? (event.data.name || event.subject)
        : null;
      if ((tld && !safeTld(tld)) || (name && !safeName(name))) continue;
      if ((tld && event.subject.toLowerCase() !== `.${tld}`) ||
          (name && event.subject.toLowerCase() !== name.toLowerCase())) continue;
      if (event.type === 'instance.announce') {
        let announced;
        try { announced = new URL(event.data.instance).origin; } catch { continue; }
        if (announced !== event.instance || announced !== event.subject || typeof event.data.protocol !== 'string') continue;
      }
      const body = {
        type: event.type,
        subject: event.subject,
        data: event.data,
        actor: event.actor ?? null,
        instance: event.instance,
        createdAt: event.createdAt,
      };
      if (hash(json(body)) !== event.id) continue;
      const foreignId = `${event.instance}/${event.id}`;
      const imported = await db.transaction(async (tx) => {
        const existing = await tx.get(
          'SELECT event_id FROM net_events WHERE event_id = ?',
          [event.id],
        );
        await tx.run(
          'INSERT INTO net_foreign_records(source, kind, foreign_id, payload, imported_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT (source, kind, foreign_id) DO NOTHING',
          [event.instance, 'federation-event', foreignId, json(event), now()],
        );
        if (existing) return false;
        await tx.run(
          'INSERT INTO net_events(event_id, type, subject, payload, actor_id, instance, created_at, public_event) VALUES (?, ?, ?, ?, ?, ?, ?, 1) ON CONFLICT (event_id) DO NOTHING',
          [event.id, event.type, event.subject, json(event.data), event.actor ?? null, event.instance, event.createdAt],
        );
        const stored = await tx.get('SELECT seq FROM net_events WHERE event_id = ?', [event.id]);
        const peers = await tx.all("SELECT id FROM net_peers WHERE kind = 'w3bs' AND status = 'active' AND id <> ?", [peer.id]);
        for (const destination of peers)
          await tx.run(
            'INSERT INTO net_outbox(peer_id, event_id, next_attempt_at) VALUES (?, ?, ?) ON CONFLICT (peer_id, event_id) DO NOTHING',
            [destination.id, event.id, now()],
          );
        return Boolean(stored);
      });
      if (imported) accepted++;
    }
    return { accepted, cursor };
  }

  async function importRecord(source, kind, foreignId, payload) {
    await db.run(
      'INSERT INTO net_foreign_records(source, kind, foreign_id, payload, imported_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT (source, kind, foreign_id) DO UPDATE SET payload = excluded.payload, imported_at = excluded.imported_at',
      [source, kind, foreignId, json(payload), now()],
    );
  }

  async function importState(source, key, value) {
    await db.run(
      'INSERT INTO net_import_state(source, key, value, updated_at) VALUES (?, ?, ?, ?) ON CONFLICT (source, key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at',
      [source, key, String(value), now()],
    );
  }

  async function getImportState(source, key, fallback = '') {
    const row = await db.get('SELECT value FROM net_import_state WHERE source = ? AND key = ?', [source, key]);
    return row?.value ?? fallback;
  }

  async function backfillSummary() {
    const rows = await db.all(
      'SELECT source, kind, COUNT(*) AS count, MAX(imported_at) AS imported_at FROM net_foreign_records GROUP BY source, kind ORDER BY source, kind',
    );
    return rows.map((row) => ({ source: row.source, kind: row.kind, count: Number(row.count), importedAt: row.imported_at }));
  }

  async function queueOwnEvent(tx, options) {
    return announce(tx, options);
  }

  return {
    dns: createNetRecords(db),
    ensureDefaults,
    registry,
    resolveName,
    requestTld,
    requestName,
    pending,
    managed,
    moderate,
    events,
    peerList,
    knownInstances,
    addPeer,
    disablePeer,
    peersForWorker,
    dueOutbox,
    markDelivered,
    markDeliveryFailed,
    markPeerSeen,
    markPeerReceived,
    markPeerActivity,
    incomingPeer,
    receiveEvents,
    importRecord,
    importState,
    getImportState,
    backfillSummary,
    queueOwnEvent,
  };
}
