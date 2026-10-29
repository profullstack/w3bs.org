import { request as httpsRequest } from 'node:https';
import { lookup as dnsLookup } from 'node:dns/promises';
import { isIP } from 'node:net';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { createBrotliDecompress, createGunzip, createInflate } from 'node:zlib';
import express from 'express';
import { pageShell } from './site.mjs';
import { ROOT } from './store.mjs';
import { validDomainName } from './doh.mjs';

const MOSHPIT_API = 'https://pit.world';
const REGISTRY_API = 'https://pit.moshcode.sh';
const DEFAULT_TLD = 'whatever';
const DEFAULT_APPROVED_NAMES = ['ai.whatever', 'human.whatever', 'people.whatever'];
const FETCH_TIMEOUT = 8_000;
const PROXY_TIMEOUT = 60_000;
const MAX_HTML = 8 * 1024 * 1024;
const HOP_HEADERS = new Set([
  'connection',
  'keep-alive',
  'proxy-connection',
  'transfer-encoding',
  'upgrade',
  'te',
  'trailer',
  'proxy-authorization',
  'proxy-authenticate',
  'host',
  'accept-encoding',
]);
const ESCAPE = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
const escapeHtml = (value) => String(value ?? '').replace(/[&<>"']/g, (char) => ESCAPE[char]);
const validLabel = (value) => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(value);
const validMoshpitName = (value) =>
  typeof value === 'string' &&
  value.length <= 253 &&
  value.split('.').length >= 2 &&
  value.split('.').every(validLabel);

function privateOrSpecialAddress(address) {
  const family = isIP(address);
  if (family === 4) {
    const [a, b, c] = address.split('.').map(Number);
    return (
      a === 0 ||
      a === 10 ||
      a === 127 ||
      a >= 224 ||
      (a === 100 && b >= 64 && b <= 127) ||
      (a === 169 && b === 254) ||
      (a === 172 && b >= 16 && b <= 31) ||
      (a === 192 && b === 168) ||
      (a === 192 && b === 0 && c === 0) ||
      (a === 192 && b === 0 && c === 2) ||
      (a === 192 && b === 88 && c === 99) ||
      (a === 198 && (b === 18 || b === 19)) ||
      (a === 198 && b === 51 && c === 100) ||
      (a === 203 && b === 0 && c === 113)
    );
  }
  if (family !== 6) return true;
  const input = address.toLowerCase().split('%')[0];
  const parts = input.split('::');
  const expand = (part) => (part ? part.split(':').filter(Boolean) : []);
  let words = [...expand(parts[0])];
  if (parts.length === 2) {
    const right = expand(parts[1]);
    words = [...words, ...Array(Math.max(0, 8 - words.length - right.length)).fill('0'), ...right];
  }
  if (words.length !== 8) return true;
  words = words.map((word) => Number.parseInt(word, 16));
  if (words.slice(0, 5).every((word) => word === 0) && words[5] === 0xffff) {
    const mapped = `${words[6] >> 8}.${words[6] & 255}.${words[7] >> 8}.${words[7] & 255}`;
    return privateOrSpecialAddress(mapped);
  }
  // Only global unicast (2000::/3) is a public destination. Exclude documentation,
  // protocol transition ranges, and special-purpose allocations within that block.
  if ((words[0] & 0xe000) !== 0x2000) return true;
  if (words[0] === 0x2001 && (words[1] <= 0x01ff || words[1] === 0x0db8)) return true;
  if (words[0] === 0x2002) return true;
  return false;
}

export function approvedMoshpitNames(value = process.env.W3BS_NET_APPROVED_NAMES) {
  const configured = value === undefined ? DEFAULT_APPROVED_NAMES : String(value).split(',');
  return new Set(
    configured
      .map((name) => name.trim().toLowerCase())
      .filter((name) => validMoshpitName(name) && name.split('.').length === 2),
  );
}

const safeWebUrl = (value, { httpsOnly = false } = {}) => {
  try {
    const url = new URL(String(value));
    if (
      !['https:', ...(httpsOnly ? [] : ['http:'])].includes(url.protocol) ||
      url.username ||
      url.password
    )
      return null;
    return url;
  } catch {
    return null;
  }
};

function searchForm(query = '') {
  return `<form class="search-bar resolver" action="/search" method="post" role="search"><label class="sr-only" for="net-search">Search the Web and Moshpit</label><input id="net-search" name="q" type="search" value="${escapeHtml(query)}" placeholder="Search the Web or enter a Moshpit name…" maxlength="200" required><button class="button primary" type="submit">Search <span aria-hidden="true">→</span></button></form>`;
}

function resultCards(results = []) {
  const entries = Array.isArray(results) ? results.slice(0, 10) : [];
  if (!entries.length)
    return '<div class="empty"><h2>No Web results.</h2><p>Try another phrase or search a Moshpit name.</p></div>';
  return `<div class="prompt-grid net-result-grid">${entries
    .map((item) => {
      const url = safeWebUrl(item?.url);
      if (!url) return '';
      const host = typeof item.host === 'string' ? item.host : url.hostname;
      const sources = Array.isArray(item.sources)
        ? item.sources.filter((x) => typeof x === 'string').slice(0, 3)
        : [];
      return `<article class="prompt-card net-result"><div class="prompt-top"><span class="pill">${escapeHtml(host)}</span>${sources.length ? `<span class="verified-small">${escapeHtml(sources.join(' · '))}</span>` : ''}</div><h2><a href="${escapeHtml(url.href)}" rel="noopener noreferrer">${escapeHtml(item.title || host)}</a></h2><p>${escapeHtml(item.description || '')}</p><div class="prompt-bottom"><span><code>${escapeHtml(url.origin)}</code></span><span aria-hidden="true">↗</span></div></article>`;
    })
    .join('')}</div>`;
}

function moshpitCard(moshpit, approved) {
  if (!moshpit || typeof moshpit !== 'object' || typeof moshpit.name !== 'string') return '';
  const name = moshpit.name.toLowerCase();
  if (!validMoshpitName(name)) return '';
  const useW3bsHost = approved.has(name);
  const openUrl = useW3bsHost ? `https://${name}.w3bs.org/` : null;
  const registryUrl = `https://pit.moshcode.sh/n/${name}`;
  const target = typeof moshpit.target === 'string' ? moshpit.target : '';
  const claimState =
    moshpit.nameRegistered === false
      ? 'This name is not registered.'
      : `Registered on the Moshpit network${target ? ` · points to <code>${escapeHtml(target)}</code>` : ''}.`;
  const link = openUrl
    ? `<a class="button primary" href="${openUrl}">Open ${escapeHtml(name)} on W3BS <span aria-hidden="true">↗</span></a>`
    : `<a class="button dark" href="${registryUrl}" rel="noopener noreferrer">Open registry page <span aria-hidden="true">↗</span></a>`;
  const moderation = useW3bsHost
    ? '<p class="net-muted">This name is approved for the W3BS gateway.</p>'
    : '<p class="net-muted">This name has not been approved for a W3BS gateway. Registration alone does not add a W3BS host.</p>';
  return `<section class="net-moshpit-card" aria-labelledby="net-moshpit-title"><div class="eyebrow">MOSHPIT NAME</div><h2 id="net-moshpit-title">${escapeHtml(name)}</h2><p>${claimState}</p>${moderation}<div class="actions">${link}<a class="text-link" href="${registryUrl}" rel="noopener noreferrer">View registry record →</a></div></section>`;
}

function searchContent({ query, data, error, approved }) {
  const hasQuery = Boolean(query);
  const moshpit = moshpitCard(data?.moshpit, approved);
  const results = resultCards(data?.results);
  const status = error
    ? `<div class="draft-note"><strong>Search is unavailable.</strong> ${escapeHtml(error)}</div>`
    : hasQuery && !moshpit && !data?.results?.length
      ? '<div class="draft-note">No results came back. Try another search in a moment.</div>'
      : '';
  return `<section class="page-head compact"><div class="eyebrow">W3BS.NET · PRIVATE SEARCH</div><h1>Search the Web.<br><em>Find your Moshpit.</em></h1><p>A W3BS front door for pit.world search and Moshpit names. Search is handled by pit.world; W3BS does not keep a search history.</p></section>${searchForm(query)}${hasQuery ? `<div class="catalog-heading"><span>Results for “${escapeHtml(query)}”</span><span>WEB + MOSHPIT</span></div>${status}${moshpit}${results}` : '<section class="section"><div class="section-heading"><div><div class="eyebrow">THE NETWORK</div><h2>Search by name<br>or by idea.</h2></div><p>Find an ordinary Web page, or look up a name in the Moshpit network. W3BS gateway names are approved by a person before they are served.</p></div><div class="principle-grid"><article><span class="card-index">01 / SEARCH</span><h3>The Web.</h3><p>Search the public Web through pit.world. The query is sent from W3BS to pit.world to fetch results.</p><a href="/net">Explore W3BS Net <span aria-hidden="true">↗</span></a></article><article><span class="card-index">02 / DISCOVER</span><h3>Moshpit names.</h3><p>Look up a Moshpit name and see whether it has been approved for a W3BS gateway.</p><a href="/net">See approved names <span aria-hidden="true">↗</span></a></article><article><span class="card-index">03 / OPEN</span><h3>Through W3BS.</h3><p>Approved names open at <code>name.whatever.w3bs.org</code>, like the pit.world gateway.</p><a href="/browse">Open the W3BS browser <span aria-hidden="true">↗</span></a></article></div></section>'}`;
}

function netContent({ names, error }) {
  const cards = names.length
    ? `<div class="prompt-grid net-name-grid">${names.map((name) => `<a class="prompt-card net-name-card" href="https://${name}.w3bs.org/"><div class="prompt-top"><span class="pill">.WHATEVER</span><span class="verified-small">APPROVED</span></div><h2>${escapeHtml(name)}</h2><p>Registered on Moshpit and approved for a W3BS gateway.</p><div class="prompt-bottom"><span><code>${escapeHtml(name)}.w3bs.org</code></span><span aria-hidden="true">↗</span></div></a>`).join('')}</div>`
    : '<div class="empty"><h2>No approved names are available.</h2><p>The public Moshpit registry may be unavailable right now.</p></div>';
  const notice = error
    ? `<div class="draft-note"><strong>Registry lookup failed.</strong> ${escapeHtml(error)}</div>`
    : '';
  return `<section class="page-head"><div class="eyebrow">W3BS.NET · MOSHPIT NAMES</div><h1>The open network.<br><em>Names with a front door.</em></h1><p>Browse approved names from the Moshpit <code>.whatever</code> namespace. Open one as <code>name.whatever.w3bs.org</code> in any browser.</p><div class="actions"><a class="button primary" href="/search">Search the Web and Moshpit <span aria-hidden="true">→</span></a><a class="text-link" href="/browse">W3BS browser <span aria-hidden="true">↗</span></a></div></section><section class="section net-listing"><div class="section-heading"><div><div class="eyebrow">APPROVED FOR W3BS</div><h2>The .whatever names.</h2></div><p>Names appear here only after manual W3BS approval. Registration on Moshpit does not automatically create a W3BS host.</p></div>${notice}${cards}</section><aside class="callout net-policy"><strong>No pre-buys, paid claims or reservations.</strong><p>W3BS does not sell Moshpit names. Names are registered through the Moshpit registry; W3BS only approves which names get a W3BS gateway.</p><a href="mailto:hello@w3bs.org?subject=W3BS%20Net%20name%20review">Ask for a name review →</a></aside>`;
}

const netShell = ({ title, description, active = '/net', content, canonical = '' }) =>
  pageShell({ title, description, active, content, canonical, track: false });

function gatewayInfoPage({ name, title, message, target = '' }) {
  const host = `${name}.w3bs.org`;
  const targetLink =
    target && validDomainName(target)
      ? `<p><a class="button primary" href="https://${escapeHtml(target)}" rel="noopener noreferrer">Open destination ↗</a></p>`
      : '';
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="robots" content="noindex"><title>${escapeHtml(title)} — W3BS Net</title><meta name="theme-color" content="#f6f4ed"><link rel="stylesheet" href="https://w3bs.org/assets/style.css"><link rel="icon" type="image/svg+xml" href="https://w3bs.org/assets/mark.svg"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src https://w3bs.org; img-src https://w3bs.org data:; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self' https:"></head><body><a class="skip" href="#main">Skip to content</a><header class="header"><a class="wordmark" href="https://w3bs.org/net" aria-label="W3BS Net"><img src="https://w3bs.org/assets/mark.svg" alt="" width="28" height="28">w3bs<span class="wordmark-dot">.</span></a><nav aria-label="W3BS Net"><div class="nav-inline"><a href="https://w3bs.org/net">Net</a><a href="https://w3bs.org/search">Search</a></div></nav><a class="header-action" href="https://w3bs.org">w3bs.org <span aria-hidden="true">↗</span></a></header><main id="main"><section class="page-head"><div class="eyebrow">W3BS.NET · MOSHPIT GATEWAY</div><h1>${escapeHtml(name)}</h1><p>${escapeHtml(message)}</p>${targetLink}<p><code>https://${escapeHtml(host)}</code> · <a href="https://pit.moshcode.sh/n/${encodeURIComponent(name)}" rel="noopener noreferrer">Moshpit registry record ↗</a></p></section></main><footer><div><a class="wordmark" href="https://w3bs.org/net">w3bs<span class="wordmark-dot">.</span></a><p>W3BS Net · A moderated gateway to Moshpit.</p><a href="https://w3bs.org/search">Search</a></div></footer></body></html>`;
}

function sendGatewayPage(req, res, status, name, title, message, target = '') {
  const body = gatewayInfoPage({ name, title, message, target });
  res.status(status).set({
    'Content-Type': 'text/html; charset=utf-8',
    'Cache-Control': 'no-store',
    'X-Robots-Tag': 'noindex',
    'X-Content-Type-Options': 'nosniff',
    'Referrer-Policy': 'no-referrer',
    'Content-Security-Policy':
      "default-src 'none'; style-src https://w3bs.org; img-src https://w3bs.org data:; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self' https:",
    'Permissions-Policy': 'camera=(), microphone=(), geolocation=()',
  });
  if (req.method === 'HEAD') return res.end();
  return res.send(body);
}

function parseCookie(value) {
  return String(value).replace(/;\s*domain=[^;]*/gi, '');
}

function rewriteUrlText(text, name, gatewayHost) {
  const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return String(text).replace(
    new RegExp(`(https?:)?//${escaped}(?![a-z0-9.-])(:\\d+)?`, 'gi'),
    (_match, scheme) => `${scheme ? 'https:' : ''}//${gatewayHost}`,
  );
}

function rewriteBrowserHeaders(req, name, gatewayHost) {
  const headers = {};
  for (const [key, rawValue] of Object.entries(req.headers)) {
    const lower = key.toLowerCase();
    if (
      HOP_HEADERS.has(lower) ||
      lower.startsWith('x-forwarded-') ||
      lower === 'x-real-ip' ||
      lower === 'expect'
    )
      continue;
    const value = Array.isArray(rawValue) ? rawValue.join(', ') : String(rawValue ?? '');
    if (lower === 'origin' || lower === 'referer') {
      headers[lower] = value.replace(
        new RegExp(
          `^https?://${gatewayHost.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}(?=[:/]|$)`,
          'i',
        ),
        `https://${name}`,
      );
    } else if (lower !== 'content-length') headers[lower] = value;
    else headers[lower] = value;
  }
  headers.host = name;
  headers['accept-encoding'] = 'identity';
  headers['x-forwarded-host'] = gatewayHost;
  headers['x-forwarded-proto'] = 'https';
  headers['x-moshpit-name'] = name;
  const forwardedIp = String(req.headers['x-real-ip'] ?? '').trim();
  const clientIp = isIP(forwardedIp) ? forwardedIp : String(req.socket.remoteAddress ?? '');
  if (isIP(clientIp)) {
    headers['x-real-ip'] = clientIp;
    headers['x-forwarded-for'] = clientIp;
  }
  return headers;
}

function resolveTargetAddress(target) {
  const value = String(target ?? '')
    .trim()
    .toLowerCase()
    .replace(/\.$/, '');
  if (
    !value ||
    value.includes('/') ||
    value.includes('@') ||
    (value.includes(':') && isIP(value) === 0)
  )
    return Promise.reject(new Error('invalid destination'));
  if (isIP(value)) {
    if (privateOrSpecialAddress(value))
      return Promise.reject(new Error('destination is not public'));
    return Promise.resolve({ address: value });
  }
  if (!validDomainName(value)) return Promise.reject(new Error('invalid destination'));
  return dnsLookup(value, { all: true, verbatim: true }).then((addresses) => {
    if (!addresses.length || addresses.some(({ address }) => privateOrSpecialAddress(address)))
      throw new Error('destination is not public');
    return addresses[0];
  });
}

function decompressStream(stream, encoding) {
  if (encoding === 'gzip' || encoding === 'x-gzip') return stream.pipe(createGunzip());
  if (encoding === 'br') return stream.pipe(createBrotliDecompress());
  if (encoding === 'deflate') return stream.pipe(createInflate());
  return stream;
}

async function boundedBody(stream, limit) {
  const chunks = [];
  let size = 0;
  for await (const chunk of stream) {
    size += chunk.length;
    if (size > limit) throw new Error('page is too large to rewrite');
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}

function resolveName(fetchImpl, name) {
  return fetchImpl(`${MOSHPIT_API}/api/resolve?name=${encodeURIComponent(name)}`, {
    headers: { accept: 'application/json', 'user-agent': 'w3bs-net/1.0 (+https://w3bs.org/net)' },
    redirect: 'error',
    signal: AbortSignal.timeout(FETCH_TIMEOUT),
  }).then(async (response) => {
    if (!response.ok) throw new Error(`Moshpit lookup returned ${response.status}`);
    const data = await response.json();
    return data && typeof data === 'object' ? data : null;
  });
}

async function finalTarget(fetchImpl, name) {
  let current = name;
  const seen = new Set();
  for (let i = 0; i < 8; i++) {
    if (seen.has(current)) throw new Error('Moshpit alias cycle');
    seen.add(current);
    const entry = await resolveName(fetchImpl, current);
    if (!entry?.nameRegistered) return { state: 'missing' };
    const target = String(entry.target ?? '')
      .trim()
      .toLowerCase()
      .replace(/\.$/, '');
    if (!target) return { state: 'empty' };
    if (isIP(target) || validDomainName(target)) {
      // A Moshpit alias is resolved by the same public lookup. An unregistered two-label
      // name is a clearnet target and will be DNS checked before a socket is opened.
      if (validMoshpitName(target)) {
        const alias = await resolveName(fetchImpl, target);
        if (alias?.nameRegistered) {
          current = target;
          continue;
        }
      }
      return { state: 'target', target };
    }
    return { state: 'invalid' };
  }
  throw new Error('Moshpit alias chain is too long');
}

async function proxyRequest(req, res, name, target, rootPem) {
  const address = await resolveTargetAddress(target);
  const gatewayHost = `${name}.w3bs.org`;
  const headers = rewriteBrowserHeaders(req, name, gatewayHost);
  const upstream = await new Promise((resolvePromise, reject) => {
    const request = httpsRequest(
      {
        host: address.address,
        family: isIP(address.address),
        port: 443,
        method: req.method,
        path: `${req.originalUrl || req.url}`,
        servername: name,
        ca: rootPem,
        headers,
        timeout: PROXY_TIMEOUT,
        rejectUnauthorized: true,
      },
      resolvePromise,
    );
    request.setTimeout(PROXY_TIMEOUT, () => request.destroy(new Error('destination timed out')));
    request.once('error', reject);
    if (req.method === 'GET' || req.method === 'HEAD' || req.readableEnded) request.end();
    else req.pipe(request);
  });

  const gatewayHeaders = {};
  const originalHeaders = upstream.headers;
  for (const [key, value] of Object.entries(originalHeaders)) {
    if (
      value === undefined ||
      HOP_HEADERS.has(key.toLowerCase()) ||
      key.toLowerCase() === 'content-encoding'
    )
      continue;
    if (key.toLowerCase() === 'set-cookie') {
      gatewayHeaders[key] = (Array.isArray(value) ? value : [value]).map(parseCookie);
    } else if (key.toLowerCase() === 'location') {
      gatewayHeaders[key] = rewriteUrlText(String(value), name, gatewayHost);
    } else if (
      key.toLowerCase() === 'content-security-policy' ||
      key.toLowerCase() === 'content-security-policy-report-only'
    ) {
      gatewayHeaders[key] = rewriteUrlText(String(value), name, gatewayHost);
    } else {
      gatewayHeaders[key] = value;
    }
  }
  gatewayHeaders['X-Content-Type-Options'] = 'nosniff';
  gatewayHeaders['Referrer-Policy'] = 'strict-origin-when-cross-origin';
  gatewayHeaders['Permissions-Policy'] = 'camera=(), microphone=(), geolocation=()';
  gatewayHeaders['X-Robots-Tag'] = 'noindex';
  gatewayHeaders['Cache-Control'] ??= 'no-store';
  gatewayHeaders['Content-Security-Policy'] = originalHeaders['content-security-policy']
    ? rewriteUrlText(String(originalHeaders['content-security-policy']), name, gatewayHost)
    : "default-src 'self' https: data: blob:; script-src 'self' https: 'unsafe-inline' 'unsafe-eval'; style-src 'self' https: 'unsafe-inline'; img-src 'self' https: data: blob:; connect-src 'self' https: wss:; font-src 'self' https: data:; frame-src 'self' https:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'; form-action 'self' https:";
  const status = upstream.statusCode || 502;
  const type = String(originalHeaders['content-type'] ?? '').toLowerCase();
  const contentEncoding = String(originalHeaders['content-encoding'] ?? '').toLowerCase();
  const isHtml = type.includes('text/html') || type.includes('application/xhtml+xml');
  if (req.method === 'HEAD' || status === 204 || status === 304) {
    upstream.resume();
    res.status(status).set(gatewayHeaders).end();
    return;
  }
  if (isHtml) {
    delete gatewayHeaders['content-length'];
    const decoded = decompressStream(upstream, contentEncoding);
    const body = await boundedBody(decoded, MAX_HTML);
    const rewritten = rewriteUrlText(body.toString('utf8'), name, gatewayHost);
    res.status(status).set(gatewayHeaders).send(rewritten);
    return;
  }
  if (contentEncoding) gatewayHeaders['Content-Encoding'] = contentEncoding;
  res.status(status).set(gatewayHeaders);
  upstream.once('error', (error) => res.destroy(error));
  upstream.pipe(res);
}

export function createNet({
  fetchImpl = fetch,
  approvedNames = approvedMoshpitNames(),
  tld = DEFAULT_TLD,
  registry = REGISTRY_API,
} = {}) {
  const approved = approvedNames instanceof Set ? approvedNames : new Set(approvedNames);
  const rootPem = readFileSync(resolve(ROOT, 'public/moshpit-root-ca.crt'), 'utf8');
  let namesCache = { at: 0, names: null };

  async function registeredNames() {
    if (namesCache.names && Date.now() - namesCache.at < 30_000)
      return { names: namesCache.names, error: '' };
    try {
      const url = `${registry}/api/moshpit/tlds/${encodeURIComponent(tld)}/names`;
      const response = await fetchImpl(url, {
        headers: {
          accept: 'application/json',
          'user-agent': 'w3bs-net/1.0 (+https://w3bs.org/net)',
        },
        redirect: 'error',
        signal: AbortSignal.timeout(FETCH_TIMEOUT),
      });
      if (!response.ok) throw new Error(`registry returned ${response.status}`);
      const data = await response.json();
      const names = Array.isArray(data?.names) ? data.names : [];
      namesCache = { at: Date.now(), names };
      return { names, error: '' };
    } catch {
      return { names: namesCache.names || [], error: 'The Moshpit registry could not be reached.' };
    }
  }

  async function renderSearch(req, res, rawQuery) {
    const query = String(rawQuery ?? '')
      .trim()
      .slice(0, 200);
    let data = null;
    let error = '';
    if (query) {
      try {
        const url = new URL('/api/search', MOSHPIT_API);
        url.searchParams.set('q', query);
        url.searchParams.set('n', '10');
        const response = await fetchImpl(url, {
          headers: {
            accept: 'application/json',
            'user-agent': 'w3bs-net/1.0 (+https://w3bs.org/net)',
          },
          redirect: 'error',
          signal: AbortSignal.timeout(FETCH_TIMEOUT),
        });
        data = await response.json();
        if (!response.ok)
          error = typeof data?.error === 'string' ? data.error : 'Try again shortly.';
      } catch {
        error = 'The pit.world search service could not be reached. Try again shortly.';
      }
    }
    const description = 'Search the Web and Moshpit through W3BS Net.';
    const content = searchContent({ query, data, error, approved });
    return res
      .status(query && error ? 503 : 200)
      .type('html')
      .send(
        netShell({
          title: 'Search the Web and Moshpit',
          description,
          active: '/search',
          content,
          canonical: 'https://w3bs.org/search',
        }),
      );
  }

  function nameFromHost(hostname) {
    const host = String(hostname ?? '')
      .toLowerCase()
      .replace(/:\d+$/, '')
      .replace(/\.$/, '');
    const suffix = '.w3bs.org';
    if (!host.endsWith(suffix)) return null;
    const name = host.slice(0, -suffix.length);
    if (!validMoshpitName(name) || name.split('.').length !== 2) return null;
    return approved.has(name) ? name : null;
  }

  async function gateway(req, res, name) {
    if (!['GET', 'HEAD', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS'].includes(req.method)) {
      res.set('Allow', 'GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS').status(405).end();
      return;
    }
    try {
      const resolved = await resolveName(fetchImpl, name);
      if (!resolved?.nameRegistered) {
        sendGatewayPage(
          req,
          res,
          404,
          name,
          `${name} is not registered`,
          'This W3BS gateway is approved, but the name is not currently registered on Moshpit.',
        );
        return;
      }
      const route = await finalTarget(fetchImpl, name);
      if (route.state === 'missing') {
        sendGatewayPage(
          req,
          res,
          404,
          name,
          `${name} is not registered`,
          'This name is no longer registered on Moshpit.',
        );
        return;
      }
      if (route.state === 'empty') {
        sendGatewayPage(
          req,
          res,
          200,
          name,
          `${name} is on Moshpit`,
          'This name is registered and approved for W3BS, but no Web destination is set yet.',
        );
        return;
      }
      if (route.state !== 'target') {
        sendGatewayPage(
          req,
          res,
          502,
          name,
          'Moshpit destination unavailable',
          'The name does not have a usable public Web destination.',
        );
        return;
      }
      await proxyRequest(req, res, name, route.target, rootPem);
    } catch (error) {
      if (res.headersSent) return res.destroy(error);
      sendGatewayPage(
        req,
        res,
        502,
        name,
        'Moshpit gateway unavailable',
        'The destination could not be reached securely. Try again shortly.',
      );
    }
  }

  function mount(app) {
    app.get('/net', async (_req, res) => {
      const { names, error } = await registeredNames();
      const visible = names
        .map(
          (item) =>
            `${String(item?.label ?? '').toLowerCase()}.${String(item?.tld ?? '').toLowerCase()}`,
        )
        .filter((name) => approved.has(name))
        .sort();
      const content = netContent({ names: visible, error });
      return res.type('html').send(
        netShell({
          title: 'W3BS Net · Moshpit names',
          description: 'Browse Moshpit names manually approved for a W3BS gateway.',
          active: '/net',
          content,
          canonical: 'https://w3bs.org/net',
        }),
      );
    });
    app.get('/search', (req, res) => renderSearch(req, res, req.query.q));
    app.post('/search', express.urlencoded({ extended: false, limit: '2kb' }), (req, res) => {
      const query = typeof req.body?.q === 'string' ? req.body.q : '';
      return renderSearch(req, res, query);
    });
  }

  return { approved, mount, nameFromHost, gateway };
}
