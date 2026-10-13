// Scan catalog packages for malware with ThreatCrush (src/scan.mjs).
//
//   bun scripts/scan-packages.mjs --base base-catalog.json   new or changed entries (catalog PRs)
//   bun scripts/scan-packages.mjs moshcode.sh/moshcode         named packages
//   bun scripts/scan-packages.mjs --all                        the whole catalog
//
// --summary <file> appends a Markdown report (CI passes $GITHUB_STEP_SUMMARY).
// --record also stores each result where the site reads it (DATABASE_URL, else SQLite),
// so the package page updates without waiting for the background re-scan.
// Exits 1 when any package is flagged as malware or could not be scanned.
import { appendFileSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { ROOT } from '../src/store.mjs';
import { scanPackage } from '../src/scan.mjs';

const { values, positionals } = parseArgs({
  allowPositionals: true,
  options: {
    base: { type: 'string' },
    all: { type: 'boolean', default: false },
    record: { type: 'boolean', default: false },
    summary: { type: 'string' },
  },
});
const catalog = JSON.parse(readFileSync(resolve(ROOT, 'catalog/catalog.json'), 'utf8'));
let selected;
if (positionals.length) {
  selected = positionals.map((id) => {
    const entry = catalog.find((item) => item.id === id);
    if (!entry) throw new Error(`No package ${id} in catalog/catalog.json`);
    return entry;
  });
} else if (values.base) {
  let base = [];
  try {
    base = JSON.parse(readFileSync(values.base, 'utf8'));
  } catch {
    // No base catalog (first submission on a new branch layout): scan everything.
  }
  // Only what changes the code users get: copy or example edits need no new scan.
  const scanned = (item) => JSON.stringify([item.install, item.latest_version, item.repository]);
  const before = new Map(base.map((item) => [item.id, scanned(item)]));
  selected = catalog.filter((item) => before.get(item.id) !== scanned(item));
} else if (values.all) selected = catalog;
else {
  process.stderr.write('Name packages, or pass --base <catalog.json> or --all.\n');
  process.exit(2);
}

const store = values.record
  ? await (await import('../src/store.mjs')).openStore({ seed: false })
  : undefined;
const rows = [];
let failed = false;
for (const entry of selected) {
  let result;
  try {
    result = await scanPackage(entry);
  } catch (error) {
    result = {
      status: 'error',
      error: error.message,
      scannedAt: new Date().toISOString(),
      version: entry.latest_version ?? null,
      fingerprint: '',
    };
  }
  if (result.status !== 'passed') failed = true;
  await store?.recordScan(entry.id, result);
  const counts = result.summary
    ? Object.entries(result.summary)
        .filter(([, count]) => count)
        .map(([level, count]) => `${count} ${level}`)
        .join(', ') || 'none'
    : '';
  process.stdout.write(
    `${result.status.padEnd(7)} ${entry.id}@${entry.latest_version ?? '-'}  ${result.error || `${result.filesScanned} files; findings: ${counts}`}\n`,
  );
  for (const finding of (result.findings || []).filter((item) => item.malware))
    process.stdout.write(
      `        MALWARE ${finding.ruleId} ${finding.file}:${finding.line} ${finding.title}\n`,
    );
  for (const note of result.notes || []) process.stdout.write(`        note: ${note}\n`);
  rows.push({ entry, result, counts });
}
await store?.close();

if (values.summary) {
  const icon = { passed: '✅', flagged: '🚫', error: '⚠️' };
  const lines = [
    '## ThreatCrush package scan',
    '',
    rows.length
      ? '| | Package | Version | Files | Findings | Malware |'
      : 'No new or changed packages in this catalog change.',
    ...(rows.length ? ['|---|---|---|---|---|---|'] : []),
    ...rows.map(
      ({ entry, result, counts }) =>
        `| ${icon[result.status]} | \`${entry.id}\` | ${entry.latest_version ?? '-'} | ${result.filesScanned ?? '-'} | ${result.error ? `scan failed: ${result.error}` : counts} | ${result.malware ?? '-'} |`,
    ),
    '',
    ...rows.flatMap(({ entry, result }) =>
      (result.findings || [])
        .filter((item) => item.malware)
        .map(
          (item) =>
            `- 🚫 \`${entry.id}\` ${item.ruleId} at \`${item.file}:${item.line}\`: ${item.title}`,
        ),
    ),
    '',
  ];
  appendFileSync(values.summary, lines.join('\n'));
}
process.exit(failed ? 1 : 0);
