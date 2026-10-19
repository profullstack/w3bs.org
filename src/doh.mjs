// A small RFC 8484 (DNS over HTTPS) client for Moshpit names. It only ever asks the Moshpit
// resolver: Moshpit names (chovy.hacker, pay.agent) are not in the ICANN root, so the system
// resolver cannot answer them. The resolver also forwards ICANN names, so one client serves both.
import { W3bsError } from './core.mjs';

export const DOH_ENDPOINT = 'https://dns.moshcode.sh/dns-query';
// The only hosts this client will ever send a query to.
export const DOH_ALLOWLIST = Object.freeze(['dns.moshcode.sh']);

export const TYPES = Object.freeze({ A: 1, CNAME: 5, TXT: 16, AAAA: 28 });
const TYPE_NAMES = Object.fromEntries(Object.entries(TYPES).map(([name, code]) => [code, name]));
const RCODES = ['NOERROR', 'FORMERR', 'SERVFAIL', 'NXDOMAIN', 'NOTIMP', 'REFUSED'];

const fail = (message) => new W3bsError('DNS_FAILED', message, 502);

export function validDomainName(name) {
  return (
    typeof name === 'string' &&
    name.length <= 253 &&
    /^(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(name)
  );
}

// The wire-format query (RFC 1035 §4.1): ID 0 as RFC 8484 §4.1 recommends, recursion desired.
export function encodeQuery(name, type) {
  if (!validDomainName(name))
    throw new W3bsError('INVALID_INPUT', `Not a domain name: ${name}`, 400);
  const code = TYPES[type];
  if (!code) throw new W3bsError('INVALID_INPUT', `Unsupported record type: ${type}`, 400);
  const labels = name.split('.').map((label) => Buffer.from(label, 'ascii'));
  const question = Buffer.concat([
    ...labels.flatMap((label) => [Buffer.from([label.length]), label]),
    Buffer.from([0, code >> 8, code & 0xff, 0, 1]),
  ]);
  return Buffer.concat([Buffer.from([0, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0]), question]);
}

function readName(buf, start) {
  const labels = [];
  let offset = start,
    end = -1,
    jumps = 0;
  for (;;) {
    if (offset >= buf.length) throw fail('Truncated DNS name.');
    const len = buf[offset];
    if ((len & 0xc0) === 0xc0) {
      if (offset + 1 >= buf.length || ++jumps > 16) throw fail('Bad DNS name pointer.');
      if (end < 0) end = offset + 2;
      offset = ((len & 0x3f) << 8) | buf[offset + 1];
      continue;
    }
    if (len > 63) throw fail('Bad DNS label.');
    if (len === 0) {
      if (end < 0) end = offset + 1;
      break;
    }
    if (offset + 1 + len > buf.length) throw fail('Truncated DNS label.');
    labels.push(buf.toString('latin1', offset + 1, offset + 1 + len));
    offset += 1 + len;
  }
  return { name: labels.join('.').toLowerCase(), end };
}

function ipv6(bytes) {
  const groups = [];
  for (let i = 0; i < 16; i += 2) groups.push(((bytes[i] << 8) | bytes[i + 1]).toString(16));
  // Collapse the longest run of zero groups (RFC 5952).
  let best = [-1, 0];
  for (let i = 0; i < 8;) {
    if (groups[i] !== '0') {
      i++;
      continue;
    }
    let j = i;
    while (j < 8 && groups[j] === '0') j++;
    if (j - i > best[1] && j - i > 1) best = [i, j - i];
    i = j;
  }
  if (best[0] < 0) return groups.join(':');
  return `${groups.slice(0, best[0]).join(':')}::${groups.slice(best[0] + best[1]).join(':')}`;
}

// Parse a wire-format response into { rcode, answers: [{ name, type, ttl, data }] }.
export function parseResponse(input) {
  const buf = Buffer.from(input);
  if (buf.length < 12) throw fail('DNS response is too short.');
  const flags = buf.readUInt16BE(2);
  if (!(flags & 0x8000)) throw fail('Not a DNS response.');
  const rcode = RCODES[flags & 0xf] || `RCODE${flags & 0xf}`;
  const qd = buf.readUInt16BE(4),
    an = buf.readUInt16BE(6);
  let offset = 12;
  for (let i = 0; i < qd; i++) offset = readName(buf, offset).end + 4;
  const answers = [];
  for (let i = 0; i < an; i++) {
    const { name, end } = readName(buf, offset);
    if (end + 10 > buf.length) throw fail('Truncated DNS record.');
    const type = buf.readUInt16BE(end),
      ttl = buf.readUInt32BE(end + 4),
      length = buf.readUInt16BE(end + 8),
      rdata = end + 10;
    if (rdata + length > buf.length) throw fail('Truncated DNS record data.');
    offset = rdata + length;
    const typeName = TYPE_NAMES[type];
    let data;
    if (typeName === 'A' && length === 4) data = [...buf.subarray(rdata, rdata + 4)].join('.');
    else if (typeName === 'AAAA' && length === 16) data = ipv6(buf.subarray(rdata, rdata + 16));
    else if (typeName === 'CNAME') data = readName(buf, rdata).name;
    else if (typeName === 'TXT') {
      const parts = [];
      for (let at = rdata; at < rdata + length;) {
        const size = buf[at];
        if (at + 1 + size > rdata + length) throw fail('Truncated TXT string.');
        parts.push(buf.toString('utf8', at + 1, at + 1 + size));
        at += 1 + size;
      }
      data = parts.join('');
    } else continue;
    answers.push({ name, type: typeName, ttl, data });
  }
  return { rcode, answers };
}

export function createDohClient({
  fetchImpl = fetch,
  endpoint = DOH_ENDPOINT,
  timeoutMs = 3000,
  ttlMs = 5 * 60 * 1000,
  maxEntries = 500,
  now = Date.now,
} = {}) {
  const url = new URL(endpoint);
  if (url.protocol !== 'https:' || !DOH_ALLOWLIST.includes(url.hostname))
    throw new Error(`DoH endpoint ${url.hostname} is not on the allowlist.`);
  const cache = new Map();
  async function query(name, type) {
    const key = `${type} ${name}`;
    const hit = cache.get(key);
    if (hit && now() - hit.at < ttlMs) return hit.value;
    const target = new URL(url);
    target.searchParams.set('dns', encodeQuery(name, type).toString('base64url'));
    if (!DOH_ALLOWLIST.includes(target.hostname)) throw fail('DoH host is not allowed.');
    const res = await fetchImpl(target, {
      headers: { accept: 'application/dns-message' },
      redirect: 'error',
      signal: AbortSignal.timeout(timeoutMs),
    }).catch((error) => {
      throw fail(`DNS over HTTPS failed: ${error.message}`);
    });
    if (res.status !== 200) throw fail(`DNS over HTTPS answered HTTP ${res.status}.`);
    const body = Buffer.from(await res.arrayBuffer());
    if (body.length > 65535) throw fail('DNS response is too large.');
    const value = parseResponse(body);
    if (cache.size >= maxEntries) cache.delete(cache.keys().next().value);
    cache.set(key, { at: now(), value });
    return value;
  }
  return { query, endpoint: url.href };
}
