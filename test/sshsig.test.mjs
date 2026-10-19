import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { parsePublicKey, verifySshSig } from '../src/sshsig.mjs';

// Real signatures from the installed OpenSSH, so the verifier is checked against the
// reference implementation rather than against itself.
export function sshKeypair(parent, type = 'ed25519') {
  const dir = mkdtempSync(join(parent, 'key-'));
  const path = join(dir, `id_${type}`);
  const args = ['-q', '-t', type, '-N', '', '-C', 'test', '-f', path];
  if (type === 'ecdsa') args.push('-b', '256');
  execFileSync('ssh-keygen', args);
  const sign = (message, namespace) => {
    const file = join(dir, `msg-${Math.random().toString(36).slice(2)}`);
    writeFileSync(file, message);
    execFileSync('ssh-keygen', ['-q', '-Y', 'sign', '-n', namespace, '-f', path, file], {
      stdio: 'ignore',
    });
    return readFileSync(`${file}.sig`, 'utf8');
  };
  return { publicKey: readFileSync(`${path}.pub`, 'utf8').trim(), pubPath: `${path}.pub`, sign };
}

const dir = mkdtempSync(join(tmpdir(), 'sshsig-'));
test.after(() => rmSync(dir, { recursive: true, force: true }));

for (const type of ['ed25519', 'ecdsa']) {
  test(`${type}: a real ssh-keygen signature verifies and binds key, namespace and message`, () => {
    const key = sshKeypair(dir, type);
    const other = sshKeypair(dir, type === 'ed25519' ? 'ecdsa' : 'ed25519');
    const message = Buffer.from('{"package":"a.dev/a","version":"1.0.0"}');
    const armored = key.sign(message, 'w3bs-release@w3bs.org');
    const parsed = parsePublicKey(key.publicKey);
    const fingerprint = execFileSync('ssh-keygen', ['-lf', key.pubPath]).toString().split(' ')[1];
    assert.equal(parsed.fingerprint, fingerprint);
    const ok = verifySshSig({
      armored,
      message,
      namespace: 'w3bs-release@w3bs.org',
      publicKey: key.publicKey,
    });
    assert.equal(ok.fingerprint, fingerprint);
    assert.throws(
      () => verifySshSig({ armored, message, namespace: 'git' }),
      /namespace "w3bs-release@w3bs.org", expected "git"/,
    );
    assert.throws(
      () =>
        verifySshSig({
          armored,
          message: Buffer.from('{"package":"a.dev/a","version":"1.0.1"}'),
          namespace: 'w3bs-release@w3bs.org',
        }),
      /does not verify/,
    );
    assert.throws(
      () =>
        verifySshSig({
          armored,
          message,
          namespace: 'w3bs-release@w3bs.org',
          publicKey: other.publicKey,
        }),
      /different key/,
    );
  });
}

test('RSA keys and garbage are refused', () => {
  const path = join(dir, 'id_rsa');
  execFileSync('ssh-keygen', ['-q', '-t', 'rsa', '-b', '3072', '-N', '', '-f', path]);
  assert.throws(
    () => parsePublicKey(readFileSync(`${path}.pub`, 'utf8')),
    /ssh-rsa is not accepted/,
  );
  assert.throws(() => parsePublicKey('hello'), /OpenSSH public key/);
  assert.throws(
    () => verifySshSig({ armored: 'nope', message: 'x', namespace: 'n' }),
    /armored SSH signature/,
  );
});
