// Refresh `latest_version` in catalog/catalog.json from each entry's GitHub releases.
//
//   bun scripts/catalog-versions.mjs            update catalog/catalog.json in place
//   bun scripts/catalog-versions.mjs --check    print what would change; exit 1 if anything is stale
//
// `w3bs install` and `w3bs update` compare `<command> --version` against latest_version and
// skip the publisher's installer when it is already current, so a stale value hides real
// releases. Run by the registry refresh on dev2 before the catalog is signed. Needs `gh` auth.
//
// Monorepos ship several products from one repository, so a release counts for an entry only
// when its tag is a bare version (`v1.2.3`, `1.2.3`) or carries the entry's name
// (`diskpush-v1.2.3`, `diskpush@1.2.3`). Drafts and prereleases are ignored, and a value is
// only ever raised: a catalog ahead of GitHub (hand-set) is reported and left alone.
import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

const ROOT = resolve(import.meta.dirname, '..');
const FILE = resolve(ROOT, 'catalog/catalog.json');
const check = process.argv.includes('--check');

const SEMVER = /^(\d+)\.(\d+)\.(\d+)$/;
const parse = (v) => (v || '').replace(/^v/i, '').match(SEMVER)?.slice(1).map(Number) ?? null;
const cmp = (a, b) => a[0] - b[0] || a[1] - b[1] || a[2] - b[2];
const escape = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

function tagVersion(tag, names) {
  const prefix = names.map(escape).join('|');
  const m = tag.match(new RegExp(`^(?:(?:${prefix})[-_@/]?)?v?(\\d+\\.\\d+\\.\\d+)$`, 'i'));
  return m ? m[1] : null;
}

function releases(slug, tries = 3) {
  try {
    const out = execFileSync('gh', ['api', `repos/${slug}/releases?per_page=100`, '--jq', '.[] | select(.draft|not) | select(.prerelease|not) | .tag_name'], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    return out.split('\n').filter(Boolean);
  } catch (e) {
    const error = (e.stderr || e.message || '').toString().trim().split('\n')[0];
    // GitHub answers big repositories with an occasional 502/504; ask again.
    if (tries > 1 && /HTTP 5\d\d/.test(error)) return releases(slug, tries - 1);
    return { error };
  }
}

const catalog = JSON.parse(readFileSync(FILE, 'utf8'));
const cache = new Map();
let changed = 0;
for (const entry of catalog) {
  const slug = (entry.repository || '').match(/^https:\/\/github\.com\/([^/]+\/[^/#?]+?)(?:\.git)?\/?$/i)?.[1];
  if (!slug) continue;
  if (!cache.has(slug)) cache.set(slug, releases(slug));
  const tags = cache.get(slug);
  if (tags.error) {
    console.error(`skip   ${entry.id}: ${slug}: ${tags.error}`);
    continue;
  }
  const names = [...new Set([entry.name, ...(entry.commands || [])])];
  const best = tags
    .map((t) => tagVersion(t, names))
    .filter(Boolean)
    .sort((a, b) => cmp(parse(b), parse(a)))[0];
  if (!best) continue;
  const current = parse(entry.latest_version);
  if (current && cmp(parse(best), current) < 0) {
    console.error(`ahead  ${entry.id}: catalog ${entry.latest_version} > newest release ${best}; left alone`);
    continue;
  }
  if (current && cmp(parse(best), current) === 0) continue;
  console.log(`bump   ${entry.id}: ${entry.latest_version ?? '-'} -> ${best}`);
  entry.latest_version = best;
  changed++;
}

if (check) {
  console.log(`${changed} stale`);
  process.exit(changed ? 1 : 0);
}
if (changed) writeFileSync(FILE, JSON.stringify(catalog, null, 2) + '\n');
console.log(`${changed} updated`);
