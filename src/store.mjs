import { mkdirSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { W3bsError, parseUri, validateManifest, verifyManifest, compareVersions } from './core.mjs';

export const ROOT = fileURLToPath(new URL('../', import.meta.url));

// Registry storage. PostgreSQL is the production backend (DATABASE_URL); SQLite
// remains for local development and tests. Both expose the same async adapter:
// all/get/run take SQL with `?` placeholders, and transaction(fn) runs fn against
// a connection that holds one transaction open.
const SCHEMA = {
  sqlite: `PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;
    CREATE TABLE IF NOT EXISTS resources (id TEXT PRIMARY KEY, base TEXT NOT NULL, version TEXT NOT NULL, manifest TEXT NOT NULL, published_at TEXT NOT NULL);
    CREATE INDEX IF NOT EXISTS resources_base ON resources(base);
    CREATE TABLE IF NOT EXISTS revocations (id TEXT PRIMARY KEY REFERENCES resources(id), reason TEXT NOT NULL, revoked_at TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS audit (sequence INTEGER PRIMARY KEY AUTOINCREMENT, operation TEXT NOT NULL, resource_id TEXT NOT NULL, at TEXT NOT NULL);`,
  postgres: `CREATE TABLE IF NOT EXISTS resources (id TEXT PRIMARY KEY, base TEXT NOT NULL, version TEXT NOT NULL, manifest TEXT NOT NULL, published_at TEXT NOT NULL);
    CREATE INDEX IF NOT EXISTS resources_base ON resources(base);
    CREATE TABLE IF NOT EXISTS revocations (id TEXT PRIMARY KEY REFERENCES resources(id), reason TEXT NOT NULL, revoked_at TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS audit (sequence BIGSERIAL PRIMARY KEY, operation TEXT NOT NULL, resource_id TEXT NOT NULL, at TEXT NOT NULL);`,
};

async function openSqlite(path) {
  const { DatabaseSync } = await import('node:sqlite');
  if (path !== ':memory:') mkdirSync(dirname(path), { recursive: true });
  const db = new DatabaseSync(path);
  db.exec(SCHEMA.sqlite);
  const bind = (target) => ({
    all: async (sql, params = []) => target.prepare(sql).all(...params),
    get: async (sql, params = []) => target.prepare(sql).get(...params),
    run: async (sql, params = []) => {
      target.prepare(sql).run(...params);
    },
  });
  return {
    ...bind(db),
    dialect: 'sqlite',
    async transaction(fn) {
      db.exec('BEGIN IMMEDIATE');
      try {
        const result = await fn(bind(db));
        db.exec('COMMIT');
        return result;
      } catch (error) {
        db.exec('ROLLBACK');
        throw error;
      }
    },
    close: async () => db.close(),
  };
}

async function openPostgres(connectionString) {
  const { default: pg } = await import('pg');
  const sslmode = new URL(connectionString).searchParams.get('sslmode');
  const pool = new pg.Pool({
    connectionString,
    max: 8,
    // Railway's PostgreSQL presents a self-signed certificate on its public
    // proxy and runs internal traffic without TLS. Honour sslmode without
    // certificate verification; the internal URL carries no sslmode.
    ...(sslmode && sslmode !== 'disable' ? { ssl: { rejectUnauthorized: false } } : {}),
  });
  const positional = (sql) => {
    let index = 0;
    return sql.replaceAll('?', () => `$${++index}`);
  };
  const bind = (target) => ({
    all: async (sql, params = []) => (await target.query(positional(sql), params)).rows,
    get: async (sql, params = []) => (await target.query(positional(sql), params)).rows[0],
    run: async (sql, params = []) => {
      await target.query(positional(sql), params);
    },
  });
  await pool.query(SCHEMA.postgres);
  return {
    ...bind(pool),
    dialect: 'postgres',
    async transaction(fn) {
      const client = await pool.connect();
      try {
        await client.query('BEGIN');
        const result = await fn(bind(client));
        await client.query('COMMIT');
        return result;
      } catch (error) {
        await client.query('ROLLBACK').catch(() => {});
        throw error;
      } finally {
        client.release();
      }
    },
    close: () => pool.end(),
  };
}

export async function openStore({
  databaseUrl = process.env.DATABASE_URL,
  path = resolve(process.env.W3BS_DATA_DIR || resolve(ROOT, '.local/data'), 'registry.sqlite'),
  trustStore,
  seed = true,
} = {}) {
  const trust =
    trustStore ??
    JSON.parse(
      readFileSync(process.env.W3BS_TRUST_FILE || resolve(ROOT, 'fixtures/trust.json'), 'utf8'),
    );
  const db = databaseUrl ? await openPostgres(databaseUrl) : await openSqlite(path);
  async function revoked(id, tx = db) {
    return Boolean(await tx.get('SELECT id FROM revocations WHERE id = ?', [id]));
  }
  async function inspect(uri, tx = db) {
    const parsed = parseUri(uri);
    let candidates = (
      await tx.all('SELECT manifest FROM resources WHERE base = ?', [parsed.base])
    ).map((row) => JSON.parse(row.manifest));
    if (parsed.version)
      candidates = candidates.filter((item) =>
        parsed.version.includes('.')
          ? item.version === parsed.version
          : item.version.split('.')[0] === parsed.version,
      );
    // A revoked newest version remains visible. Never silently downgrade a caller to older code.
    candidates.sort((a, b) => compareVersions(b.version, a.version));
    const manifest = candidates[0];
    if (!manifest)
      throw new W3bsError('NOT_FOUND', 'No matching resource version in this registry.', 404);
    return {
      requestedUri: uri,
      canonicalUri: manifest.id,
      manifest,
      verification: verifyManifest(manifest, trust, await revoked(manifest.id, tx)),
    };
  }
  async function publish(input) {
    const manifest = validateManifest(input);
    const verification = verifyManifest(manifest, trust);
    if (!verification.valid)
      throw new W3bsError(
        'UNTRUSTED_SIGNATURE',
        'Signature must verify against an operator-pinned key for this publisher namespace.',
        403,
      );
    const now = new Date().toISOString();
    await db.transaction(async (tx) => {
      const existing = await tx.get('SELECT manifest FROM resources WHERE id = ?', [manifest.id]);
      if (existing) {
        if (JSON.parse(existing.manifest).proof.digest === manifest.proof.digest) return;
        throw new W3bsError(
          'IMMUTABLE_VERSION',
          'This version already exists. Publish a new version.',
          409,
        );
      }
      await tx.run('INSERT INTO resources VALUES (?, ?, ?, ?, ?)', [
        manifest.id,
        parseUri(manifest.id).base,
        manifest.version,
        JSON.stringify(manifest),
        now,
      ]);
      await tx.run('INSERT INTO audit(operation, resource_id, at) VALUES (?, ?, ?)', [
        'publish',
        manifest.id,
        now,
      ]);
    });
    return inspect(manifest.id);
  }
  async function search(query = '', type = '') {
    if (typeof query !== 'string' || query.length > 200)
      throw new W3bsError('INVALID_QUERY', 'Search is limited to 200 characters.');
    const latest = new Map();
    for (const row of await db.all('SELECT manifest FROM resources')) {
      const manifest = JSON.parse(row.manifest),
        base = parseUri(manifest.id).base;
      if (!latest.has(base) || compareVersions(manifest.version, latest.get(base).version) > 0)
        latest.set(base, manifest);
    }
    const revokedIds = new Set((await db.all('SELECT id FROM revocations')).map((row) => row.id));
    return [...latest.values()]
      .filter(
        (item) =>
          (!type || item.type === type) &&
          `${item.id} ${item.name} ${item.description}`.toLowerCase().includes(query.toLowerCase()),
      )
      .sort((a, b) => a.id.localeCompare(b.id))
      .slice(0, 100)
      .map((manifest) => ({
        id: manifest.id,
        name: manifest.name,
        description: manifest.description,
        type: manifest.type,
        version: manifest.version,
        publisher: manifest.publisher,
        permissions: manifest.permissions,
        verification: verifyManifest(manifest, trust, revokedIds.has(manifest.id)),
      }));
  }
  async function revoke(uri, reason) {
    if (!parseUri(uri).version?.includes('.'))
      throw new W3bsError('EXACT_VERSION_REQUIRED', 'Revocation requires a fully versioned URI.');
    const { canonicalUri } = await inspect(uri);
    if (typeof reason !== 'string' || reason.trim().length < 3 || reason.length > 1000)
      throw new W3bsError('INVALID_REASON', 'Provide a revocation reason of 3–1000 characters.');
    const now = new Date().toISOString();
    await db.transaction(async (tx) => {
      await tx.run('INSERT INTO revocations VALUES (?, ?, ?) ON CONFLICT (id) DO NOTHING', [
        canonicalUri,
        reason,
        now,
      ]);
      await tx.run('INSERT INTO audit(operation, resource_id, at) VALUES (?, ?, ?)', [
        'revoke',
        canonicalUri,
        now,
      ]);
    });
    return inspect(canonicalUri);
  }
  if (seed) {
    for (const filename of readdirSync(resolve(ROOT, 'fixtures/resources'))
      .filter((name) => name.endsWith('.json'))
      .sort()) {
      const manifest = JSON.parse(
        readFileSync(resolve(ROOT, 'fixtures/resources', filename), 'utf8'),
      );
      if (!(await db.get('SELECT id FROM resources WHERE id = ?', [manifest.id])))
        await publish(manifest);
    }
  }
  return {
    inspect,
    publish,
    search,
    revoke,
    trust,
    dialect: db.dialect,
    close: () => db.close(),
  };
}
