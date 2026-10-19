// Writes the W3BS OS draft suite snapshot (os/suite-manifest.json) from the catalog.
// The snapshot is frozen per OS release (docs/os-prd.md §5); `--check` reports drift.
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { buildSuite } from '../src/os-suite.mjs';

const ROOT = resolve(import.meta.dirname, '..');
const catalog = JSON.parse(readFileSync(resolve(ROOT, 'catalog/catalog.json'), 'utf8'));
const out = resolve(ROOT, 'os/suite-manifest.json');
const text = `${JSON.stringify(buildSuite(catalog), null, 2)}\n`;
if (process.argv.includes('--check')) {
  if (readFileSync(out, 'utf8') !== text) {
    console.error('os/suite-manifest.json is stale: run bun scripts/os-suite.mjs');
    process.exit(1);
  }
} else writeFileSync(out, text);
const s = JSON.parse(text).summary;
console.log(
  `${s.entries} entries, ${s.named} named, ${s.unpinned} unpinned, ${s.publisher_scripts} script-only, ${s.blocking_tasks} blocking tasks`,
);
