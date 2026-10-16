// Artifact encryption: AES-256-GCM with a per-workspace 32-byte key.
//
// Key sources, in order:
//   1. W3BS_TRAINING_PASSPHRASE (scrypt, salt kept beside the store)
//   2. the OS key store: libsecret `secret-tool` on Linux, `security` on macOS,
//      DPAPI (through PowerShell) on Windows
// When neither is available, artifact writes fail with KEY_UNAVAILABLE. There
// is no plaintext fallback.
import { createCipheriv, createDecipheriv, randomBytes, scryptSync } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { W3bsError } from '../../src/core.mjs';

const MAGIC = Buffer.from('W3TA1'); // W3BS Training Artifact, format 1

export function encrypt(key, plaintext, aad) {
  const iv = randomBytes(12);
  const cipher = createCipheriv('aes-256-gcm', key, iv);
  if (aad) cipher.setAAD(Buffer.from(aad));
  const body = Buffer.concat([cipher.update(plaintext), cipher.final()]);
  return Buffer.concat([MAGIC, iv, cipher.getAuthTag(), body]);
}

export function decrypt(key, sealed, aad) {
  if (!sealed.subarray(0, MAGIC.length).equals(MAGIC))
    throw new W3bsError(
      'ARTIFACT_CORRUPT',
      'Artifact is not a sealed W3BS training artifact.',
      500,
    );
  const iv = sealed.subarray(5, 17);
  const tag = sealed.subarray(17, 33);
  const decipher = createDecipheriv('aes-256-gcm', key, iv);
  if (aad) decipher.setAAD(Buffer.from(aad));
  decipher.setAuthTag(tag);
  try {
    return Buffer.concat([decipher.update(sealed.subarray(33)), decipher.final()]);
  } catch {
    throw new W3bsError('ARTIFACT_CORRUPT', 'Artifact failed authentication.', 500);
  }
}

function osKeyStore(account, directory) {
  const service = 'w3bs-training';
  if (process.platform === 'linux') {
    // Started over ssh or by a service, there is no DBUS_SESSION_BUS_ADDRESS; the logged-in
    // desktop's keyring is still on the user's standard session bus socket.
    const env = { ...process.env };
    const bus = `/run/user/${process.getuid?.()}/bus`;
    if (!env.DBUS_SESSION_BUS_ADDRESS && existsSync(bus))
      env.DBUS_SESSION_BUS_ADDRESS = `unix:path=${bus}`;
    const which = spawnSync('secret-tool', ['--version'], { encoding: 'utf8', env });
    if (which.error) return null;
    return {
      name: 'libsecret',
      get() {
        const result = spawnSync(
          'secret-tool',
          ['lookup', 'service', service, 'account', account],
          {
            encoding: 'utf8',
            env,
          },
        );
        return result.status === 0 && result.stdout.trim()
          ? Buffer.from(result.stdout.trim(), 'base64')
          : null;
      },
      set(key) {
        const result = spawnSync(
          'secret-tool',
          ['store', '--label', `W3BS Training ${account}`, 'service', service, 'account', account],
          { input: key.toString('base64'), encoding: 'utf8', env },
        );
        return result.status === 0;
      },
    };
  }
  if (process.platform === 'darwin') {
    return {
      name: 'macos-keychain',
      get() {
        const result = spawnSync(
          'security',
          ['find-generic-password', '-s', service, '-a', account, '-w'],
          {
            encoding: 'utf8',
          },
        );
        return result.status === 0 ? Buffer.from(result.stdout.trim(), 'base64') : null;
      },
      set(key) {
        const result = spawnSync(
          'security',
          [
            'add-generic-password',
            '-U',
            '-s',
            service,
            '-a',
            account,
            '-w',
            key.toString('base64'),
          ],
          { encoding: 'utf8' },
        );
        return result.status === 0;
      },
    };
  }
  if (process.platform === 'win32' && directory) {
    // DPAPI: the key file is only decryptable by this Windows user on this machine.
    const file = resolve(directory, 'key.dpapi');
    const ps = (script, input) =>
      spawnSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', script], {
        input,
        encoding: 'utf8',
        windowsHide: true,
      });
    const prelude =
      'Add-Type -AssemblyName System.Security; $b = [Convert]::FromBase64String([Console]::In.ReadToEnd().Trim());';
    return {
      name: 'windows-dpapi',
      get() {
        if (!existsSync(file)) return null;
        const r = ps(
          `${prelude} [Convert]::ToBase64String([Security.Cryptography.ProtectedData]::Unprotect($b, $null, 'CurrentUser'))`,
          readFileSync(file, 'utf8'),
        );
        return r.status === 0 && r.stdout.trim() ? Buffer.from(r.stdout.trim(), 'base64') : null;
      },
      set(key) {
        const r = ps(
          `${prelude} [Convert]::ToBase64String([Security.Cryptography.ProtectedData]::Protect($b, $null, 'CurrentUser'))`,
          key.toString('base64'),
        );
        if (r.status !== 0 || !r.stdout.trim()) return false;
        writeFileSync(file, r.stdout.trim(), { mode: 0o600 });
        return true;
      },
    };
  }
  return null;
}

// Returns { mode, key() } where key() throws KEY_UNAVAILABLE when no protected
// key can be obtained. `workspace` names the key in the OS store.
export function keyProvider(directory, workspace, env = process.env) {
  const passphrase = env.W3BS_TRAINING_PASSPHRASE;
  let cached = null;
  if (passphrase) {
    return {
      mode: 'passphrase',
      key() {
        if (cached) return cached;
        const saltPath = resolve(directory, 'key-salt.json');
        let salt;
        if (existsSync(saltPath))
          salt = Buffer.from(JSON.parse(readFileSync(saltPath, 'utf8')).salt, 'base64');
        else {
          salt = randomBytes(16);
          writeFileSync(
            saltPath,
            JSON.stringify({ kdf: 'scrypt', salt: salt.toString('base64') }),
            { mode: 0o600 },
          );
        }
        cached = scryptSync(passphrase, salt, 32, {
          N: 2 ** 15,
          r: 8,
          p: 1,
          maxmem: 64 * 1024 * 1024,
        });
        return cached;
      },
    };
  }
  const store = env.W3BS_TRAINING_NO_KEYRING ? null : osKeyStore(workspace, directory);
  if (store) {
    return {
      mode: store.name,
      key() {
        if (cached) return cached;
        let key = store.get();
        if (!key || key.length !== 32) {
          key = randomBytes(32);
          if (!store.set(key) || !store.get()?.equals(key))
            throw new W3bsError(
              'KEY_UNAVAILABLE',
              `The ${store.name} key store did not accept a key. Unlock it, or set W3BS_TRAINING_PASSPHRASE.`,
              409,
            );
        }
        cached = key;
        return key;
      },
    };
  }
  return {
    mode: 'unavailable',
    key() {
      throw new W3bsError(
        'KEY_UNAVAILABLE',
        'No protected key store is available. Set W3BS_TRAINING_PASSPHRASE for passphrase-locked mode; artifacts are never stored in plaintext.',
        409,
      );
    },
  };
}
