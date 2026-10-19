// W3BS-SIGN-1 (docs/W3BS-SIGN-1.md): publishers sign releases for w3bs.org/packages with
// an SSH key. An account registers a key for a domain namespace, proves the domain lists
// the key, and submits releases signed with `ssh-keygen -Y sign -n w3bs-release@w3bs.org`.
// Releases are immutable; revoking a key marks its releases revoked.
import { createHash, randomBytes } from 'node:crypto';
import { Resolver, lookup as dnsLookup } from 'node:dns/promises';
import { isIP } from 'node:net';
import canonicalize from 'canonicalize';
import { W3bsError } from './core.mjs';
import { parsePublicKey, verifySshSig } from './sshsig.mjs';

const tables = () => `
  CREATE TABLE IF NOT EXISTS publisher_challenges (challenge TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, expires_at TEXT NOT NULL);
  CREATE TABLE IF NOT EXISTS publisher_namespaces (namespace TEXT PRIMARY KEY, owner_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, created_at TEXT NOT NULL);
  CREATE TABLE IF NOT EXISTS publisher_keys (fingerprint TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, namespace TEXT NOT NULL, type TEXT NOT NULL, public_key TEXT NOT NULL, created_at TEXT NOT NULL, domain_verified_at TEXT, revoked_at TEXT);
  CREATE INDEX IF NOT EXISTS publisher_keys_namespace ON publisher_keys(namespace);
  CREATE TABLE IF NOT EXISTS package_releases (package_id TEXT NOT NULL, version TEXT NOT NULL, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, fingerprint TEXT NOT NULL, statement TEXT NOT NULL, signature TEXT NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY (package_id, version));`;
export const PUBLISHING_SCHEMA = { sqlite: tables(), postgres: tables() };

export const RELEASE_NAMESPACE = 'w3bs-release@w3bs.org';
export const KEY_NAMESPACE = 'w3bs-key@w3bs.org';
const CHALLENGE_TTL = 15 * 60 * 1000;
const INSTALLER_MAX = 2 * 1024 * 1024;
const FETCH_TIMEOUT = 10_000;

