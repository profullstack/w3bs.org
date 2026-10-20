// /browse and /api/browse: one input box for W3BS URIs, Moshpit names (chovy.hacker), pit.world
// gateway URLs (https://chovy.hacker.pit.world/path) and ordinary https URLs.
//
// Moshpit names resolve "internally": over DNS over HTTPS to the Moshpit resolver (src/doh.mjs),
// never the system resolver. The page then checks the name's ICANN gateway
// (https://<name>.pit.world) and reads the site's W3BS discovery document through it.
//
// SSRF: every fetch is https, to the pit.world gateway host or the public host the person typed,
// after checking that host resolves only to public addresses; redirects are followed by hand, at
// most 3, and never to another host; bodies are capped and the whole check is timed out.
import { lookup as dnsLookup } from 'node:dns/promises';
import { isIP } from 'node:net';
import { W3bsError } from './core.mjs';
import { privateAddress } from './publishing.mjs';
import { createDohClient, validDomainName } from './doh.mjs';

export const GATEWAY = 'pit.world';
const FETCH_TIMEOUT = 5000;
const MAX_REDIRECTS = 3;
const PAGE_LIMIT = 512 * 1024;
const DISCOVERY_LIMIT = 64 * 1024;
const USER_AGENT = 'w3bs-browse/0.1 (+https://w3bs.org/browse)';

const invalid = (message) => new W3bsError('INVALID_INPUT', message, 400);

export const gatewayHost = (name) => `${name}.${GATEWAY}`;

