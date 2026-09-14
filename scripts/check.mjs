import { readdirSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { validateManifest, verifyManifest } from '../src/core.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
for (const directory of ['src', 'native', 'scripts', 'test', 'public']) {
  for (const entry of readdirSync(resolve(root, directory))) {
    if (/\.(mjs|js)$/.test(entry))
      execFileSync(process.execPath, ['--check', resolve(root, directory, entry)], {
        stdio: 'pipe',
      });
  }
}
const trust = JSON.parse(readFileSync(resolve(root, 'fixtures/trust.json'), 'utf8'));
const resources = readdirSync(resolve(root, 'fixtures/resources')).filter((name) =>
  name.endsWith('.json'),
);
if (resources.length < 10) throw new Error('At least ten example resources required.');
for (const file of resources) {
  const manifest = JSON.parse(readFileSync(resolve(root, 'fixtures/resources', file), 'utf8'));
  validateManifest(manifest);
  if (!verifyManifest(manifest, trust).valid) throw new Error(`Invalid fixture ${file}`);
}
process.stdout.write(`Syntax valid. ${resources.length} signed reference resources verify.\n`);