const fail = (code, message, status = 400) => new W3bsError(code, message, status);
const nowIso = () => new Date().toISOString();
const sha256hex = (data) => createHash('sha256').update(data).digest('hex');
const HOST = /^(?=.{4,253}$)([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z][a-z0-9-]{0,61}[a-z0-9]$/;
const NAME = /^[a-z0-9][a-z0-9._-]{0,63}$/;
const SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$/;
const INSTALL = /^curl -fsSL (https:\/\/\S+) \| (sh|bash)$/;

export const releaseBytes = (statement) => Buffer.from(canonicalize(statement));
export const proofLine = (fingerprint) => `w3bs-publisher=${fingerprint}`;

export function checkNamespace(value) {
  const namespace = String(value ?? '').toLowerCase();
  if (!HOST.test(namespace))
    throw fail('BAD_NAMESPACE', 'The namespace is a domain name you control, like example.com.');
  return namespace;
}

const text = (value, max) => typeof value === 'string' && value.length > 0 && value.length <= max;
const httpsUrl = (value) => {
  try {
    return text(value, 2048) && new URL(value).protocol === 'https:';
  } catch {
    return false;
  }
};

// The statement shape of W3BS-SIGN-1 §3; anything else is refused rather than stored.
export function checkStatement(statement) {
  const s = statement && typeof statement === 'object' ? statement : {};
  const bad = (message) => fail('BAD_STATEMENT', message);
  const allowed = new Set([
    'type',
    'package',
    'version',
    'name',
    'description',
    'homepage',
    'repository',
    'license',
    'commands',
    'install',
    'installer',
    'artifacts',
    'created_at',
  ]);
  for (const key of Object.keys(s)) if (!allowed.has(key)) throw bad(`Unknown field "${key}".`);
  if (s.type !== 'w3bs-release/1') throw bad('type must be "w3bs-release/1".');
  const [namespace, name, extra] = String(s.package ?? '').split('/');
  if (extra !== undefined || !name || !NAME.test(name))
    throw bad('package is <domain>/<name>, like example.com/tool.');
  checkNamespace(namespace);
  if (!SEMVER.test(s.version ?? '')) throw bad('version must be semver, like 1.2.3.');
  if (!text(s.name, 64)) throw bad('name is required (up to 64 characters).');
  if (!text(s.description, 500)) throw bad('description is required (up to 500 characters).');
  for (const key of ['homepage', 'repository'])
    if (s[key] !== undefined && !httpsUrl(s[key])) throw bad(`${key} must be an https URL.`);
  if (s.license !== undefined && !text(s.license, 64)) throw bad('license is a short SPDX id.');
  if (
    s.commands !== undefined &&
    !(Array.isArray(s.commands) && s.commands.length <= 32 && s.commands.every((c) => NAME.test(c)))
  )
    throw bad('commands is a list of command names.');
  const install = INSTALL.exec(s.install ?? '');
  if (!install) throw bad('install must be "curl -fsSL https://… | sh".');
  if (!s.installer || install[1] !== s.installer.url)
    throw bad('installer.url must be the URL in the install line.');
  if (!/^[0-9a-f]{64}$/.test(s.installer.sha256 ?? ''))
    throw bad('installer.sha256 is the hex sha256 of the installer.');
  if (Object.keys(s.installer).some((key) => !['url', 'sha256'].includes(key)))
    throw bad('installer has only url and sha256.');
  if (s.artifacts !== undefined) {
    if (!Array.isArray(s.artifacts) || s.artifacts.length > 32) throw bad('artifacts is a list.');
    for (const a of s.artifacts)
      if (
        !a ||
        !/^[a-z0-9_-]{1,32}$/.test(a.target ?? '') ||
        !httpsUrl(a.url) ||
        !/^[0-9a-f]{64}$/.test(a.sha256 ?? '') ||
        Object.keys(a).some((key) => !['target', 'url', 'sha256'].includes(key))
      )
        throw bad('Each artifact has target, an https url and a hex sha256.');
  }
  if (!text(s.created_at, 40) || Number.isNaN(Date.parse(s.created_at)))
    throw bad('created_at is an RFC 3339 time.');
  return { namespace, name };
}

// Loopback, private, link-local, CGNAT and other non-public addresses.
export function privateAddress(address) {
  if (isIP(address) === 6) {
    const a = address.toLowerCase();
    if (a.startsWith('::ffff:')) return privateAddress(a.slice(7));
    return a === '::1' || a === '::' || /^(fc|fd|fe8|fe9|fea|feb)/.test(a);
  }
  const [a, b] = address.split('.').map(Number);
  return (
    a === 0 ||
    a === 10 ||
    a === 127 ||
    a >= 224 ||
    (a === 100 && b >= 64 && b <= 127) ||
    (a === 169 && b === 254) ||
    (a === 172 && b >= 16 && b <= 31) ||
    (a === 192 && b === 168)
  );
}

// Domain proof reads TXT records from public resolvers with a new Resolver each time. The
// process-wide resolver keeps negative answers, so a record added after the first check (the
// order `w3bs publish key add` asks for) went unseen until the server restarted.
export async function freshResolveTxt(name) {
  const resolver = new Resolver({ timeout: 5000, tries: 2 });
  resolver.setServers(['1.1.1.1', '8.8.8.8', '9.9.9.9']);
  return resolver.resolveTxt(name);
}

export function createPublishing(
  db,
  { fetchImpl = fetch, resolveTxt = freshResolveTxt, lookup = dnsLookup } = {},
) {
  // An https GET of a public host, bounded in time and size; redirects are not followed.
  async function fetchPublic(url, max) {
    const { hostname, protocol } = new URL(url);
    if (protocol !== 'https:') throw fail('FETCH_FAILED', `${url} is not https.`);
    const addresses = await lookup(hostname, { all: true }).catch(() => []);
    if (!addresses.length || addresses.some(({ address }) => privateAddress(address)))
      throw fail('FETCH_FAILED', `${hostname} does not resolve to a public address.`);
    const res = await fetchImpl(url, {
      redirect: 'manual',
      signal: AbortSignal.timeout(FETCH_TIMEOUT),
    }).catch((error) => {
      throw fail('FETCH_FAILED', `Could not fetch ${url}: ${error.message}`);
    });
    if (res.status !== 200) throw fail('FETCH_FAILED', `${url} answered HTTP ${res.status}.`);
    const body = Buffer.from(await res.arrayBuffer());
    if (body.length > max) throw fail('FETCH_FAILED', `${url} is larger than ${max} bytes.`);
    return body;
  }

  // W3BS-SIGN-1 §2: the well-known file or the _w3bs TXT record lists the fingerprint.
  async function domainLists(namespace, fingerprint) {
    const line = proofLine(fingerprint);
    try {
      const body = await fetchPublic(`https://${namespace}/.well-known/w3bs-publisher.txt`, 65536);
      if (
        body
          .toString('utf8')
          .split(/\r?\n/)
          .some((l) => l.trim() === line)
      )
        return 'well-known';
    } catch {}
    try {
      const records = await resolveTxt(`_w3bs.${namespace}`);
      if (records.some((chunks) => chunks.join('').trim() === line)) return 'dns';
    } catch {}
    return null;
  }

  const proof = (namespace, fingerprint) => ({
    well_known: `https://${namespace}/.well-known/w3bs-publisher.txt`,
    dns: `_w3bs.${namespace}`,
    line: proofLine(fingerprint),
  });
  const keyView = (row) => ({
    fingerprint: row.fingerprint,
    namespace: row.namespace,
    type: row.type,
    public_key: row.public_key,
    domain_verified: Boolean(row.domain_verified_at),
    domain_verified_at: row.domain_verified_at ?? null,
    revoked_at: row.revoked_at ?? null,
    created_at: row.created_at,
    proof: proof(row.namespace, row.fingerprint),
  });
  const ownerOf = async (namespace) =>
    (await db.get('SELECT owner_id FROM publisher_namespaces WHERE namespace = ?', [namespace]))
      ?.owner_id;

  async function challenge(user) {
    const value = randomBytes(18).toString('base64url');
    await db.run('DELETE FROM publisher_challenges WHERE expires_at < ?', [nowIso()]);
    await db.run(
      'INSERT INTO publisher_challenges(challenge, user_id, expires_at) VALUES (?, ?, ?)',
      [value, user.id, new Date(Date.now() + CHALLENGE_TTL).toISOString()],
    );
    return {
      challenge: value,
      message: `w3bs-key:${user.username}:${value}`,
      namespace: KEY_NAMESPACE,
      expires_at: new Date(Date.now() + CHALLENGE_TTL).toISOString(),
    };
  }

  // Domain proof is checked now; when it passes and nobody owns the namespace yet,
  // this account becomes its owner.
  async function confirmDomain(userId, row) {
    const via = await domainLists(row.namespace, row.fingerprint);
    if (!via) return row;
    const owner = await ownerOf(row.namespace);
    if (owner && owner !== userId) return row;
    const at = nowIso();
    await db.transaction(async (tx) => {
      if (!owner)
        await tx.run(
          'INSERT INTO publisher_namespaces(namespace, owner_id, created_at) VALUES (?, ?, ?) ON CONFLICT (namespace) DO NOTHING',
          [row.namespace, userId, at],
        );
      await tx.run('UPDATE publisher_keys SET domain_verified_at = ? WHERE fingerprint = ?', [
        at,
        row.fingerprint,
      ]);
    });
    if ((await ownerOf(row.namespace)) !== userId) return row;
    return { ...row, domain_verified_at: at };
  }

  async function addKey(user, body = {}) {
    const namespace = checkNamespace(body.namespace);
    const key = parsePublicKey(body.public_key);
    const owner = await ownerOf(namespace);
    if (owner && owner !== user.id)
      throw fail('NAMESPACE_TAKEN', `${namespace} belongs to another W3BS account.`, 409);
    const pending = await db.get(
      'SELECT expires_at FROM publisher_challenges WHERE challenge = ? AND user_id = ?',
      [String(body.challenge ?? ''), user.id],
    );
    if (!pending || pending.expires_at < nowIso())
      throw fail('BAD_CHALLENGE', 'Get a fresh challenge from /api/publish/keys/challenge.');
    verifySshSig({
      armored: body.signature,
      message: Buffer.from(`w3bs-key:${user.username}:${body.challenge}`),
      namespace: KEY_NAMESPACE,
      publicKey: key.line,
    });
    await db.run('DELETE FROM publisher_challenges WHERE challenge = ?', [body.challenge]);
    const existing = await db.get('SELECT * FROM publisher_keys WHERE fingerprint = ?', [
      key.fingerprint,
    ]);
    if (existing && (existing.user_id !== user.id || existing.namespace !== namespace))
      throw fail('KEY_IN_USE', 'That key is already registered for another namespace.', 409);
    if (existing?.revoked_at)
      throw fail('KEY_REVOKED', 'That key was revoked; generate a new one.', 409);
    if (!existing)
      await db.run(
        'INSERT INTO publisher_keys(fingerprint, user_id, namespace, type, public_key, created_at) VALUES (?, ?, ?, ?, ?, ?)',
        [key.fingerprint, user.id, namespace, key.type, key.line, nowIso()],
      );
    const row = await db.get('SELECT * FROM publisher_keys WHERE fingerprint = ?', [
      key.fingerprint,
    ]);
    return keyView(await confirmDomain(user.id, row));
  }

  async function ownKey(userId, fingerprint) {
    const row = await db.get('SELECT * FROM publisher_keys WHERE fingerprint = ? AND user_id = ?', [
      String(fingerprint ?? ''),
      userId,
    ]);
    if (!row) throw fail('NOT_FOUND', 'No such key on your account.', 404);
    return row;
  }

  async function listKeys(userId) {
    const rows = await db.all(
      'SELECT * FROM publisher_keys WHERE user_id = ? ORDER BY created_at',
      [userId],
    );
    return { keys: rows.map(keyView) };
  }

  async function verifyKey(userId, fingerprint) {
    const row = await ownKey(userId, fingerprint);
    if (row.revoked_at) throw fail('KEY_REVOKED', 'That key is revoked.', 409);
    return keyView(await confirmDomain(userId, row));
  }

  async function revokeKey(userId, fingerprint) {
    const row = await ownKey(userId, fingerprint);
    if (!row.revoked_at)
      await db.run('UPDATE publisher_keys SET revoked_at = ? WHERE fingerprint = ?', [
        nowIso(),
        row.fingerprint,
      ]);
    return keyView(await ownKey(userId, fingerprint));
  }

  // W3BS-SIGN-1 §5, in order: signature, key and namespace, immutability, installer bytes.
  async function submitRelease(user, body = {}) {
    const statement = body.statement;
    const { namespace } = checkStatement(statement);
    const signer = verifySshSig({
      armored: body.signature,
      message: releaseBytes(statement),
      namespace: RELEASE_NAMESPACE,
    });
    const key = await db.get('SELECT * FROM publisher_keys WHERE fingerprint = ?', [
      signer.fingerprint,
    ]);
    if (!key || key.user_id !== user.id)
      throw fail('UNKNOWN_KEY', `${signer.fingerprint} is not registered to your account.`, 403);
    if (key.revoked_at) throw fail('KEY_REVOKED', 'That key is revoked.', 403);
    if (key.namespace !== namespace)
      throw fail('WRONG_NAMESPACE', `That key signs for ${key.namespace}, not ${namespace}.`, 403);
    if ((await ownerOf(namespace)) !== user.id)
      throw fail(
        'DOMAIN_UNVERIFIED',
        `Prove you control ${namespace} first (w3bs publish key verify).`,
        403,
      );
    if (!(await domainLists(namespace, key.fingerprint)))
      throw fail('DOMAIN_UNVERIFIED', `${namespace} no longer lists ${key.fingerprint}.`, 403);
    if (
      await db.get('SELECT 1 AS x FROM package_releases WHERE package_id = ? AND version = ?', [
        statement.package,
        statement.version,
      ])
    )
      throw fail(
        'VERSION_EXISTS',
        `${statement.package} ${statement.version} is already published; releases are immutable.`,
        409,
      );
    const installer = await fetchPublic(statement.installer.url, INSTALLER_MAX);
    if (sha256hex(installer) !== statement.installer.sha256)
      throw fail(
        'INSTALLER_MISMATCH',
        `The installer at ${statement.installer.url} does not match installer.sha256.`,
      );
    await db.run(
      'INSERT INTO package_releases(package_id, version, user_id, fingerprint, statement, signature, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)',
      [
        statement.package,
        statement.version,
        user.id,
        key.fingerprint,
        JSON.stringify(statement),
        String(body.signature).trim(),
        nowIso(),
      ],
    );
    return {
      package: statement.package,
      version: statement.version,
      fingerprint: key.fingerprint,
      path: `/packages/${statement.package}`,
    };
  }

  const releaseView = (row) => ({
    statement: JSON.parse(row.statement),
    signature: row.signature,
    public_key: row.public_key,
    fingerprint: row.fingerprint,
    revoked: Boolean(row.revoked_at),
    published_at: row.created_at,
  });
  const RELEASE_SQL =
    'SELECT r.*, k.public_key, k.revoked_at FROM package_releases r JOIN publisher_keys k ON k.fingerprint = r.fingerprint';
  const semverKey = (v) => v.split(/[-+]/)[0].split('.').map(Number);
  const newer = (a, b) => {
    const [x, y] = [semverKey(a), semverKey(b)];
    for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] > y[i];
    return !a.includes('-') && b.includes('-');
  };

  async function releases(packageId) {
    const rows = await db.all(`${RELEASE_SQL} WHERE r.package_id = ?`, [packageId]);
    return rows
      .map(releaseView)
      .sort((a, b) => (newer(a.statement.version, b.statement.version) ? -1 : 1));
  }

  // The newest unrevoked, non-prerelease signed release of each package, as catalog entries.
  async function catalogEntries() {
    const best = new Map();
    for (const row of await db.all(RELEASE_SQL)) {
      const release = releaseView(row);
      const { version } = release.statement;
      if (release.revoked || version.includes('-')) continue;
      const current = best.get(row.package_id);
      if (!current || newer(version, current.statement.version)) best.set(row.package_id, release);
    }
    return [...best.values()].map(({ statement, signature, public_key, fingerprint }) => {
      const namespace = statement.package.split('/')[0];
      return {
        id: statement.package,
        name: statement.name,
        commands: statement.commands ?? [statement.name],
        description: statement.description,
        ...(statement.homepage ? { homepage: statement.homepage } : {}),
        ...(statement.repository ? { repository: statement.repository } : {}),
        ...(statement.license ? { license: statement.license } : {}),
        agentic: false,
        latest_version: statement.version,
        vendor: namespace,
        install: statement.install,
        publisher: namespace,
        signed: { statement, signature, public_key, fingerprint },
      };
    });
  }

  return {
    challenge,
    addKey,
    listKeys,
    verifyKey,
    revokeKey,
    submitRelease,
    releases,
    catalogEntries,
  };
}
