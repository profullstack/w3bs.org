// Merge publisher-signed releases (W3BS-SIGN-1) from w3bs.org into catalog/catalog.json,
// so the registry signs them into the catalog `w3bs install` reads. Every signature is
// re-verified here: the registry does not take the web server's word for it.
//   bun scripts/catalog-signed.mjs [--source https://w3bs.org/api/publish/catalog.json]
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { RELEASE_NAMESPACE, checkStatement, releaseBytes } from '../src/publishing.mjs';
import { parsePublicKey, verifySshSig } from '../src/sshsig.mjs';

const ROOT = resolve(import.meta.dirname, '..');
const arg = (name, fallback) => {
  const i = process.argv.indexOf(name);
  return i === -1 ? fallback : process.argv[i + 1];
};

export function mergeSigned(catalog, signedEntries) {
  const byId = new Map(catalog.map((entry) => [entry.id, entry]));
  const live = new Set();
  const log = [];
  for (const entry of signedEntries) {
    try {
      const { statement, signature, public_key, fingerprint } = entry.signed ?? {};
      checkStatement(statement);
      if (statement.package !== entry.id || statement.install !== entry.install)
        throw new Error('entry does not match its statement');
      if (parsePublicKey(public_key).fingerprint !== fingerprint)
        throw new Error('fingerprint does not match the key');
      verifySshSig({
        armored: signature,
        message: releaseBytes(statement),
        namespace: RELEASE_NAMESPACE,
        publicKey: public_key,
      });
      const current = byId.get(entry.id);
      const merged = current
        ? {
            ...current,
            latest_version: statement.version,
            install: statement.install,
            signed: entry.signed,
          }
        : entry;
      if (JSON.stringify(merged) !== JSON.stringify(current))
        log.push(`${current ? 'signed' : 'added '} ${entry.id} ${statement.version}`);
      byId.set(entry.id, merged);
      live.add(entry.id);
    } catch (error) {
      log.push(`skip   ${entry.id}: ${error.message}`);
    }
  }
  // A signature that is no longer served (its key was revoked) leaves the catalog.
  for (const [id, entry] of byId) {
    if (!entry.signed || live.has(id)) continue;
    const { signed, ...rest } = entry;
    if (rest.publisher === entry.signed.statement.package.split('/')[0] && !rest.evidence) {
      byId.delete(id);
      log.push(`remove ${id}: signature withdrawn`);
    } else {
      byId.set(id, rest);
      log.push(`unsign ${id}: signature withdrawn`);
    }
  }
  return { catalog: [...byId.values()], log };
}

if (import.meta.main) {
  const source = arg('--source', 'https://w3bs.org/api/publish/catalog.json');
  const res = await fetch(source, { signal: AbortSignal.timeout(20_000) });
  if (!res.ok) throw new Error(`${source}: HTTP ${res.status}`);
  const path = resolve(ROOT, 'catalog/catalog.json');
  const { catalog, log } = mergeSigned(JSON.parse(readFileSync(path, 'utf8')), await res.json());
  writeFileSync(path, `${JSON.stringify(catalog, null, 2)}\n`);
  for (const line of log) console.log(line);
  console.log(`${log.length} change${log.length === 1 ? '' : 's'} from signed releases`);
}
