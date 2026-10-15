// Writes training/schema/json/*.json from format.mjs. `--check` exits 1 when
// the committed files differ from what format.mjs generates.
import { mkdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { jsonSchemas } from './format.mjs';

const directory = fileURLToPath(new URL('./json/', import.meta.url));
const check = process.argv.includes('--check');
const stale = [];
mkdirSync(directory, { recursive: true });
for (const [name, schema] of Object.entries(jsonSchemas())) {
  const path = resolve(directory, `${name}.json`);
  const body = `${JSON.stringify(schema, null, 2)}\n`;
  if (check) {
    if (!existsSync(path) || readFileSync(path, 'utf8') !== body) stale.push(name);
  } else writeFileSync(path, body);
}
if (stale.length) {
  process.stderr.write(
    `Stale JSON Schemas: ${stale.join(', ')}. Run bun training/schema/generate.mjs\n`,
  );
  process.exit(1);
}
process.stdout.write(
  check ? 'Training JSON Schemas are current.\n' : `Wrote JSON Schemas to ${directory}\n`,
);
