import { DatabaseSync } from 'node:sqlite';
import { mkdirSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { W3bsError, parseUri, validateManifest, verifyManifest, compareVersions } from './core.mjs';

export const ROOT = fileURLToPath(new URL('../', import.meta.url));
export function openStore({
  path = resolve(process.env.W3BS_DATA_DIR || resolve(ROOT, '.local/data'), 'registry.sqlite'),
  trustStore,
  seed = true,
} = {}) {
  if (path !== ':memory:') mkdirSync(dirname(path), { recursive: true });
  const trust =
    trustStore ??
    JSON.parse(
      readFileSync(process.env.W3BS_TRUST_FILE || resolve(ROOT, 'fixtures/trust.json'), 'utf8'),
    );
  const db = new DatabaseSync(path);
  db.exec(`PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;
    CREATE TABLE IF NOT EXISTS resources (id TEXT PRIMARY KEY, base TEXT NOT NULL, version TEXT NOT NULL, manifest TEXT NOT NULL, published_at TEXT NOT NULL);
    CREATE INDEX IF NOT EXISTS resources_base ON resources(base);
    CREATE TABLE IF NOT EXISTS revocations (id TEXT PRIMARY KEY REFERENCES resources(id), reason TEXT NOT NULL, revoked_at TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS audit (sequence INTEGER PRIMARY KEY AUTOINCREMENT, operation TEXT NOT NULL, resource_id TEXT NOT NULL, at TEXT NOT NULL);`);
  function revoked(id) {
    return Boolean(db.prepare('SELECT id FROM revocations WHERE id = ?').get(id));
  }
  function inspect(uri) {
    const parsed = parseUri(uri);
    let candidates = db
      .prepare('SELECT manifest FROM resources WHERE base = ?')
      .all(parsed.base)
      .map((row) => JSON.parse(row.manifest));
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
      verification: verifyManifest(manifest, trust, revoked(manifest.id)),
    };
  }
  function publish(input) {
    const manifest = validateManifest(input);
    const verification = verifyManifest(manifest, trust);
    if (!verification.valid)
      throw new W3bsError(
        'UNTRUSTED_SIGNATURE',
        'Signature must verify against an operator-pinned key for this publisher namespace.',
        403,
      );
    const existing = db.prepare('SELECT manifest FROM resources WHERE id = ?').get(manifest.id);
    if (existing) {
      if (JSON.parse(existing.manifest).proof.digest === manifest.proof.digest)
        return inspect(manifest.id);
      throw new W3bsError(
        'IMMUTABLE_VERSION',
        'This version already exists. Publish a new version.',
        409,
      );
    }
    const now = new Date().toISOString();
    db.exec('BEGIN IMMEDIATE');
    try {
      db.prepare('INSERT INTO resources VALUES (?, ?, ?, ?, ?)').run(
        manifest.id,
        parseUri(manifest.id).base,
        manifest.version,
        JSON.stringify(manifest),
        now,
      );
      db.prepare('INSERT INTO audit(operation, resource_id, at) VALUES (?, ?, ?)').run(
        'publish',
        manifest.id,
        now,
      );
      db.exec('COMMIT');
    } catch (error) {
      db.exec('ROLLBACK');
      throw error;
    }
    return inspect(manifest.id);
  }
  function search(query = '', type = '') {
    if (typeof query !== 'string' || query.length > 200)
      throw new W3bsError('INVALID_QUERY', 'Search is limited to 200 characters.');
    const latest = new Map();
    for (const row of db.prepare('SELECT manifest FROM resources').all()) {
      const manifest = JSON.parse(row.manifest),
        base = parseUri(manifest.id).base;
      if (!latest.has(base) || compareVersions(manifest.version, latest.get(base).version) > 0)
        latest.set(base, manifest);
    }
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
        verification: verifyManifest(manifest, trust, revoked(manifest.id)),
      }));
  }
  function revoke(uri, reason) {
    if (!parseUri(uri).version?.includes('.'))
      throw new W3bsError('EXACT_VERSION_REQUIRED', 'Revocation requires a fully versioned URI.');
    const { canonicalUri } = inspect(uri);
    if (typeof reason !== 'string' || reason.trim().length < 3 || reason.length > 1000)
      throw new W3bsError('INVALID_REASON', 'Provide a revocation reason of 3–1000 characters.');
    const now = new Date().toISOString();
    db.exec('BEGIN IMMEDIATE');
    try {
      db.prepare('INSERT OR IGNORE INTO revocations VALUES (?, ?, ?)').run(
        canonicalUri,
        reason,
        now,
      );
      db.prepare('INSERT INTO audit(operation, resource_id, at) VALUES (?, ?, ?)').run(
        'revoke',
        canonicalUri,
        now,
      );
      db.exec('COMMIT');
    } catch (error) {
      db.exec('ROLLBACK');
      throw error;
    }
    return inspect(canonicalUri);
  }
  if (seed) {
    for (const filename of readdirSync(resolve(ROOT, 'fixtures/resources'))
      .filter((name) => name.endsWith('.json'))
      .sort()) {
      const manifest = JSON.parse(
        readFileSync(resolve(ROOT, 'fixtures/resources', filename), 'utf8'),
      );
      if (!db.prepare('SELECT id FROM resources WHERE id = ?').get(manifest.id)) publish(manifest);
    }
  }
  return { inspect, publish, search, revoke, trust, close: () => db.close() };
}