// What the person typed, classified without touching the network.
//   { kind: 'w3bs', uri }
//   { kind: 'moshpit', name, path, via }       a pit.world gateway URL, so certainly Moshpit
//   { kind: 'web', host, url, path, candidate } candidate: label.ending, which may be a Moshpit
//                                               name (decided by its v=moshpit1 TXT record)
export function normalizeInput(raw) {
  const input = String(raw ?? '').trim();
  if (!input) return null;
  if (input.length > 2048) throw invalid('Input is longer than 2048 characters.');
  let text = input;
  // w3bs://type/namespace/name is a W3BS resource. A dotted authority is not a type but a name to
  // browse: w3bs://chovy.hacker is chovy.hacker, w3bs://example.com/path is that https page.
  const w3bs = /^w3bs:\/\/([^/?#]*)/i.exec(input);
  if (w3bs) {
    if (!w3bs[1].includes('.')) return { kind: 'w3bs', input, uri: input };
    text = `https://${input.slice('w3bs://'.length)}`;
  }
  const scheme = /^([a-z][a-z0-9+.-]*):\/\//i.exec(text);
  if (scheme) {
    const name = scheme[1].toLowerCase();
    if (name === 'http') text = `https://${text.slice(scheme[0].length)}`;
    else if (name !== 'https') throw invalid(`Only https URLs can be browsed, not ${name}:.`);
  } else text = `https://${text}`;
  let url;
  try {
    url = new URL(text);
  } catch {
    throw invalid('That is not a W3BS URI, Moshpit name or URL.');
  }
  if (url.username || url.password) throw invalid('URLs with credentials are not browsed.');
  if (url.port && url.port !== '443') throw invalid('Only the default https port is browsed.');
  const host = url.hostname.toLowerCase().replace(/\.$/, '');
  if (isIP(host.replace(/^\[|\]$/g, ''))) throw invalid('Browse a name, not an IP address.');
  if (!validDomainName(host)) throw invalid(`${host} is not a domain name.`);
  const path = `${url.pathname}${url.search}`;
  if (host.endsWith(`.${GATEWAY}`)) {
    const labels = host.slice(0, -GATEWAY.length - 1).split('.');
    if (labels.length === 2)
      return { kind: 'moshpit', input, name: labels.join('.'), path, via: 'gateway' };
  }
  const candidate = host !== GATEWAY && host.split('.').length === 2;
  return { kind: 'web', input, host, url: `https://${host}${path}`, path, candidate };
}

// "v=moshpit1 name=chovy.hacker resolved=chovy.hacker target=dev.profullstack.com"
export function parseMoshpitTxt(records) {
  for (const record of records) {
    const fields = record.trim().split(/\s+/);
    if (fields[0] !== 'v=moshpit1') continue;
    return Object.fromEntries(
      fields
        .slice(1)
        .map((field) => field.split('='))
        .filter(([key, value]) => key && value !== undefined)
        .map(([key, ...rest]) => [key, rest.join('=')]),
    );
  }
  return null;
}

const decodeEntities = (text) =>
  text
    .replace(/&amp;/g, '&')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"')
    .replace(/&#39;|&#x27;/g, "'")
    .replace(/&middot;/g, '·');

export function pageTitle(html) {
  const match = /<title[^>]*>([^<]{1,300})<\/title>/i.exec(html);
  return match ? decodeEntities(match[1]).replace(/\s+/g, ' ').trim() : null;
}

export function createBrowser({
  doh = createDohClient(),
  fetchImpl = fetch,
  lookup = (host) => dnsLookup(host, { all: true }),
  cacheMs = 60 * 1000,
  maxPerMinute = 60,
  now = Date.now,
} = {}) {
  const cache = new Map();
  let windowStart = 0,
    used = 0;

  // An https GET of a public host, bounded in time, redirects and size.
  async function safeFetch(start, { allowHosts, maxBytes, accept }) {
    const signal = AbortSignal.timeout(FETCH_TIMEOUT);
    const redirects = [];
    let url = new URL(start);
    for (;;) {
      if (url.protocol !== 'https:') throw new Error(`refused ${url.protocol} (https only)`);
      if (!allowHosts.includes(url.hostname))
        throw new Error(`refused a redirect to ${url.hostname}`);
      const addresses = await lookup(url.hostname).catch(() => []);
      if (!addresses.length) throw new Error(`${url.hostname} does not resolve`);
      if (addresses.some(({ address }) => privateAddress(address)))
        throw new Error(`refused ${url.hostname}: it resolves to a private address`);
      const res = await fetchImpl(url, {
        redirect: 'manual',
        signal,
        headers: { accept, 'user-agent': USER_AGENT },
      });
      const location = res.headers.get('location');
      if (res.status >= 300 && res.status < 400 && location) {
        await res.body?.cancel?.().catch(() => {});
        if (redirects.length >= MAX_REDIRECTS) throw new Error('too many redirects');
        url = new URL(location, url);
        redirects.push(url.href);
        continue;
      }
      const chunks = [];
      let size = 0,
        truncated = false;
      if (res.body?.getReader) {
        const reader = res.body.getReader();
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          size += value.length;
          if (size > maxBytes) {
            truncated = true;
            chunks.push(value.subarray(0, value.length - (size - maxBytes)));
            await reader.cancel().catch(() => {});
            break;
          }
          chunks.push(value);
        }
      } else {
        const whole = Buffer.from(await res.arrayBuffer());
        truncated = whole.length > maxBytes;
        chunks.push(whole.subarray(0, maxBytes));
      }
      return {
        status: res.status,
        url: url.href,
        redirects,
        contentType: res.headers.get('content-type') || '',
        body: Buffer.concat(chunks),
        truncated,
      };
    }
  }

  async function checkSite(base, path, allowHosts) {
    const start = `${base}${path || '/'}`;
    try {
      const res = await safeFetch(start, {
        allowHosts,
        maxBytes: PAGE_LIMIT,
        accept: 'text/html,application/xhtml+xml;q=0.9,*/*;q=0.5',
      });
      return {
        url: start,
        ok: res.status >= 200 && res.status < 300,
        status: res.status,
        finalUrl: res.url,
        redirects: res.redirects,
        contentType: res.contentType,
        title: /html/i.test(res.contentType) ? pageTitle(res.body.toString('utf8')) : null,
      };
    } catch (error) {
      return { url: start, ok: false, status: null, error: error.message };
    }
  }

  async function discovery(base, allowHosts) {
    const url = `${base}/.well-known/w3bs.json`;
    try {
      const res = await safeFetch(url, {
        allowHosts,
        maxBytes: DISCOVERY_LIMIT,
        accept: 'application/json',
      });
      if (res.status !== 200) return { url, found: false, status: res.status };
      if (res.truncated) return { url, found: false, error: 'larger than 64 KB' };
      let document;
      try {
        document = JSON.parse(res.body.toString('utf8'));
      } catch {
        return { url, found: false, status: 200, error: 'not JSON' };
      }
      if (!document || typeof document !== 'object' || Array.isArray(document))
        return { url, found: false, status: 200, error: 'not a JSON object' };
      return { url, found: true, status: 200, document };
    } catch (error) {
      return { url, found: false, error: error.message };
    }
  }

  async function records(name) {
    const out = { resolver: doh.endpoint, A: [], AAAA: [], CNAME: [], TXT: [] };
    const results = await Promise.allSettled(['A', 'AAAA', 'TXT'].map((t) => doh.query(name, t)));
    const errors = [];
    results.forEach((result, index) => {
      if (result.status === 'rejected') return errors.push(result.reason.message);
      out.rcode ??= result.value.rcode;
      for (const answer of result.value.answers) {
        if (answer.type === 'CNAME' && answer.name === name) {
          if (!out.CNAME.includes(answer.data)) out.CNAME.push(answer.data);
        } else if (
          answer.type === ['A', 'AAAA', 'TXT'][index] &&
          !out[answer.type].includes(answer.data)
        )
          out[answer.type].push(answer.data);
      }
    });
    if (errors.length) out.errors = errors;
    return out;
  }

  async function lookupFresh(normal) {
    if (normal.kind === 'moshpit' || normal.candidate) {
      const name = normal.kind === 'moshpit' ? normal.name : normal.host;
      const dns = await records(name);
      const moshpit = parseMoshpitTxt(dns.TXT);
      if (normal.kind === 'moshpit' || moshpit) {
        const gateway = gatewayHost(name);
        const base = `https://${gateway}`;
        const [site, found] = await Promise.all([
          checkSite(base, normal.path, [gateway]),
          discovery(base, [gateway]),
        ]);
        return {
          input: normal.input,
          kind: 'moshpit',
          name,
          // Moshpit-aware clients open the name itself; everyone else the ICANN gateway.
          primary: `https://${name}${normal.path || '/'}`,
          fallback: `${base}${normal.path || '/'}`,
          moshpit: moshpit && {
            // The ending is on Moshpit; minted says whether this label has been claimed.
            ending: name.split('.').at(-1),
            minted: Boolean(moshpit.target),
            target: moshpit.target || null,
            resolved: moshpit.resolved || name,
            record: moshpit,
          },
          dns,
          gateway: site,
          discovery: found,
        };
      }
      return web(normal, dns);
    }
    return web(normal, await records(normal.host));
  }

  async function web(normal, dns) {
    const base = `https://${normal.host}`;
    const allow = [
      normal.host,
      normal.host.startsWith('www.') ? normal.host.slice(4) : `www.${normal.host}`,
    ];
    const [site, found] = await Promise.all([
      checkSite(base, normal.path, allow),
      discovery(base, allow),
    ]);
    return {
      input: normal.input,
      kind: 'web',
      name: normal.host,
      primary: normal.url,
      fallback: null,
      moshpit: null,
      dns,
      gateway: site,
      discovery: found,
    };
  }

  // resolve('chovy.hacker', { inspect }) -> the JSON /api/browse answers. W3BS URIs go to the
  // store's resolver (inspect), as /browse always has.
  async function resolve(raw, { inspect } = {}) {
    const normal = normalizeInput(raw);
    if (!normal) throw invalid('Enter a W3BS URI, a Moshpit name or a URL.');
    if (normal.kind === 'w3bs') {
      if (!inspect) return { input: normal.input, kind: 'w3bs', uri: normal.uri };
      return {
        input: normal.input,
        kind: 'w3bs',
        uri: normal.uri,
        resource: await inspect(normal.uri),
      };
    }
    // chovy.hacker and https://chovy.hacker.pit.world/ are the same name, so one cache entry.
    const key = `${normal.kind === 'moshpit' ? normal.name : normal.host} ${normal.path}`;
    const hit = cache.get(key);
    if (hit && now() - hit.at < hit.ttl) return { ...hit.value, input: normal.input };
    if (now() - windowStart > 60 * 1000) {
      windowStart = now();
      used = 0;
    }
    if (++used > maxPerMinute)
      throw new W3bsError('RATE_LIMITED', 'Browse is busy; try again in a minute.', 429);
    const value = await lookupFresh(normal);
    if (cache.size >= 200) cache.delete(cache.keys().next().value);
    // A failed check is retried sooner: gateways blip.
    cache.set(key, {
      at: now(),
      value,
      ttl: value.gateway.ok ? cacheMs : Math.min(cacheMs, 10000),
    });
    return value;
  }

  return { resolve, safeFetch };
}
