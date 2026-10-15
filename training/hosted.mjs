// W3BS Training on w3bs.org: one Training service per account, backed by the site's
// PostgreSQL. Each workspace is its own schema (tr_<hash of the user id>), so the same SQL as the
// local store runs unchanged and one workspace's queries cannot name another's tables.
//
// Artifacts are sealed blobs in the workspace schema, encrypted with a per-workspace key derived
// (HKDF-SHA256) from W3BS_TRAINING_MASTER_KEY, which lives in the vault, never in the database.
// The server can decrypt them: this is hosted processing, not end-to-end encrypted storage.
import { createHash, hkdfSync } from 'node:crypto';
import { W3bsError } from '../src/core.mjs';
import { openPostgresDb, sqlBlobs } from './core/db.mjs';
import { openTraining } from './core/service.mjs';

export const schemaFor = (userId) =>
  `tr_${createHash('sha256').update(`w3bs-training:${userId}`).digest('hex').slice(0, 32)}`;

export function createHostedTraining({ databaseUrl, masterKey, now, maxOpen = 32 } = {}) {
  if (!databaseUrl) throw new Error('W3BS Training hosting needs DATABASE_URL (PostgreSQL)');
  const master = masterKey
    ? Buffer.from(masterKey, /^[0-9a-f]{64}$/i.test(masterKey) ? 'hex' : 'utf8')
    : null;
  const open = new Map(); // userId -> Promise<{ service, pool, used }>
  let admin;
  const ssl = () => {
    const mode = new URL(databaseUrl).searchParams.get('sslmode');
    return mode && mode !== 'disable' ? { ssl: { rejectUnauthorized: false } } : {};
  };
  const pg = async () => (await import('pg')).default;

  async function evict() {
    if (open.size <= maxOpen) return;
    const oldest = [...open.entries()].sort((a, b) => a[1].used - b[1].used)[0];
    open.delete(oldest[0]);
    const entry = await oldest[1].ready.catch(() => null);
    await entry?.pool.end().catch(() => {});
  }

  async function workspace(userId) {
    if (!master || master.length < 32)
      throw new W3bsError(
        'TRAINING_UNAVAILABLE',
        'W3BS Training is not configured on this server (no master key).',
        503,
      );
    const schema = schemaFor(userId);
    const { Pool } = await pg();
    admin ??= new Pool({ connectionString: databaseUrl, max: 2, ...ssl() });
    await admin.query(`CREATE SCHEMA IF NOT EXISTS "${schema}"`);
    // Every connection of this pool sees only this workspace's schema.
    const pool = new Pool({
      connectionString: databaseUrl,
      max: 4,
      idleTimeoutMillis: 30000,
      options: `-c search_path="${schema}"`,
      ...ssl(),
    });
    const db = await openPostgresDb(pool);
    const key = Buffer.from(
      hkdfSync(
        'sha256',
        master,
        Buffer.from(schema),
        Buffer.from('w3bs-training artifacts v1'),
        32,
      ),
    );
    const service = await openTraining({
      db,
      blobs: sqlBlobs(db),
      keys: { mode: 'server-managed', key: () => key },
      config: async () =>
        JSON.parse((await db.get("SELECT value FROM meta WHERE key = 'config'"))?.value ?? '{}'),
      workspace: schema,
      backupsNote:
        'Removed from the live w3bs.org database now. Server database backups can still hold it until they expire; artifacts in them stay encrypted with your workspace key.',
      ...(now ? { now } : {}),
    });
    return { service, pool };
  }

  return {
    configured: !!master && master.length >= 32,
    // The Training service for one account's workspace (opened once, then cached).
    async forUser(userId) {
      let entry = open.get(userId);
      if (!entry) {
        entry = { ready: workspace(userId), used: Date.now() };
        open.set(userId, entry);
        entry.ready.catch(() => open.delete(userId));
        await evict();
      }
      entry.used = Date.now();
      return (await entry.ready).service;
    },
    async close() {
      for (const entry of open.values())
        await (await entry.ready.catch(() => null))?.pool.end().catch(() => {});
      open.clear();
      await admin?.end().catch(() => {});
    },
  };
}
