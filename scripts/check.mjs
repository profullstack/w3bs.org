import { readdirSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { validateManifest, verifyManifest } from '../src/core.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
// Parse every script without running it. Bun has no `--check` flag (it ignores
// the flag and executes the file), so use its transpiler, which throws on a
// syntax error.
const transpiler = new Bun.Transpiler({ loader: 'js' });
for (const directory of ['src', 'native', 'scripts', 'test', 'public']) {
  for (const entry of readdirSync(resolve(root, directory))) {
    if (/\.(mjs|js)$/.test(entry)) {
      const path = resolve(root, directory, entry);
      try {
        transpiler.transformSync(readFileSync(path, 'utf8'));
      } catch (error) {
        throw new Error(`Syntax error in ${directory}/${entry}: ${error.message}`);
      }
    }
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
