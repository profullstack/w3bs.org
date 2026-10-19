// Import Fabric's patterns (github.com/danielmiessler/Fabric, MIT) as signed prompt
// resources under the `fabric` namespace.
//
//   git clone --depth 1 https://github.com/danielmiessler/Fabric <dir>
//   bun scripts/import-fabric.mjs <dir>
//
// Each data/patterns/<name>/system.md becomes w3bs://prompt/fabric/<name>@1.0.0 with the
// piped text as `{{input}}`, the way Fabric appends it. Fabric's own template variables
// ({{lang_code}}, {{author_name}}) become declared inputs. Patterns whose text the template
// runtime cannot carry unchanged are skipped and listed, never rewritten.
import { execFileSync } from 'node:child_process';
import { generateKeyPairSync } from 'node:crypto';
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { signManifest } from '../src/core.mjs';

const KEY_ID = 'w3bs-fabric-1';
const NAMESPACE = 'fabric';
const VERSION = '1.0.0';
// Literal nuclei `{{var}}` syntax throughout; it is example output, not template input.
const SKIP = {
  write_nuclei_template_rule: 'its text is nuclei template syntax full of {{var}} tokens',
};
const VARIABLES = {
  lang_code: 'Language code to translate into, e.g. en-us or fr.',
  author_name: 'Author whose style the essay should follow.',
  query_language_info: 'Reference information about the query language.',
  guidelines: 'Guidelines for evaluating the queries.',
  user_input: 'The natural-language question that was asked.',
  generated_query: 'The query generated for that question.',
};

const source = process.argv[2];
if (!source || !existsSync(resolve(source, 'data/patterns')))
  throw new Error('usage: bun scripts/import-fabric.mjs <Fabric checkout>');
const commit = execFileSync('git', ['-C', source, 'rev-parse', 'HEAD'], {
  encoding: 'utf8',
}).trim();
const root = fileURLToPath(new URL('../', import.meta.url));
const out = resolve(root, 'fixtures/resources/fabric');

// The publisher key: generated once, private half kept in ignored .local/keys.
const keyDir = resolve(root, '.local/keys');
mkdirSync(keyDir, { recursive: true, mode: 0o700 });
const privatePath = resolve(keyDir, 'fabric-private.pem');
const trustPath = resolve(root, 'fixtures/trust.json');
const trust = JSON.parse(readFileSync(trustPath, 'utf8'));
let privateKey;
if (existsSync(privatePath)) {
  privateKey = readFileSync(privatePath, 'utf8');
} else {
  if (trust.keys.some((key) => key.id === KEY_ID))
    throw new Error(
      `${KEY_ID} is pinned but ${privatePath} is missing. Refusing to replace the publisher identity.`,
    );
  const keys = generateKeyPairSync('ed25519');
  privateKey = keys.privateKey.export({ type: 'pkcs8', format: 'pem' });
  writeFileSync(privatePath, privateKey, { mode: 0o600, flag: 'wx' });
  trust.keys.push({
    id: KEY_ID,
    namespace: NAMESPACE,
    publicKeyPem: keys.publicKey.export({ type: 'spki', format: 'pem' }),
    revoked: false,
  });
  writeFileSync(trustPath, JSON.stringify(trust, null, 2) + '\n');
}

const described = new Map(
  JSON.parse(
    readFileSync(resolve(source, 'scripts/pattern_descriptions/pattern_descriptions.json'), 'utf8'),
  ).patterns.map((item) => [item.patternName, item]),
);
const title = (name) => {
  const words = name.replace(/[_-]+/g, ' ');
  return words[0].toUpperCase() + words.slice(1);
};
// Fabric tags are upper case: WRITING -> Writing, CR THINKING -> CR thinking, AI stays AI.
const category = (tag) =>
  tag
    ? tag
        .split(' ')
        .map((word, i) =>
          word.length <= 2 ? word : i ? word.toLowerCase() : word[0] + word.slice(1).toLowerCase(),
        )
        .join(' ')
    : 'Fabric';

rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
const catalog = [];
const skipped = [];
const patterns = resolve(source, 'data/patterns');
for (const pattern of readdirSync(patterns).sort()) {
  const file = resolve(patterns, pattern, 'system.md');
  if (!existsSync(file)) continue;
  if (SKIP[pattern]) {
    skipped.push([pattern, SKIP[pattern]]);
    continue;
  }
  const system = readFileSync(file, 'utf8').trimEnd();
  const tokens = [...new Set([...system.matchAll(/\{\{([a-z][a-z0-9_]*)\}\}/g)].map((m) => m[1]))];
  const unknown = tokens.filter((token) => !VARIABLES[token]);
  if (unknown.length) {
    skipped.push([pattern, `undeclared template tokens: ${unknown.join(', ')}`]);
    continue;
  }
  const instructions = `${system}\n\n{{input}}`;
  if (instructions.length > 50000) {
    skipped.push([pattern, `${instructions.length} characters, over the 50,000 instruction limit`]);
    continue;
  }
  const slug = pattern.replace(/_/g, '-');
  const id = `w3bs://prompt/${NAMESPACE}/${slug}@${VERSION}`;
  const info = described.get(pattern);
  const inputs = {
    // A pattern with its own variables (judge_output) may not need piped text.
    input: { description: 'The text to run the pattern on.', required: tokens.length === 0 },
  };
  for (const token of tokens) inputs[token] = { description: VARIABLES[token], required: true };
  const manifest = signManifest(
    {
      specVersion: 'W3BS-MANIFEST-1-draft-1',
      id,
      type: 'prompt',
      version: VERSION,
      name: title(pattern),
      description: (info?.description || `Fabric's ${pattern} pattern.`).slice(0, 1000),
      publisher: {
        namespace: NAMESPACE,
        name: 'Fabric patterns by Daniel Miessler and contributors, imported by W3BS',
        keyId: KEY_ID,
      },
      license: 'MIT',
      createdAt: '2026-10-19T00:00:00.000Z',
      provenance: {
        source: `https://github.com/danielmiessler/Fabric/blob/${commit}/data/patterns/${pattern}/system.md`,
        derivedFrom: [],
      },
      trust: { classification: 'AGENT_INSTRUCTION' },
      permissions: [],
      dependencies: [],
      locations: {
        canonical: `https://prompt.w3bs.org/api/manifest?uri=${encodeURIComponent(id)}`,
        mirrors: [],
      },
      content: {
        mediaType: 'text/plain',
        instructions,
        inputs,
        output: {
          mediaType: 'text/plain',
          description:
            'Rendered instructions, ready for an explicitly authorized runtime or human review.',
        },
        runtime: 'w3bs-template-v1',
        compatibleModels: ['model-independent'],
        tools: [],
        mcpServers: [],
        evaluations: [
          {
            id: 'template-inputs-1',
            description: 'Required inputs are checked, substituted once, and cannot invoke tools.',
          },
        ],
      },
    },
    privateKey,
  );
  writeFileSync(resolve(out, `${slug}.json`), JSON.stringify(manifest, null, 2) + '\n');
  catalog.push({ id, category: category(info?.tags?.[0]) });
}
writeFileSync(resolve(out, 'catalog.json'), JSON.stringify(catalog, null, 2) + '\n');
writeFileSync(
  resolve(out, 'NOTICE'),
  `These prompts are Fabric's patterns (https://github.com/danielmiessler/Fabric, commit ${commit}),\n` +
    `converted to W3BS manifests by scripts/import-fabric.mjs and signed with ${KEY_ID}.\n` +
    `The signature attests the conversion, not Fabric's authorship. Fabric's license follows.\n\n` +
    readFileSync(resolve(source, 'LICENSE'), 'utf8'),
);
process.stdout.write(`Signed ${catalog.length} Fabric patterns from ${commit.slice(0, 12)}.\n`);
for (const [pattern, why] of skipped) process.stdout.write(`skipped ${pattern}: ${why}\n`);
