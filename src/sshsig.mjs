// OpenSSH signatures (`ssh-keygen -Y sign`, PROTOCOL.sshsig) for W3BS-SIGN-1. Verifies
// ed25519 and nistp256 keys, plain and FIDO (sk-) variants; RSA and DSA are refused.
import { createHash, createPublicKey, verify } from 'node:crypto';
import { W3bsError } from './core.mjs';

export const KEY_TYPES = [
  'ssh-ed25519',
  'sk-ssh-ed25519@openssh.com',
  'ecdsa-sha2-nistp256',
  'sk-ecdsa-sha2-nistp256@openssh.com',
];
const fail = (message) => new W3bsError('BAD_SIGNATURE', message, 400);
const MAGIC = Buffer.from('SSHSIG');

function reader(buf) {
  let at = 0;
  const need = (n) => {
    if (at + n > buf.length) throw fail('Truncated SSH data.');
  };
  return {
    u8() {
      need(1);
      return buf[at++];
    },
    u32() {
      need(4);
      const value = buf.readUInt32BE(at);
      at += 4;
      return value;
    },
    bytes(n) {
      need(n);
      const value = buf.subarray(at, at + n);
      at += n;
      return value;
    },
    string() {
      return this.bytes(this.u32());
    },
    done: () => at === buf.length,
  };
}
const sshString = (value) => {
  const data = Buffer.isBuffer(value) ? value : Buffer.from(value);
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  return Buffer.concat([len, data]);
};

// The wire blob of a public key -> its type and a node KeyObject.
function decodeKey(blob) {
  const r = reader(blob);
  const type = r.string().toString();
  if (!KEY_TYPES.includes(type))
    throw fail(`Key type ${type} is not accepted; use ed25519 or ecdsa-sha2-nistp256.`);
  let key;
  let application;
  if (type.includes('ed25519')) {
    const raw = r.string();
    if (raw.length !== 32) throw fail('Bad ed25519 key.');
    key = createPublicKey({
      key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), raw]),
      format: 'der',
      type: 'spki',
    });
  } else {
    if (r.string().toString() !== 'nistp256') throw fail('Only the nistp256 curve is accepted.');
    const q = r.string();
    if (q.length !== 65 || q[0] !== 4) throw fail('Bad nistp256 key.');
    key = createPublicKey({
      key: {
        kty: 'EC',
        crv: 'P-256',
        x: q.subarray(1, 33).toString('base64url'),
        y: q.subarray(33).toString('base64url'),
      },
      format: 'jwk',
    });
  }
  if (type.startsWith('sk-')) application = r.string();
  if (!r.done()) throw fail('Trailing data in the public key.');
  return { type, key, application };
}

export const fingerprintOf = (blob) =>
  `SHA256:${createHash('sha256').update(blob).digest('base64').replace(/=+$/, '')}`;

// "ssh-ed25519 AAAA… comment" -> { type, blob, fingerprint, line }.
export function parsePublicKey(text) {
  const parts = String(text ?? '')
    .trim()
    .split(/\s+/);
  if (parts.length < 2) throw fail('Paste an OpenSSH public key (the .pub file).');
  const blob = Buffer.from(parts[1], 'base64');
  const { type } = decodeKey(blob);
  if (type !== parts[0]) throw fail('The key type does not match the key data.');
  return { type, blob, fingerprint: fingerprintOf(blob), line: `${type} ${parts[1]}` };
}

function parseArmor(armored) {
  const match = String(armored ?? '').match(
    /-----BEGIN SSH SIGNATURE-----([\s\S]+?)-----END SSH SIGNATURE-----/,
  );
  if (!match) throw fail('Expected an armored SSH signature (-----BEGIN SSH SIGNATURE-----).');
  const r = reader(Buffer.from(match[1].replace(/\s+/g, ''), 'base64'));
  if (!r.bytes(6).equals(MAGIC)) throw fail('Not an SSH signature.');
  if (r.u32() !== 1) throw fail('Unsupported SSH signature version.');
  const sig = {
    publicKey: Buffer.from(r.string()),
    namespace: r.string().toString(),
    reserved: Buffer.from(r.string()),
    hash: r.string().toString(),
    signature: Buffer.from(r.string()),
  };
  if (!r.done()) throw fail('Trailing data in the SSH signature.');
  return sig;
}

// The r||s pair of an ssh ecdsa signature, each padded to 32 bytes.
function ecdsaP1363(blob) {
  const r = reader(blob);
  const int = (value) => {
    const trimmed = value[0] === 0 ? value.subarray(1) : value;
    if (trimmed.length > 32) throw fail('Bad ecdsa signature.');
    return Buffer.concat([Buffer.alloc(32 - trimmed.length), trimmed]);
  };
  return Buffer.concat([int(r.string()), int(r.string())]);
}

// Verifies `armored` over `message` for `namespace`. Returns the signer's key details;
// throws BAD_SIGNATURE otherwise. Pass `publicKey` to require a specific key.
export function verifySshSig({ armored, message, namespace, publicKey }) {
  const sig = parseArmor(armored);
  if (sig.namespace !== namespace)
    throw fail(`Signed for namespace "${sig.namespace}", expected "${namespace}".`);
  if (!['sha256', 'sha512'].includes(sig.hash)) throw fail(`Unsupported hash ${sig.hash}.`);
  if (publicKey && !parsePublicKey(publicKey).blob.equals(sig.publicKey))
    throw fail('Signed by a different key.');
  const { type, key, application } = decodeKey(sig.publicKey);
  const signed = Buffer.concat([
    MAGIC,
    sshString(sig.namespace),
    sshString(sig.reserved),
    sshString(sig.hash),
    sshString(createHash(sig.hash).update(message).digest()),
  ]);
  const r = reader(sig.signature);
  if (r.string().toString() !== type) throw fail('Signature type does not match the key.');
  const raw = r.string();
  let data = signed;
  let flags;
  let counter;
  if (application) {
    flags = r.u8();
    counter = r.u32();
    const counterBytes = Buffer.alloc(4);
    counterBytes.writeUInt32BE(counter);
    const sha = (value) => createHash('sha256').update(value).digest();
    data = Buffer.concat([sha(application), Buffer.from([flags]), counterBytes, sha(signed)]);
  }
  if (!r.done()) throw fail('Trailing data in the signature.');
  const ok = type.includes('ed25519')
    ? verify(null, data, key, raw)
    : verify('sha256', data, { key, dsaEncoding: 'ieee-p1363' }, ecdsaP1363(raw));
  if (!ok) throw fail('The signature does not verify.');
  return {
    type,
    fingerprint: fingerprintOf(sig.publicKey),
    ...(application ? { flags, counter, application: application.toString() } : {}),
  };
}
