#!/usr/bin/env node
// Experimental native URI client. Deliberately imports no shared resolver,
// manifest verifier, canonicalization library, CLI or API client implementation.
import { createHash, createPublicKey, verify } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { spawn } from 'node:child_process';
import { parseArgs } from 'node:util';
import { pathToFileURL } from 'node:url';

function canonical(value) {
  if (Array.isArray(value)) return '[' + value.map(canonical).join(',') + ']';
  if (value && typeof value === 'object')
    return (
      '{' +
      Object.keys(value)
        .sort()
        .map((key) => JSON.stringify(key) + ':' + canonical(value[key]))
        .join(',') +
      '}'
    );
  return JSON.stringify(value);
}
function parts(uri) {
  const match =
    typeof uri === 'string' &&
    uri.length <= 512 &&
    /^w3bs:\/\/(prompt|behavior|agent|tool|mcp|schema|eval|identity|service|device|stream|swarm)\/([a-z0-9]+(?:-[a-z0-9]+)*)\/([a-z0-9]+(?:-[a-z0-9]+)*)(?:@((?:0|[1-9][0-9]{0,8})(?:\.(?:0|[1-9][0-9]{0,8})\.(?:0|[1-9][0-9]{0,8}))?))?$/.exec(
      uri,
    );
  if (!match) throw new Error('Invalid W3BS URI.');
  return { type: match[1], namespace: match[2], name: match[3], version: match[4] };
}
export async function nativeResolve(
  uri,
  { origin = process.env.W3BS_API || 'https://w3bs.org', trustStore } = {},
) {
  const requested = parts(uri),
    api = new URL(origin);
  if (
    api.username ||
    api.password ||
    api.pathname !== '/' ||
    api.hash ||
    api.search ||
    (api.protocol !== 'https:' &&
      !(api.protocol === 'http:' && ['localhost', '127.0.0.1', '[::1]'].includes(api.hostname)))
  )
    throw new Error('Use a trusted HTTPS registry origin, or loopback for development.');
  const response = await fetch(new URL(`/api/resolve?uri=${encodeURIComponent(uri)}`, api), {
    redirect: 'error',
    signal: AbortSignal.timeout(15000),
  });
  if (!response.ok) throw new Error(`Resolution failed: HTTP ${response.status}`);
  const chunks = [];
  let size = 0;
  for await (const chunk of response.body) {
    size += chunk.length;
    if (size > 2_000_000) throw new Error('Oversized manifest response.');
    chunks.push(chunk);
  }
  const envelope = JSON.parse(Buffer.concat(chunks).toString('utf8'));
  const { manifest } = envelope,
    actual = parts(manifest.id);
  if (
    envelope.canonicalUri !== manifest.id ||
    envelope.requestedUri !== uri ||
    actual.type !== requested.type ||
    actual.namespace !== requested.namespace ||
    actual.name !== requested.name ||
    actual.version !== manifest.version ||
    !actual.version?.includes('.') ||
    actual.type !== manifest.type ||
    actual.namespace !== manifest.publisher.namespace ||
    (requested.version &&
      (requested.version.includes('.')
        ? actual.version !== requested.version
        : actual.version.split('.')[0] !== requested.version))
  )
    throw new Error('Canonical resource identity mismatch.');
  if (
    manifest.specVersion !== 'W3BS-MANIFEST-1-draft-1' ||
    manifest.proof?.type !== 'Ed25519' ||
    manifest.publisher.keyId !== manifest.proof.keyId
  )
    throw new Error('Unsupported manifest or signature profile.');
  const trust =
    trustStore ||
    JSON.parse(
      readFileSync(
        process.env.W3BS_TRUST_FILE || new URL('../fixtures/trust.json', import.meta.url),
        'utf8',
      ),
    );
  const key = trust.keys.find((entry) => entry.id === manifest.proof.keyId);
  const { proof, ...payload } = manifest;
  const bytes = Buffer.from(canonical(payload));
  const digest = `sha256:${createHash('sha256').update(bytes).digest('hex')}`;
  let signatureValid = false;
  try {
    const publicKey = key && createPublicKey(key.publicKeyPem);
    signatureValid = Boolean(
      publicKey &&
      publicKey.asymmetricKeyType === 'ed25519' &&
      /^[A-Za-z0-9_-]{86}$/.test(proof.signature) &&
      digest === proof.digest &&
      verify(null, bytes, publicKey, Buffer.from(proof.signature, 'base64url')),
    );
  } catch {
    /* Invalid signatures remain invalid. */
  }
  const publisherTrusted = Boolean(
    key && key.namespace === manifest.publisher.namespace && !key.revoked,
  );
  const revoked = Boolean(envelope.verification.revoked || key?.revoked);
  const verification = {
    valid: signatureValid && publisherTrusted && !revoked,
    signatureValid,
    publisherTrusted,
    revoked,
    digest,
    keyId: proof.keyId,
    authority: 'DATA',
  };
  if (canonical(verification) !== canonical(envelope.verification))
    throw new Error('Native verification disagrees with the registry.');
  return { ...envelope, verification };
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const { values, positionals } = parseArgs({
      allowPositionals: true,
      options: { json: { type: 'boolean' }, open: { type: 'boolean' }, help: { type: 'boolean' } },
    });
    if (values.help || !positionals[0]) {
      process.stdout.write(
        'w3bs-open w3bs://prompt/w3bs/research@1 [--json | --open]\nAn experimental native URI handler with independent manifest verification. --open uses the system browser for the verified inspection view.\n',
      );
    } else {
      const result = await nativeResolve(positionals[0]);
      process.stdout.write(JSON.stringify(result, null, 2) + '\n');
      if (!result.verification.valid) process.exitCode = 1;
      else if (values.open) {
        const url = new URL(
          `/browse?uri=${encodeURIComponent(result.canonicalUri)}`,
          process.env.W3BS_API || 'https://w3bs.org',
        );
        const command = process.platform === 'darwin' ? 'open' : 'xdg-open';
        if (process.platform === 'win32')
          throw new Error(
            'Automatic browser opening is currently supported on Linux and macOS. Use the HTTPS gateway on Windows.',
          );
        const child = spawn(command, [url.href], { stdio: 'ignore', detached: true });
        child.on('error', (error) => {
          process.stderr.write(error.message + '\n');
          process.exitCode = 1;
        });
        child.unref();
      }
    }
  } catch (error) {
    process.stderr.write(
      JSON.stringify({ error: { code: 'NATIVE_CLIENT_ERROR', message: error.message } }) + '\n',
    );
    process.exitCode = 1;
  }
}
