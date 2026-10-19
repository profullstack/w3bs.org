import { mkdirSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { W3bsError, parseUri, validateManifest, verifyManifest, compareVersions } from './core.mjs';
import { ACCOUNT_SCHEMA, createAccounts } from './accounts.mjs';
import { FLEET_SCHEMA, createFleet } from './fleet.mjs';
import { ABTEST_SCHEMA, createAbTest } from './abtest.mjs';
import { TASKS_SCHEMA, createTasks } from './tasks.mjs';
import { SOCIAL_SCHEMA, createSocial } from './social.mjs';
import { EVENT_SCHEMA, createEvents } from './events.mjs';
import { PUBLISHING_SCHEMA, createPublishing } from './publishing.mjs';

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
    CREATE TABLE IF NOT EXISTS audit (sequence INTEGER PRIMARY KEY AUTOINCREMENT, operation TEXT NOT NULL, resource_id TEXT NOT NULL, at TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS packages (id TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL, agentic INTEGER NOT NULL, publisher TEXT NOT NULL, entry TEXT NOT NULL, updated_at TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS package_scans (sequence INTEGER PRIMARY KEY AUTOINCREMENT, package_id TEXT NOT NULL, fingerprint TEXT NOT NULL, status TEXT NOT NULL, result TEXT NOT NULL, scanned_at TEXT NOT NULL);
    CREATE INDEX IF NOT EXISTS package_scans_package ON package_scans(package_id, sequence);`,
  postgres: `CREATE TABLE IF NOT EXISTS resources (id TEXT PRIMARY KEY, base TEXT NOT NULL, version TEXT NOT NULL, manifest TEXT NOT NULL, published_at TEXT NOT NULL);
    CREATE INDEX IF NOT EXISTS resources_base ON resources(base);
    CREATE TABLE IF NOT EXISTS revocations (id TEXT PRIMARY KEY REFERENCES resources(id), reason TEXT NOT NULL, revoked_at TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS audit (sequence BIGSERIAL PRIMARY KEY, operation TEXT NOT NULL, resource_id TEXT NOT NULL, at TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS packages (id TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL, agentic INTEGER NOT NULL, publisher TEXT NOT NULL, entry TEXT NOT NULL, updated_at TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS package_scans (sequence BIGSERIAL PRIMARY KEY, package_id TEXT NOT NULL, fingerprint TEXT NOT NULL, status TEXT NOT NULL, result TEXT NOT NULL, scanned_at TEXT NOT NULL);
    CREATE INDEX IF NOT EXISTS package_scans_package ON package_scans(package_id, sequence);`,
};

async function openSqlite(path) {
  const { DatabaseSync } = await import('node:sqlite');
  if (path !== ':memory:') mkdirSync(dirname(path), { recursive: true });
  const db = new DatabaseSync(path);
  db.exec(SCHEMA.sqlite);
  db.exec(ACCOUNT_SCHEMA.sqlite);
  db.exec(FLEET_SCHEMA.sqlite);
  db.exec(ABTEST_SCHEMA.sqlite);
  db.exec(TASKS_SCHEMA.sqlite);
  db.exec(SOCIAL_SCHEMA.sqlite);
  db.exec(EVENT_SCHEMA.sqlite);
  db.exec(PUBLISHING_SCHEMA.sqlite);
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
  await pool.query(ACCOUNT_SCHEMA.postgres);
  await pool.query(FLEET_SCHEMA.postgres);
  await pool.query(ABTEST_SCHEMA.postgres);
  await pool.query(TASKS_SCHEMA.postgres);
  await pool.query(SOCIAL_SCHEMA.postgres);
  await pool.query(EVENT_SCHEMA.postgres);
  await pool.query(PUBLISHING_SCHEMA.postgres);
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
  publishingOptions,
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
    return (
      [...latest.values()]
        .filter(
          (item) =>
            (!type || item.type === type) &&
            `${item.id} ${item.name} ${item.description}`
              .toLowerCase()
              .includes(query.toLowerCase()),
        )
        // The w3bs reference examples lead; imported libraries follow by id.
        .sort(
          (a, b) =>
            (b.publisher.namespace === 'w3bs') - (a.publisher.namespace === 'w3bs') ||
            a.id.localeCompare(b.id),
        )
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
        }))
    );
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
  // The package catalog: every tool listed here installs with its publisher's own
  // `curl | sh` installer (the listing rule), mirrored from catalog/catalog.json and
  // signed into the official W3BS repository at /repo.
  async function syncPackages() {
    const path = resolve(ROOT, 'catalog/catalog.json');
    let rows = [];
    try {
      rows = JSON.parse(readFileSync(path, 'utf8'));
    } catch {
      return 0;
    }
    const now = new Date().toISOString();
    await db.transaction(async (tx) => {
      const ids = rows.map((row) => row.id);
      for (const row of rows) {
        if (!/^curl\s.*https:\/\/\S+.*\|\s*(sh|bash)\s*$/.test(row.install || ''))
          throw new Error(`catalog entry ${row.id} has no curl | sh installer`);
        await tx.run(
          'INSERT INTO packages(id, name, kind, agentic, publisher, entry, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT (id) DO UPDATE SET name = excluded.name, kind = excluded.kind, agentic = excluded.agentic, publisher = excluded.publisher, entry = excluded.entry, updated_at = excluded.updated_at',
          [
            row.id,
            row.name,
            'curl',
            row.agentic ? 1 : 0,
            row.publisher || 'third-party',
            JSON.stringify(row),
            now,
          ],
        );
      }
      for (const { id } of await tx.all('SELECT id FROM packages'))
        if (!ids.includes(id)) await tx.run('DELETE FROM packages WHERE id = ?', [id]);
    });
    return rows.length;
  }
  // ThreatCrush results (src/scan.mjs). Each scan is a row; the newest one per package
  // is its current state, and the last few are kept as history.
  async function recordScan(packageId, result) {
    await db.transaction(async (tx) => {
      await tx.run(
        'INSERT INTO package_scans(package_id, fingerprint, status, result, scanned_at) VALUES (?, ?, ?, ?, ?)',
        [
          packageId,
          result.fingerprint || '',
          result.status,
          JSON.stringify(result),
          result.scannedAt,
        ],
      );
      await tx.run(
        'DELETE FROM package_scans WHERE package_id = ? AND sequence NOT IN (SELECT sequence FROM package_scans WHERE package_id = ? ORDER BY sequence DESC LIMIT 20)',
        [packageId, packageId],
      );
    });
  }
  async function latestScan(packageId) {
    const row = await db.get(
      'SELECT result FROM package_scans WHERE package_id = ? ORDER BY sequence DESC LIMIT 1',
      [packageId],
    );
    return row ? JSON.parse(row.result) : undefined;
  }
  async function latestScans() {
    const rows = await db.all(
      'SELECT package_id, result FROM package_scans WHERE sequence IN (SELECT MAX(sequence) FROM package_scans GROUP BY package_id)',
    );
    return new Map(rows.map((row) => [row.package_id, JSON.parse(row.result)]));
  }
  // The scan state a listing shows. A package changed since its last scan is
  // "pending" until the re-scan lands: an old pass must not vouch for new code.
  const scanState = (item, scan, detail = false) => {
    if (!scan || (scan.version ?? null) !== (item.latest_version ?? null))
      return { status: 'pending', scanner: 'threatcrush' };
    const { findings, fingerprint, ...summary } = scan;
    return detail ? { ...summary, findings: findings || [] } : summary;
  };
  // The curated catalog plus signed releases (src/publishing.mjs, W3BS-SIGN-1). A signed
  // release of a curated package takes over its version and installer; the curated
  // description, examples and evidence stay.
  const publishing = createPublishing(db, publishingOptions);
  async function allEntries() {
    const curated = (await db.all('SELECT entry FROM packages ORDER BY id')).map((row) =>
      JSON.parse(row.entry),
    );
    const signed = new Map((await publishing.catalogEntries()).map((entry) => [entry.id, entry]));
    const merged = curated.map((entry) => {
      const release = signed.get(entry.id);
      if (!release) {
        // Signed in the file, but no longer served (its key was revoked): not signed now.
        if (!entry.signed) return entry;
        const { signed: _withdrawn, ...rest } = entry;
        return rest;
      }
      signed.delete(entry.id);
      return {
        ...entry,
        latest_version: release.latest_version,
        install: release.install,
        signed: release.signed,
      };
    });
    return [...merged, ...signed.values()].sort((a, b) => a.id.localeCompare(b.id));
  }
  async function packages(query = '', { agentic, publisher } = {}) {
    if (typeof query !== 'string' || query.length > 200)
      throw new W3bsError('INVALID_QUERY', 'Search is limited to 200 characters.');
    const q = query.toLowerCase();
    const scans = await latestScans();
    return (await allEntries())
      .filter(
        (item) =>
          (agentic === undefined || Boolean(item.agentic) === agentic) &&
          (!publisher || item.publisher === publisher) &&
          `${item.id} ${item.name} ${item.vendor || ''} ${item.description} ${(item.commands || []).join(' ')}`
            .toLowerCase()
            .includes(q),
      )
      .map(({ evidence, ...item }) => ({
        ...item,
        w3bs: `w3bs install ${item.id}`,
        scan: scanState(item, scans.get(item.id)),
      }));
  }
  async function packageById(id) {
    const found = (await allEntries()).find((entry) => entry.id === id);
    if (!found) throw new W3bsError('NOT_FOUND', `No package ${id} in the catalog.`, 404);
    const { evidence, ...item } = found;
    return {
      ...item,
      w3bs: `w3bs install ${item.id}`,
      scan: scanState(item, await latestScan(id), true),
    };
  }
  await syncPackages();
  // Accounts and OAuth (src/accounts.mjs) share this database. Passkeys are bound to
  // the public origin, which the HTTP app knows, so it asks for them by origin.
  const events = createEvents(db);
  await createAccounts(db, { origin: 'http://localhost', events }).seedClients();
  const accounts = (origin) => createAccounts(db, { origin, events });
  if (seed) {
    // The reference examples, then imported libraries (fixtures/resources/fabric).
    for (const dir of ['fixtures/resources', 'fixtures/resources/fabric'])
      for (const filename of readdirSync(resolve(ROOT, dir))
        .filter((name) => name.endsWith('.json') && name !== 'catalog.json')
        .sort()) {
        const manifest = JSON.parse(readFileSync(resolve(ROOT, dir, filename), 'utf8'));
        if (!(await db.get('SELECT id FROM resources WHERE id = ?', [manifest.id])))
          await publish(manifest);
      }
  }
  return {
    inspect,
    publish,
    search,
    revoke,
    packages,
    package: packageById,
    syncPackages,
    recordScan,
    latestScan,
    accounts,
    fleet: createFleet(db, { events }),
    abtest: createAbTest(db),
    // Published OpenTasks and OpenWorkflows; their documents carry the site's origin.
    tasksFor: (origin) => createTasks(db, { origin }),
    events,
    social: createSocial(db),
    publishing,
    trust,
    dialect: db.dialect,
    close: () => db.close(),
  };
}
