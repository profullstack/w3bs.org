import test from 'node:test';
import assert from 'node:assert/strict';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { startServer } from '../src/server.mjs';
import { openStore } from '../src/store.mjs';
import { createBrowser, normalizeInput, parseMoshpitTxt, pageTitle } from '../src/browse.mjs';
import { createDohClient, encodeQuery, parseResponse, TYPES } from '../src/doh.mjs';
import { MOSHPIT_EXAMPLES, WEB_EXAMPLES } from '../src/browse-examples.mjs';

const ORIGIN = 'http://localhost:3000';

// ---- A real DNS wire response, built by hand (RFC 1035 §4.1) ----
const nameBytes = (name) =>
  Buffer.concat([
    ...name.split('.').flatMap((label) => [Buffer.from([label.length]), Buffer.from(label)]),
    Buffer.from([0]),
  ]);
const u16 = (n) => Buffer.from([n >> 8, n & 0xff]);
const u32 = (n) => Buffer.from([(n >>> 24) & 0xff, (n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff]);
function rdataOf(type, data) {
  if (type === 'A') return Buffer.from(data.split('.').map(Number));
  if (type === 'AAAA') {
    const [head, tail = ''] = data.split('::');
    const h = head ? head.split(':') : [];
    const t = tail ? tail.split(':') : [];
    const groups = [...h, ...Array(8 - h.length - t.length).fill('0'), ...t];
    return Buffer.concat(groups.map((g) => u16(parseInt(g, 16))));
  }
  if (type === 'CNAME') return nameBytes(data);
  if (type === 'TXT')
    return Buffer.concat(
      [].concat(data).flatMap((part) => [Buffer.from([part.length]), Buffer.from(part)]),
    );
  throw new Error(type);
}
// records: [{ type, data, name? }]; a record without name uses a pointer to the question.
function dnsResponse(qname, qtype, records = [], rcode = 0) {
  const header = Buffer.concat([
    u16(0),
    u16(0x8180 | rcode),
    u16(1),
    u16(records.length),
    u16(0),
    u16(0),
  ]);
  const question = Buffer.concat([nameBytes(qname), u16(TYPES[qtype]), u16(1)]);
  const answers = records.map(({ type, data, name }) => {
    const rdata = rdataOf(type, data);
    return Buffer.concat([
      name ? nameBytes(name) : Buffer.from([0xc0, 12]),
      u16(TYPES[type]),
      u16(1),
      u32(60),
      u16(rdata.length),
      rdata,
    ]);
  });
  return Buffer.concat([header, question, ...answers]);
}
function decodeQuestion(bytes) {
  const labels = [];
  let at = 12;
  while (bytes[at]) {
    labels.push(bytes.toString('ascii', at + 1, at + 1 + bytes[at]));
    at += 1 + bytes[at];
  }
  const code = bytes.readUInt16BE(at + 1);
  return { name: labels.join('.'), type: Object.keys(TYPES).find((k) => TYPES[k] === code) };
}

// A fake Moshpit zone, answered over a fake DoH transport that speaks real wire format.
const ZONE = {
  'chovy.hacker': {
    A: [
      { type: 'CNAME', data: 'dev.profullstack.com' },
      { type: 'A', data: '23.95.228.174', name: 'dev.profullstack.com' },
    ],
    AAAA: [{ type: 'CNAME', data: 'dev.profullstack.com' }],
    TXT: [
      {
        type: 'TXT',
        data: ['v=moshpit1 name=chovy.hacker resolved=chovy.hacker target=dev.profullstack.com'],
      },
    ],
  },
  'w3bs.example': { A: [{ type: 'A', data: '93.184.216.34' }], AAAA: [], TXT: [] },
};
function fakeDoh(zone = ZONE) {
  const asked = [];
  const fetchImpl = async (url, init) => {
    asked.push(new URL(url));
    assert.equal(init.headers.accept, 'application/dns-message');
    const q = decodeQuestion(Buffer.from(new URL(url).searchParams.get('dns'), 'base64url'));
    const records = zone[q.name]?.[q.type] ?? [];
    return new Response(dnsResponse(q.name, q.type, records), {
      status: 200,
      headers: { 'content-type': 'application/dns-message' },
    });
  };
  return { client: createDohClient({ fetchImpl }), asked, fetchImpl };
}

// Fake web: url -> { status, headers, body }.
function fakeWeb(routes) {
  const calls = [];
  const fetchImpl = async (url, init) => {
    calls.push(String(url));
    assert.equal(init.redirect, 'manual');
    const route = routes[String(url)];
    if (!route)
      return new Response('not found', { status: 404, headers: { 'content-type': 'text/html' } });
    return new Response(route.body ?? '', {
      status: route.status ?? 200,
      headers: route.headers ?? {},
    });
  };
  return { fetchImpl, calls };
}
const publicLookup = async () => [{ address: '23.95.228.174', family: 4 }];
const GOOD_SITE = {
  'https://chovy.hacker.pit.world/': {
    headers: { 'content-type': 'text/html' },
    body: '<html><head><title>chovy.hacker &amp; friends</title></head></html>',
  },
  'https://chovy.hacker.pit.world/.well-known/w3bs.json': {
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      specVersion: 'W3BS-DISCOVERY-1-draft-1',
      registry: 'https://chovy.hacker.pit.world',
      mcp: 'https://chovy.hacker.pit.world/mcp',
      surfaces: { web: 'javascript:alert(1)' },
      operations: ['resolve'],
    }),
  },
};

