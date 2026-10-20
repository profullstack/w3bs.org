// W3BS DE (w3bs.org/de): the installer, and the files it installs with a checksummed manifest.
// The pack is de/share in this repository; the manifest is computed once at startup.
import { createHash } from 'node:crypto';
import { readFileSync, readdirSync, existsSync } from 'node:fs';
import { join, relative, resolve, sep } from 'node:path';
import { ROOT } from './store.mjs';
import { W3bsError } from './core.mjs';

const DE = resolve(ROOT, 'de');
const SHARE = resolve(DE, 'share');

const TYPES = {
  '.png': 'image/png',
  '.lua': 'text/plain; charset=utf-8',
  '.conf': 'text/plain; charset=utf-8',
  '.ini': 'text/plain; charset=utf-8',
  '.css': 'text/plain; charset=utf-8',
  '.json': 'application/json',
  '.jsonc': 'text/plain; charset=utf-8',
};

function walk(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = join(dir, e.name);
    return e.isDirectory() ? walk(p) : e.isFile() ? [p] : [];
  });
}

/** Every file of the pack with its SHA-256, as `sha256  path` lines under a version header. */
export function deManifest() {
  if (!existsSync(SHARE)) return null;
  const version = readFileSync(join(SHARE, 'VERSION'), 'utf8').trim();
  const files = walk(SHARE)
    .map((p) => relative(SHARE, p).split(sep).join('/'))
    .sort()
    .map((rel) => ({ rel, sum: createHash('sha256').update(readFileSync(join(SHARE, rel))).digest('hex') }));
  return { version, files, text: `# W3BS DE files\n# version ${version}\n${files.map((f) => `${f.sum}  ${f.rel}`).join('\n')}\n` };
}

export function mountDe(app) {
  const manifest = deManifest();
  const known = new Set(manifest?.files.map((f) => f.rel) ?? []);
  const cache = { 'Cache-Control': 'public, max-age=300' };
  app.get('/de/install.sh', (_req, res) => {
    const path = join(DE, 'install.sh');
    if (!existsSync(path)) throw new W3bsError('NOT_FOUND', 'Installer not published yet.', 404);
    res.type('text/x-shellscript; charset=utf-8').set(cache).send(readFileSync(path));
  });
  app.get('/de/manifest.txt', (_req, res) => {
    if (!manifest) throw new W3bsError('NOT_FOUND', 'W3BS DE is not published yet.', 404);
    res.type('text/plain; charset=utf-8').set(cache).send(manifest.text);
  });
  // Only paths listed in the manifest are served, so nothing outside de/share is reachable.
  app.get(/^\/de\/files\/(.+)$/, (req, res) => {
    const rel = req.params[0];
    if (!known.has(rel)) throw new W3bsError('NOT_FOUND', 'No such W3BS DE file.', 404);
    const ext = rel.slice(rel.lastIndexOf('.'));
    res.type(TYPES[ext] ?? 'text/plain; charset=utf-8').set(cache).send(readFileSync(join(SHARE, rel)));
  });
  app.get('/desktop-environment', (_req, res) => res.redirect(301, '/de'));
}

export function deVersion() {
  try {
    return readFileSync(join(SHARE, 'VERSION'), 'utf8').trim();
  } catch {
    return null;
  }
}
