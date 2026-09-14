#!/usr/bin/env node
import { generateKeyPairSync } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { request } from './client.mjs';
import { parseUri, signManifest, W3bsError } from './core.mjs';

const help = `W3BS 0.1.0 — community draft reference client

  w3bs search [query] [--type prompt]
  w3bs resolve|inspect|verify|conformance w3bs://prompt/w3bs/research@1
  w3bs run URI --inputs inputs.json --consent
  w3bs publish manifest.json
  w3bs revoke URI --reason 'Reason for withdrawal'
  w3bs keygen --out directory --namespace example
  w3bs sign manifest.json --key private.pem --out signed.json
  w3bs fork URI --namespace example --name new-name --key-id example-1 --out draft.json
  w3bs version URI --version 1.1.0 --out draft.json
  w3bs install URI --out pinned-resource.json

All output is JSON. W3BS_API selects the registry (default https://w3bs.org).
W3BS_PUBLISH_TOKEN authenticates writes; W3BS_TRUST_FILE selects pinned keys.
run renders instructions locally in the server's template runtime. No model or tool is called.
fork/version produce unsigned drafts; sign locally, then publish a new immutable version.
install saves a verified manifest; it does not execute an installer or register a protocol handler.
`;
try {
  const { values, positionals } = parseArgs({
    allowPositionals: true,
    options: {
      help: { type: 'boolean', short: 'h' },
      consent: { type: 'boolean' },
      json: { type: 'boolean' },
      type: { type: 'string' },
      inputs: { type: 'string' },
      key: { type: 'string' },
      out: { type: 'string' },
      namespace: { type: 'string' },
      name: { type: 'string' },
      'key-id': { type: 'string' },
      version: { type: 'string' },
      reason: { type: 'string' },
      api: { type: 'string' },
    },
  });
  const [command, target] = positionals;
  if (values.help || !command) {
    process.stdout.write(help);
    process.exit(0);
  }
  const readJson = (filename) => JSON.parse(readFileSync(filename, 'utf8'));
  const required = (name) => {
    if (!values[name]) throw new W3bsError('MISSING_OPTION', `--${name} is required.`);
    return values[name];
  };
  let result;
  if (command === 'keygen') {
    const namespace = required('namespace');
    parseUri(`w3bs://identity/${namespace}/publisher`);
    const directory = resolve(required('out'));
    mkdirSync(directory, { recursive: true, mode: 0o700 });
    const { privateKey, publicKey } = generateKeyPairSync('ed25519');
    writeFileSync(
      resolve(directory, 'private.pem'),
      privateKey.export({ type: 'pkcs8', format: 'pem' }),
      { mode: 0o600, flag: 'wx' },
    );
    const key = {
      id: `${namespace}-1`,
      namespace,
      publicKeyPem: publicKey.export({ type: 'spki', format: 'pem' }),
      revoked: false,
    };
    writeFileSync(resolve(directory, 'public.json'), JSON.stringify(key, null, 2) + '\n', {
      flag: 'wx',
    });
    result = {
      directory,
      publicKey: key,
      next: 'Ask the registry operator to bind this public key to your namespace. Never send the private key.',
    };
  } else if (command === 'sign') {
    result = signManifest(readJson(target), readFileSync(required('key'), 'utf8'));
    writeFileSync(required('out'), JSON.stringify(result, null, 2) + '\n', { flag: 'wx' });
  } else if (['fork', 'version', 'install'].includes(command)) {
    const resolved = await request('resolve', { uri: target }, { origin: values.api });
    if (!resolved.verification.valid)
      throw new W3bsError(
        'VERIFICATION_FAILED',
        'Cannot install or derive from a revoked or unverified resource.',
      );
    result = structuredClone(resolved.manifest);
    if (command !== 'install') {
      delete result.proof;
      result.provenance.derivedFrom = [resolved.canonicalUri];
      result.createdAt = new Date().toISOString();
      const namespace = command === 'fork' ? required('namespace') : result.publisher.namespace;
      const name = command === 'fork' ? required('name') : parseUri(result.id).name;
      result.version = command === 'fork' ? '1.0.0' : required('version');
      result.id = `w3bs://${result.type}/${namespace}/${name}@${result.version}`;
      parseUri(result.id);
      result.publisher.namespace = namespace;
      if (command === 'fork') {
        result.publisher.keyId = required('key-id');
        result.publisher.name = namespace;
      }
      result.locations.canonical = `https://prompt.w3bs.org/api/manifest?uri=${encodeURIComponent(result.id)}`;
      result.locations.mirrors = [];
    }
    writeFileSync(required('out'), JSON.stringify(result, null, 2) + '\n', { flag: 'wx' });
  } else {
    let args;
    if (command === 'search')
      args = {
        query: positionals.slice(1).join(' '),
        ...(values.type ? { type: values.type } : {}),
      };
    else if (command === 'publish') args = { manifest: readJson(target) };
    else if (command === 'run')
      args = {
        uri: target,
        inputs: values.inputs ? readJson(values.inputs) : {},
        consent: values.consent ?? false,
      };
    else if (command === 'revoke') args = { uri: target, reason: required('reason') };
    else args = { uri: target };
    result = await request(command, args, { origin: values.api });
  }
  process.stdout.write(JSON.stringify(result, null, 2) + '\n');
  if (command === 'verify' && !result.verification.valid) process.exitCode = 1;
} catch (error) {
  process.stderr.write(
    JSON.stringify({ error: { code: error.code || 'CLIENT_ERROR', message: error.message } }) +
      '\n',
  );
  process.exitCode = 1;
}