test('normalizeInput: W3BS URIs, Moshpit names, pit.world gateways and plain URLs', () => {
  assert.deepEqual(normalizeInput('w3bs://prompt/w3bs/research@1'), {
    kind: 'w3bs',
    input: 'w3bs://prompt/w3bs/research@1',
    uri: 'w3bs://prompt/w3bs/research@1',
  });
  assert.equal(normalizeInput('  '), null);
  const bare = normalizeInput('chovy.hacker');
  assert.equal(bare.kind, 'web');
  assert.equal(bare.candidate, true);
  assert.equal(bare.host, 'chovy.hacker');
  // w3bs:// with a dotted authority names something to browse, not a resource type.
  const named = normalizeInput('w3bs://chovy.hacker');
  assert.equal(named.kind, 'web');
  assert.equal(named.candidate, true);
  assert.equal(named.host, 'chovy.hacker');
  assert.equal(named.input, 'w3bs://chovy.hacker');
  assert.equal(normalizeInput('W3BS://example.com/a?b=1').url, 'https://example.com/a?b=1');
  assert.equal(normalizeInput('w3bs://chovy.hacker.pit.world/x').kind, 'moshpit');
  for (const input of [
    'https://chovy.hacker.pit.world/path?x=1',
    'chovy.hacker.pit.world/path?x=1',
    'http://CHOVY.Hacker.pit.world./path?x=1',
  ]) {
    const n = normalizeInput(input);
    assert.equal(n.kind, 'moshpit', input);
    assert.equal(n.name, 'chovy.hacker');
    assert.equal(n.path, '/path?x=1');
  }
  // pit.world itself, and one-label gateway hosts, are ordinary ICANN sites.
  assert.equal(normalizeInput('pit.world').kind, 'web');
  assert.equal(normalizeInput('pit.world').candidate, false);
  assert.equal(normalizeInput('www.pit.world').kind, 'web');
  const url = normalizeInput('https://www.profullstack.com/about');
  assert.equal(url.kind, 'web');
  assert.equal(url.candidate, false);
  assert.equal(url.url, 'https://www.profullstack.com/about');
  for (const bad of [
    'ftp://example.com',
    'file:///etc/passwd',
    'https://127.0.0.1/',
    'https://[::1]/',
    '10.0.0.1',
    'https://user:pw@example.com/',
    'https://example.com:8443/',
    'localhost',
    'not a url',
  ])
    assert.throws(() => normalizeInput(bad), { code: 'INVALID_INPUT' }, bad);
});

