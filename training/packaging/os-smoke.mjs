// End-to-end check of an installed w3bs-training binary against the real OS key store.
// CI runs it on macOS and Windows runners after installing with install.sh / install.ps1.
//
//   bun training/packaging/os-smoke.mjs PATH/TO/w3bs-training EXPECTED_KEY_STORE
//
// EXPECTED_KEY_STORE is what `status` must report: macos-keychain, windows-dpapi, libsecret or
// passphrase.
// Checks: the key store is used (no passphrase), a staged import is committed by a second
// process (so the key really comes back out of the store), the recorder's stdin handoff works,
// and `setup claude` points at this OS's Claude config.
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const [bin, expected] = process.argv.slice(2);
if (!bin || !expected) {
  console.error('usage: os-smoke.mjs BIN EXPECTED_KEY_STORE');
  process.exit(2);
}
const fixture = resolve(import.meta.dir, '../fixtures/recorder/teach.w3bs-training.zip');
const env = { ...process.env };
// `passphrase` exercises the same flow on a box with no key store (local runs of this script).
if (expected !== 'passphrase') delete env.W3BS_TRAINING_PASSPHRASE;
delete env.W3BS_TRAINING_NO_KEYRING;

let failed = 0;
const check = (ok, what, detail) => {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}${ok || detail === undefined ? '' : `: ${detail}`}`);
  if (!ok) failed++;
};
const run = (home, args, input) => {
  const r = spawnSync(bin, args, {
    env: { ...env, W3BS_TRAINING_HOME: home },
    input,
    encoding: input ? undefined : 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  });
  const stdout = String(r.stdout ?? '');
  let json = null;
  try {
    json = JSON.parse(stdout);
  } catch {}
  return { status: r.status, json, stdout, stderr: String(r.stderr ?? ''), error: r.error };
};

const version = run(tmpdir(), ['version']);
check(
  version.status === 0 && /^\d+\.\d+\.\d+/.test(version.json?.version ?? ''),
  'version',
  version.stderr || version.error,
);

const home = mkdtempSync(join(tmpdir(), 'w3bs-training-'));
const status = run(home, ['status']);
check(
  status.json?.key_store === expected,
  `key store is ${expected}`,
  status.json?.key_store ?? status.stderr,
);

const staged = run(home, ['import', fixture, '--preview']);
const importId = staged.json?.import_id;
check(staged.status === 0 && !!importId, 'import --preview seals the stage', staged.stderr);
// A fresh process has no cached key: committing decrypts the stage with the key from the store.
const committed = run(home, ['commit', importId ?? 'none']);
check(
  committed.json?.status === 'committed',
  'commit in a second process reads the key back',
  committed.stderr,
);
const episodes = run(home, ['episodes', 'list']);
check(
  Array.isArray(episodes.json) && episodes.json.length === 1,
  'one episode imported',
  episodes.stdout || episodes.stderr,
);

// The desktop recorder pipes bundles to `import -`; frames never touch the disk unencrypted.
const home2 = mkdtempSync(join(tmpdir(), 'w3bs-training-'));
const piped = run(
  home2,
  ['import', '-', '--filename', 'teach.w3bs-training.zip'],
  readFileSync(fixture),
);
check(
  piped.status === 0 && piped.json?.status === 'committed',
  'recorder handoff over stdin',
  piped.stderr,
);

const claude = run(home, ['setup', 'claude', '--consumer', 'claude']);
const want =
  {
    darwin: /Library\/Application Support\/Claude\/claude_desktop_config\.json$/,
    win32: /%APPDATA%\\Claude\\claude_desktop_config\.json$/,
  }[process.platform] ?? /\.config\/Claude\/claude_desktop_config\.json$/;
check(
  want.test(claude.json?.file ?? ''),
  'setup claude names this OS config file',
  claude.json?.file ?? claude.stderr,
);
check(
  claude.json?.add?.mcpServers?.['w3bs-training']?.args?.at(-1) === 'claude',
  'setup claude config runs the MCP server',
  claude.stdout,
);

if (failed) {
  console.error(`${failed} check(s) failed`);
  process.exit(1);
}
console.log('all checks passed');