test('DoH: real wire-format answers parse (CNAME chain, A, AAAA, multi-string TXT)', () => {
  const query = encodeQuery('chovy.hacker', 'A');
  assert.deepEqual([...query.subarray(0, 12)], [0, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
  assert.deepEqual(decodeQuestion(query), { name: 'chovy.hacker', type: 'A' });
  const wire = dnsResponse('chovy.hacker', 'A', [
    { type: 'CNAME', data: 'dev.profullstack.com' },
    { type: 'A', data: '23.95.228.174', name: 'dev.profullstack.com' },
    { type: 'AAAA', data: '2001:db8::1' },
    { type: 'TXT', data: ['v=moshpit1 name=chovy.hacker ', 'target=dev.profullstack.com'] },
  ]);
  const parsed = parseResponse(wire);
  assert.equal(parsed.rcode, 'NOERROR');
  assert.deepEqual(parsed.answers, [
    { name: 'chovy.hacker', type: 'CNAME', ttl: 60, data: 'dev.profullstack.com' },
    { name: 'dev.profullstack.com', type: 'A', ttl: 60, data: '23.95.228.174' },
    { name: 'chovy.hacker', type: 'AAAA', ttl: 60, data: '2001:db8::1' },
    {
      name: 'chovy.hacker',
      type: 'TXT',
      ttl: 60,
      data: 'v=moshpit1 name=chovy.hacker target=dev.profullstack.com',
    },
  ]);
  assert.equal(parseResponse(dnsResponse('nope.hacker', 'A', [], 3)).rcode, 'NXDOMAIN');
  assert.throws(() => parseResponse(Buffer.alloc(5)), { code: 'DNS_FAILED' });
  // A pointer loop is refused, not followed forever.
  const loop = Buffer.concat([
    u16(0),
    u16(0x8180),
    u16(1),
    u16(0),
    u16(0),
    u16(0),
    Buffer.from([0xc0, 12]),
  ]);
  assert.throws(() => parseResponse(loop), { code: 'DNS_FAILED' });
  assert.deepEqual(parseMoshpitTxt(['v=spf1 -all', 'v=moshpit1 name=a.b target=x.example']), {
    name: 'a.b',
    target: 'x.example',
  });
  assert.equal(parseMoshpitTxt(['v=spf1 -all']), null);
  assert.equal(pageTitle('<title> pay.agent &middot; links </title>'), 'pay.agent · links');
});

test('DoH client: only dns.moshcode.sh, GET ?dns= base64url, cached', async () => {
  assert.throws(() => createDohClient({ endpoint: 'https://1.1.1.1/dns-query' }), /allowlist/);
  assert.throws(() => createDohClient({ endpoint: 'https://evil.example/dns-query' }), /allowlist/);
  assert.throws(
    () => createDohClient({ endpoint: 'http://dns.moshcode.sh/dns-query' }),
    /allowlist/,
  );
  const doh = fakeDoh();
  const first = await doh.client.query('chovy.hacker', 'TXT');
  const second = await doh.client.query('chovy.hacker', 'TXT');
  assert.deepEqual(first, second);
  assert.equal(doh.asked.length, 1);
  assert.equal(doh.asked[0].hostname, 'dns.moshcode.sh');
  assert.equal(doh.asked[0].pathname, '/dns-query');
  assert.match(doh.asked[0].searchParams.get('dns'), /^[A-Za-z0-9_-]+$/);
  await assert.rejects(doh.client.query('bad name', 'A'), { code: 'INVALID_INPUT' });
  const down = createDohClient({ fetchImpl: async () => new Response('', { status: 500 }) });
  await assert.rejects(down.query('chovy.hacker', 'A'), { code: 'DNS_FAILED' });
});

test('a Moshpit name resolves over DoH, checks its gateway and reads its discovery document', async () => {
  const web = fakeWeb(GOOD_SITE);
  const browser = createBrowser({
    doh: fakeDoh().client,
    fetchImpl: web.fetchImpl,
    lookup: publicLookup,
  });
  const r = await browser.resolve('chovy.hacker');
  assert.equal(r.kind, 'moshpit');
  assert.equal(r.name, 'chovy.hacker');
  assert.equal(r.primary, 'https://chovy.hacker/');
  assert.equal(r.fallback, 'https://chovy.hacker.pit.world/');
  assert.deepEqual(r.dns.CNAME, ['dev.profullstack.com']);
  assert.deepEqual(r.dns.A, ['23.95.228.174']);
  assert.equal(r.moshpit.minted, true);
  assert.equal(r.moshpit.target, 'dev.profullstack.com');
  assert.equal(r.gateway.ok, true);
  assert.equal(r.gateway.title, 'chovy.hacker & friends');
  assert.equal(r.discovery.found, true);
  assert.equal(r.discovery.document.mcp, 'https://chovy.hacker.pit.world/mcp');
  // Only the gateway was fetched; never the Moshpit target itself.
  assert.ok(
    web.calls.every((u) => new URL(u).hostname === 'chovy.hacker.pit.world'),
    web.calls,
  );
  // The gateway URL form lands on the same answer, and repeats come from the cache.
  const again = await browser.resolve('https://chovy.hacker.pit.world/');
  assert.equal(again.name, 'chovy.hacker');
  assert.equal(web.calls.length, 2);
  // An ICANN host is shown as it is.
  const plain = await browser.resolve('w3bs.example');
  assert.equal(plain.kind, 'web');
  assert.equal(plain.primary, 'https://w3bs.example/');
  assert.equal(plain.fallback, null);
  assert.equal(plain.moshpit, null);
});

test('SSRF: private addresses, other hosts, http, redirect loops and big bodies are refused', async () => {
  const doh = fakeDoh().client;
  const privateNet = createBrowser({
    doh,
    fetchImpl: fakeWeb(GOOD_SITE).fetchImpl,
    lookup: async () => [{ address: '10.0.0.7', family: 4 }],
  });
  const r = await privateNet.resolve('chovy.hacker');
  assert.equal(r.gateway.ok, false);
  assert.match(r.gateway.error, /private address/);
  assert.match(r.discovery.error, /private address/);
  for (const address of ['127.0.0.1', '::1', '169.254.169.254', '192.168.1.1', '::ffff:10.0.0.1']) {
    const b = createBrowser({
      doh,
      fetchImpl: fakeWeb(GOOD_SITE).fetchImpl,
      lookup: async () => [{ address }],
    });
    assert.match((await b.resolve('chovy.hacker')).gateway.error, /private address/, address);
  }
  const redirecting = (location) =>
    createBrowser({
      doh,
      lookup: publicLookup,
      fetchImpl: fakeWeb({
        'https://chovy.hacker.pit.world/': { status: 302, headers: { location } },
      }).fetchImpl,
    });
  assert.match(
    (await redirecting('https://169.254.169.254/latest').resolve('chovy.hacker')).gateway.error,
    /refused a redirect to 169\.254\.169\.254/,
  );
  assert.match(
    (await redirecting('http://chovy.hacker.pit.world/').resolve('chovy.hacker')).gateway.error,
    /https only/,
  );
  assert.match(
    (await redirecting('https://dev.profullstack.com/').resolve('chovy.hacker')).gateway.error,
    /refused a redirect/,
  );
  // A redirect loop on the gateway stops after 3 hops.
  const loop = fakeWeb({
    'https://chovy.hacker.pit.world/': { status: 301, headers: { location: '/a' } },
    'https://chovy.hacker.pit.world/a': { status: 301, headers: { location: '/b' } },
    'https://chovy.hacker.pit.world/b': { status: 301, headers: { location: '/c' } },
    'https://chovy.hacker.pit.world/c': { status: 301, headers: { location: '/' } },
  });
  const looped = await createBrowser({
    doh,
    lookup: publicLookup,
    fetchImpl: loop.fetchImpl,
  }).resolve('chovy.hacker');
  assert.match(looped.gateway.error, /too many redirects/);
  // Up to 3 redirects on the same host are fine.
  const hop = fakeWeb({
    'https://chovy.hacker.pit.world/': { status: 302, headers: { location: '/home' } },
    'https://chovy.hacker.pit.world/home': {
      headers: { 'content-type': 'text/html' },
      body: '<title>home</title>',
    },
  });
  const hopped = await createBrowser({
    doh,
    lookup: publicLookup,
    fetchImpl: hop.fetchImpl,
  }).resolve('chovy.hacker');
  assert.equal(hopped.gateway.finalUrl, 'https://chovy.hacker.pit.world/home');
  assert.equal(hopped.gateway.title, 'home');
  // A discovery document over 64 KB is not parsed.
  const big = fakeWeb({
    'https://chovy.hacker.pit.world/.well-known/w3bs.json': {
      body: `{"x":"${'a'.repeat(70000)}"}`,
    },
  });
  const huge = await createBrowser({ doh, lookup: publicLookup, fetchImpl: big.fetchImpl }).resolve(
    'chovy.hacker',
  );
  assert.equal(huge.discovery.found, false);
  assert.match(huge.discovery.error, /64 KB/);
  // Uncached lookups are rate limited.
  const limited = createBrowser({
    doh,
    lookup: publicLookup,
    fetchImpl: fakeWeb({}).fetchImpl,
    maxPerMinute: 1,
  });
  await limited.resolve('chovy.hacker');
  await assert.rejects(limited.resolve('w3bs.example'), { code: 'RATE_LIMITED' });
});

async function boot() {
  const store = await openStore({ path: ':memory:' });
  const web = fakeWeb(GOOD_SITE);
  const instance = await startServer({
    store,
    port: 0,
    publicOrigin: ORIGIN,
    mailer: { send: async () => ({ sent: true }) },
    browser: createBrowser({
      doh: fakeDoh().client,
      fetchImpl: web.fetchImpl,
      lookup: publicLookup,
    }),
  });
  const base = `http://localhost:${instance.server.address().port}`;
  return { base, close: () => instance.close() };
}

test('/browse shows the Moshpit name first, the pit.world gateway as fallback, and the examples', async () => {
  const t = await boot();
  try {
    const html = await (await fetch(`${t.base}/browse?q=chovy.hacker.pit.world`)).text();
    assert.match(html, /<h2>chovy\.hacker<\/h2>/);
    assert.match(html, /MOSHPIT NAME/);
    assert.match(
      html,
      /<a class="button primary" href="https:\/\/chovy\.hacker\/"[^>]*>Open chovy\.hacker/,
    );
    assert.match(
      html,
      /href="https:\/\/chovy\.hacker\.pit\.world\/"[^>]*>via chovy\.hacker\.pit\.world/,
    );
    assert.ok(
      html.indexOf('https://chovy.hacker/') < html.indexOf('https://chovy.hacker.pit.world/'),
    );
    assert.match(html, /Gateway answers/);
    assert.match(html, /dev\.profullstack\.com/);
    assert.match(html, /W3BS-DISCOVERY-1-draft-1/);
    // A declared javascript: URL is shown as text, never as a link.
    assert.doesNotMatch(html, /href="javascript:/);
    assert.match(html, /javascript:alert\(1\)/);
    // The example grid, from src/browse-examples.mjs.
    for (const { name } of MOSHPIT_EXAMPLES)
      assert.ok(html.includes(`href="/browse?q=${name}"`), name);
    for (const { host } of WEB_EXAMPLES) assert.ok(html.includes(`href="/browse?q=${host}"`), host);
    for (const command of [
      'w3bs resolve w3bs://prompt/w3bs/research@1',
      'w3bs packages',
      'dig +https @dns.moshcode.sh chovy.hacker TXT +short',
    ])
      assert.ok(html.includes(`data-copy="${command}"`), command);

    // The W3BS resolver still works, by ?uri= (old links) and ?q=.
    for (const query of ['uri', 'q']) {
      const page = await (
        await fetch(
          `${t.base}/browse?${query}=${encodeURIComponent('w3bs://prompt/w3bs/research@1')}`,
        )
      ).text();
      assert.match(page, /CANONICAL IDENTITY/, query);
    }
    const bad = await (await fetch(`${t.base}/browse?q=ftp%3A%2F%2Fexample.com`)).text();
    assert.match(bad, /Could not browse this\./);

    // w3bs://<name> is an address: /browse opens the site instead of describing it.
    const opened = await fetch(
      `${t.base}/browse?q=${encodeURIComponent('w3bs://chovy.hacker/blog')}`,
      {
        redirect: 'manual',
      },
    );
    assert.equal(opened.status, 302);
    assert.equal(opened.headers.get('location'), 'https://chovy.hacker.pit.world/blog');
    // A plain name, and a resource URI, still get the page.
    assert.equal(
      (await fetch(`${t.base}/browse?q=chovy.hacker`, { redirect: 'manual' })).status,
      200,
    );
    assert.equal(
      (
        await fetch(`${t.base}/browse?q=${encodeURIComponent('w3bs://prompt/w3bs/research@1')}`, {
          redirect: 'manual',
        })
      ).status,
      200,
    );

    const api = await fetch(`${t.base}/api/browse?q=chovy.hacker`);
    assert.equal(api.status, 200);
    const json = await api.json();
    assert.equal(json.kind, 'moshpit');
    assert.equal(json.primary, 'https://chovy.hacker/');
    assert.equal(json.fallback, 'https://chovy.hacker.pit.world/');
    const posted = await fetch(`${t.base}/api/browse`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ q: 'chovy.hacker' }),
    });
    assert.deepEqual(await posted.json(), json);
    const refused = await fetch(`${t.base}/api/browse?q=https%3A%2F%2F127.0.0.1%2F`);
    assert.equal(refused.status, 400);
    assert.equal((await refused.json()).error.code, 'INVALID_INPUT');
    const w3bs = await (
      await fetch(`${t.base}/api/browse?q=${encodeURIComponent('w3bs://prompt/w3bs/research@1')}`)
    ).json();
    assert.equal(w3bs.kind, 'w3bs');
    assert.equal(w3bs.resource.canonicalUri, 'w3bs://prompt/w3bs/research@1.0.0');

    const client = new Client({ name: 't', version: '0' });
    await client.connect(new StreamableHTTPClientTransport(new URL(`${t.base}/mcp`)));
    const tools = await client.listTools();
    assert.ok(tools.tools.some((tool) => tool.name === 'w3bs_browse'));
    const called = await client.callTool({ name: 'w3bs_browse', arguments: { q: 'chovy.hacker' } });
    assert.equal(called.structuredContent.fallback, 'https://chovy.hacker.pit.world/');
    await client.close();
  } finally {
    await t.close();
  }
});
