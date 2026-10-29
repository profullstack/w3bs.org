import express from 'express';
import { pageShell, escapeHtml as e } from './site.mjs';
import { W3bsError } from './core.mjs';

const MOSHPIT = 'https://pit.moshcode.sh';
const SEARCH = 'https://pit.world';
const FETCH_TIMEOUT = 12_000;
const NAME_LABEL = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;
const TLD_LABEL = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;
const safeName = (value) => {
  const parts = String(value ?? '').trim().toLowerCase().split('.');
  return parts.length === 2 && parts.every((part) => NAME_LABEL.test(part))
    ? `${parts[0]}.${parts[1]}`
    : null;
};
const safeTld = (value) => {
  const tld = String(value ?? '').trim().toLowerCase().replace(/^\./, '');
  return TLD_LABEL.test(tld) ? tld : null;
};
const publicTarget = (value) => {
  if (value == null || value === '') return null;
  if (typeof value !== 'string' || value.length > 2048) return undefined;
  try {
    const url = new URL(value);
    return url.protocol === 'https:' && url.hostname && !url.username && !url.password
      ? url.href
      : undefined;
  } catch {
    return undefined;
  }
};
const operators = () =>
  new Set(
    String(process.env.W3BS_NET_OPERATOR_USERNAMES ?? '')
      .split(',')
      .map((value) => value.trim().toLowerCase())
      .filter(Boolean),
  );
const nameHost = (name) => `https://${name}.w3bs.org`;
const page = (options) => pageShell({ track: false, ...options });
const hostedTlds = () => [...new Set(String(process.env.W3BS_NET_HOSTED_TLDS ?? 'whatever,agent,hacker').split(',').map((value) => safeTld(value)).filter(Boolean))];

function shell(title, description, active, content, robots = '') {
  return page({
    title,
    description,
    active,
    scripts: '',
    canonical: `https://w3bs.org${active}`,
    robots,
    content,
  });
}

function registryMarkup(names, { query = '', moshpitError = '' } = {}) {
  const local = names.filter((item) => item.source === 'w3bs');
  const imported = names.filter((item) => item.source !== 'w3bs');
  const rows = (items, remote = false) =>
    items.length
      ? `<div class="net-list">${items
          .map((item) => {
            const name = item.name || `${item.label}.${item.tld}`;
            const host = nameHost(name);
            const origin = remote ? `Backfilled from ${item.source}` : 'W3BS primary registry';
            const target = item.target
              ? `<a class="inline-link" href="${e(item.target)}" rel="noopener noreferrer">Open target ↗</a>`
              : item.source === MOSHPIT
                ? `<a class="inline-link" href="https://${e(name)}.pit.world" rel="noopener noreferrer">Open on pit.world ↗</a>`
                : remote
                  ? `<a class="inline-link" href="${e(item.source)}/api/net/v1/resolve/${encodeURIComponent(name)}" rel="noopener noreferrer">View peer record ↗</a>`
                  : '<span class="net-row-meta">No Web target yet</span>';
            return `<article class="net-row"><div><h3><a class="inline-link" href="${e(host)}/">${e(name)}</a></h3><p class="net-row-meta">${e(origin)} · ${item.claimed ? 'claimed' : 'operator seed'} · ${e(item.status || 'active')}</p></div><div>${target}</div></article>`;
          })
          .join('')}</div>`
      : '<p class="muted">No names match this search.</p>';
  return `<section class="page-head net-intro"><div class="eyebrow">W3BS NET · OPEN DOMAIN FEDERATION</div><h1>Your namespaces.<br><em>Your network.</em></h1><p>W3BS Net is an independent registry and event relay. Names registered here are primary on W3BS; opted-in peers receive announcements. Public Moshpit records are imported as backfill and stay attributed to their source.</p><form class="search-bar" action="/search" method="get" role="search"><label class="sr-only" for="net-q">Search W3BS Net</label><input id="net-q" name="q" type="search" value="${e(query)}" placeholder="Search names and namespaces…" maxlength="200"><button class="button primary" type="submit">Search →</button></form><div class="actions"><a class="button primary" href="/net/register">Register a namespace or name</a><a class="button" href="/net/operator">Operator console</a><a class="text-link" href="/api/net/v1/registry">Registry API ↗</a><a class="text-link" href="/api/net/v1/events">Event relay ↗</a></div></section><section class="section"><div class="section-heading"><div><div class="eyebrow">LOCAL · PRIMARY</div><h2>W3BS registrations</h2></div><p>Requests are held for moderation. Active namespaces and names are free, never sold, and never reserved for pre-buy.</p></div>${rows(local)}</section><section class="section"><div class="section-heading"><div><div class="eyebrow">FEDERATION · BACKFILL</div><h2>Imported names</h2></div><p>Moshpit records fill gaps in discovery. W3BS registrations always take precedence when a name exists in both registries.</p></div>${moshpitError ? `<p class="net-meta">${e(moshpitError)}</p>` : ''}${rows(imported, true)}</section><aside class="callout net-no-sales"><strong>No pre-buys, sales or paid reservations.</strong><p>Register a namespace or name through your W3BS account. Operators review registrations before announcing them to federation peers.</p><a class="inline-link" href="/net/operator">Open the operator console →</a></aside>`;
}

function searchMarkup(query, local, backfill, webResults, error) {
  const links = (items) => items.length
    ? `<div class="net-list">${items
        .map((item) => {
          const name = item.name || `${item.label}.${item.tld}`;
          const target = item.source === 'w3bs'
            ? nameHost(name)
            : item.source === MOSHPIT
              ? `https://${name}.pit.world`
              : `${item.source}/api/net/v1/resolve/${encodeURIComponent(name)}`;
          const href = item.source && item.source !== 'w3bs' && item.source !== MOSHPIT
            ? target
            : `${target}/`;
          return `<article class="net-row"><div><h3><a class="inline-link" href="${e(href)}">${e(name)}</a></h3><p class="net-row-meta">${item.source === 'w3bs' ? 'W3BS primary' : `Backfill · ${e(item.source)}`}</p></div></article>`;
        })
        .join('')}</div>`
    : '<p class="muted">No matching network names.</p>';
  const web = Array.isArray(webResults) && webResults.length
    ? `<div class="net-list">${webResults.slice(0, 10).map((item) => {
        let url;
        try { url = new URL(item.url || item.link); } catch { return ''; }
        if (url.protocol !== 'https:' && url.protocol !== 'http:') return '';
        return `<article class="net-row"><div><h3><a class="inline-link" href="${e(url.href)}" rel="noopener noreferrer">${e(item.title || url.hostname)}</a></h3><p>${e(item.description || item.snippet || '')}</p><p class="net-row-meta">${e(url.hostname)}</p></div></article>`;
      }).join('')}</div>`
    : '<p class="muted">No Web results were returned.</p>';
  return `<section class="page-head compact"><div class="eyebrow">W3BS NET · SEARCH</div><h1>Search the network.</h1><form class="search-bar" action="/search" method="get" role="search"><label class="sr-only" for="net-search">Search W3BS Net and the Web</label><input id="net-search" name="q" type="search" value="${e(query)}" placeholder="Name, namespace or Web query…" maxlength="200" required><button class="button primary" type="submit">Search →</button></form><p>W3BS primary registry first, Moshpit backfill second, and public Web results last.</p></section>${query ? `<section class="net-search-results"><h2>W3BS Net · local first</h2>${links(local)}</section><section class="net-search-results"><h2>Moshpit and peer backfill</h2>${error ? `<p class="net-meta">${e(error)}</p>` : ''}${links(backfill)}</section><section class="net-search-results"><h2>Public Web results</h2>${web}</section>` : '<section class="net-block"><p>Search local registrations and the Moshpit backfill. The query is sent to pit.world only for public Web results.</p></section>'}`;
}

function operatorMarkup(username) {
  return `<section class="page-head compact"><div class="eyebrow">W3BS NET · OPERATOR</div><h1>Network control room.</h1><p>Signed in as <strong>@${e(username)}</strong>. Review every local registration, keep no-sale policy on, and manage opted-in federation peers.</p><div class="actions"><a class="button" href="/net">Back to W3BS Net</a><a class="text-link" href="/api/net/v1/operator/overview">Operator API ↗</a></div></section><div class="net-console"><section class="net-block"><h2>Moderation queue</h2><p class="net-help">New namespace and name requests stay private until approved. Approvals publish an immutable event to W3BS Net and enqueue it for every active federation peer.</p><div id="net-queue" class="net-queue"><p class="net-status">Loading queue…</p></div></section><section class="net-block"><h2>Active registrations</h2><p class="net-help">Suspend a local namespace or name to stop resolving it and announce the change to peers. Suspended entries can be restored here.</p><div id="net-managed" class="net-queue"><p class="net-status">Loading registrations…</p></div></section><section class="net-block"><h2>Open a namespace</h2><form id="net-tld-form"><label for="net-tld">Namespace label</label><input id="net-tld" name="tld" placeholder="example" maxlength="63" required><label><input type="checkbox" name="open" value="1"> Let any signed-in W3BS account request names under this namespace</label><button class="button primary" type="submit">Submit for moderation</button><p class="net-status" data-status></p></form></section><section class="net-block"><h2>Register a name</h2><form id="net-name-form"><label for="net-name">Name</label><input id="net-name" name="name" placeholder="myname.agent" required maxlength="127"><label for="net-target">Optional HTTPS destination</label><input id="net-target" name="target" type="url" placeholder="https://example.org"><button class="button primary" type="submit">Submit for moderation</button><p class="net-status" data-status></p></form></section><section class="net-block"><h2>Federation peers</h2><p class="net-help">Each W3BS peer is opted in by an operator. Exchange the one-time inbound token shown after adding a peer; that token authenticates delivery from that peer to this instance. Peer event streams relay public announcements across connected instances. Moshpit remains the read-only backfill source.</p><h3>Known network instances</h3><div id="net-instances" class="net-list"><p class="net-status">Loading instances…</p></div><form id="net-peer-form"><label for="net-peer-url">Peer base URL</label><input id="net-peer-url" name="endpoint" type="url" placeholder="https://net.example.org" required><label for="net-peer-token">Peer’s inbound token</label><input id="net-peer-token" name="outboundToken" autocomplete="off" required><button class="button primary" type="submit">Opt in to peer</button><p class="net-status" data-status></p><div id="net-peer-secret" hidden></div></form><div id="net-peers" class="net-list"><p class="net-status">Loading peers…</p></div></section><section class="net-block"><h2>Moshpit backfill</h2><p class="net-help">The daemon imports public namespaces, names and allocation history from pit.moshcode.sh. Imported records remain attributed as backfill and do not become W3BS-owned registrations.</p><div id="net-backfill" class="net-status">Loading backfill status…</div><button id="net-backfill-now" class="button" type="button">Run backfill now</button></section><p class="net-help">Automation uses the existing W3BS OAuth 2.1 authorization server and <code>w3bs login</code> device flow. The CLI bearer can manage namespaces, names and federation peers only when its W3BS account is in <code>W3BS_NET_OPERATOR_USERNAMES</code>.</p></div><script type="module" src="/assets/net-operator.js"></script>`;
}

export function createNet({
  store,
  origin = 'https://w3bs.org',
  fetchImpl = fetch,
  daemonEnabled = process.env.NODE_ENV === 'production' && process.env.W3BS_NET_SYNC !== 'off',
} = {}) {
  if (!store?.network) throw new Error('W3BS Net requires the independent network registry store.');
  const network = store.network;
  const root = new URL(origin).origin;
  let daemonRunning = false;
  let stopped = false;
  let daemonTimer;
  let helpers;

  const loadBackfill = async (tld) => {
    const url = new URL(`/api/moshpit/tlds/${encodeURIComponent(tld)}/names`, MOSHPIT);
    const response = await fetchImpl(url, {
      headers: { accept: 'application/json', 'user-agent': 'w3bs-net/2.0 (+https://w3bs.org/net)' },
      redirect: 'error',
      signal: AbortSignal.timeout(FETCH_TIMEOUT),
    });
    if (!response.ok) throw new Error(`Moshpit returned ${response.status}`);
    const data = await response.json();
    for (const item of Array.isArray(data?.names) ? data.names : []) {
      const label = String(item?.label ?? '').toLowerCase();
      if (!NAME_LABEL.test(label)) continue;
      const target = publicTarget(item?.target);
      await network.importRecord(MOSHPIT, 'moshpit-name', `${label}.${tld}`, {
        name: `${label}.${tld}`,
        tld,
        label,
        target: target === undefined ? null : target,
        createdAt: item?.created_at ?? null,
      });
    }
  };

  const backfillChunk = async () => {
    const lastRun = Number(await network.getImportState(MOSHPIT, 'full-backfill-next', '0'));
    if (Date.now() < lastRun) return;
    let offset = Number(await network.getImportState(MOSHPIT, 'full-backfill-offset', '0')) || 0;
    const url = new URL('/api/moshpit/tlds', MOSHPIT);
    url.searchParams.set('limit', '100');
    url.searchParams.set('offset', String(offset));
    const response = await fetchImpl(url, {
      headers: { accept: 'application/json', 'user-agent': 'w3bs-net/2.0 (+https://w3bs.org/net)' },
      redirect: 'error',
      signal: AbortSignal.timeout(FETCH_TIMEOUT),
    });
    if (!response.ok) throw new Error(`Moshpit TLD listing returned ${response.status}`);
    const data = await response.json();
    const tlds = Array.isArray(data?.tlds) ? data.tlds : [];
    const total = Math.max(0, Number(data?.total) || tlds.length);
    const batch = tlds.slice(0, 100);
    let cursor = 0;
    const workers = Array.from({ length: 6 }, async () => {
      while (cursor < batch.length) {
        const item = batch[cursor++];
        const tld = safeTld(item?.tld);
        if (!tld) continue;
        await network.importRecord(MOSHPIT, 'moshpit-tld', tld, {
          tld,
          owner: item?.owner ?? null,
          aliasOf: item?.alias_of ?? null,
          createdAt: item?.created_at ?? null,
        });
        try {
          await loadBackfill(tld);
        } catch (error) {
        await network.importState(MOSHPIT, `name-error:${tld}`, error.message);
        }
      }
    });
    await Promise.all(workers);
    offset += batch.length;
    if (!tlds.length || offset >= total) {
      await network.importState(MOSHPIT, 'full-backfill-offset', '0');
      await network.importState(MOSHPIT, 'full-backfill-next', String(Date.now() + 6 * 60 * 60 * 1000));
      await network.importState(MOSHPIT, 'full-backfill-completed', new Date().toISOString());
    } else {
      await network.importState(MOSHPIT, 'full-backfill-offset', String(offset));
      await network.importState(MOSHPIT, 'full-backfill-next', String(Date.now() + 30_000));
    }
    await network.importState(MOSHPIT, 'full-backfill-total', String(total));
  };

  const incrementalBackfill = async () => {
    let cursor = Number(await network.getImportState(MOSHPIT, 'log-cursor', '0')) || 0;
    const url = new URL('/api/moshpit/log', MOSHPIT);
    url.searchParams.set('since', String(cursor));
    url.searchParams.set('limit', '500');
    const response = await fetchImpl(url, {
      headers: { accept: 'application/json', 'user-agent': 'w3bs-net/2.0 (+https://w3bs.org/net)' },
      redirect: 'error',
      signal: AbortSignal.timeout(FETCH_TIMEOUT),
    });
    if (!response.ok) throw new Error(`Moshpit allocation log returned ${response.status}`);
    const data = await response.json();
    const entries = Array.isArray(data?.entries) ? data.entries : [];
    const changedTlds = new Set();
    for (const entry of entries) {
      const tld = safeTld(entry.tld);
      if (!tld) continue;
      await network.importRecord(MOSHPIT, 'moshpit-event', String(entry.seq), {
        sequence: Number(entry.seq) || 0,
        tld,
        action: String(entry.action ?? ''),
        owner: typeof entry.owner === 'string' ? entry.owner : null,
        at: entry.at ?? null,
      });
      await network.importRecord(MOSHPIT, 'moshpit-tld', tld, {
        tld,
        owner: entry.owner ?? null,
        action: entry.action,
        createdAt: entry.at ?? null,
      });
      changedTlds.add(tld);
      cursor = Number(entry.seq) || cursor;
    }
    for (const tld of changedTlds) {
      try { await loadBackfill(tld); } catch { /* Full snapshots retry failed names later. */ }
    }
    await network.importState(MOSHPIT, 'log-cursor', String(cursor));
  };

  const flushOutbox = async () => {
    const rows = await network.dueOutbox(50);
    for (const row of rows) {
      const endpoint = `${row.endpoint.replace(/\/$/, '')}/api/net/v1/federation/events`;
      const event = {
        id: row.event_id,
        sequence: Number(row.seq),
        type: row.type,
        subject: row.subject,
        data: JSON.parse(row.payload),
        actor: row.actor_id ?? null,
        instance: row.instance,
        createdAt: row.created_at,
      };
      try {
        const response = await fetchImpl(endpoint, {
          method: 'POST',
          headers: {
            authorization: `Bearer ${row.outbound_token}`,
            'content-type': 'application/json',
            accept: 'application/json',
            'user-agent': 'w3bs-net/2.0 (+https://w3bs.org/net)',
          },
          body: JSON.stringify({ instance: root, events: [event] }),
          redirect: 'error',
          signal: AbortSignal.timeout(FETCH_TIMEOUT),
        });
        if (!response.ok) throw new Error(`peer returned ${response.status}`);
        await network.markDelivered(row.outbox_id);
        await network.markPeerSeen(row.peer_id, row.seq);
      } catch (error) {
        await network.markDeliveryFailed(row, error.message);
      }
    }
  };

  const pullPeerEvents = async () => {
    const peers = await network.peersForWorker();
    for (const peer of peers) {
      const cursor = Number(peer.inbound_cursor) || 0;
      const url = new URL('/api/net/v1/events', peer.endpoint);
      url.searchParams.set('since', String(cursor));
      url.searchParams.set('limit', '100');
      try {
        const response = await fetchImpl(url, {
          headers: { accept: 'application/json', 'user-agent': 'w3bs-net/2.0 (+https://w3bs.org/net)' },
          redirect: 'error',
          signal: AbortSignal.timeout(FETCH_TIMEOUT),
        });
        if (!response.ok) throw new Error(`peer returned ${response.status}`);
        const data = await response.json();
        if (new URL(data?.instance ?? '').origin !== peer.endpoint) throw new Error('peer instance identity mismatch');
        const result = await network.receiveEvents(peer, Array.isArray(data?.events) ? data.events : []);
        await network.markPeerReceived(peer.id, result.cursor);
      } catch (error) {
        await network.markPeerSeen(peer.id, peer.cursor, error.message);
        process.stderr.write(`W3BS Net peer backfill ${peer.endpoint}: ${error.message}\n`);
      }
    }
  };

  const runDaemon = async () => {
    if (daemonRunning || stopped) return;
    daemonRunning = true;
    let backfillFailure = '';
    try {
      for (const [name, run] of [
        ['Moshpit log backfill', incrementalBackfill],
        ['Moshpit registry backfill', backfillChunk],
        ['peer event backfill', pullPeerEvents],
        ['federation outbox', flushOutbox],
      ]) {
        try {
          await run();
        } catch (error) {
          if (name.startsWith('Moshpit')) backfillFailure = `${name}: ${error.message}`;
          process.stderr.write(`W3BS Net ${name}: ${error.message}\n`);
        }
      }
      await network.importState(MOSHPIT, 'last-error', backfillFailure);
    } finally {
      daemonRunning = false;
    }
  };
  if (daemonEnabled) {
    daemonTimer = setInterval(() => void runDaemon(), 30_000);
    daemonTimer.unref?.();
    setTimeout(() => void runDaemon(), 2_000).unref?.();
  }

  async function authorized(req) {
    const user = await helpers?.accountUser(req);
    if (!user) return { user: null, operator: false };
    return { user, operator: operators().has(String(user.username ?? '').toLowerCase()) };
  }

  async function requireAccount(req) {
    const { user } = await authorized(req);
    if (!user) throw new W3bsError('SIGNED_OUT', 'Sign in with a W3BS account or run `w3bs login`.', 401);
    return user;
  }

  async function requireOperator(req) {
    const { user, operator } = await authorized(req);
    if (!user) throw new W3bsError('SIGNED_OUT', 'Sign in with the W3BS account authorized for Net operations.', 401);
    if (!operators().size)
      throw new W3bsError('NET_OPERATOR_UNCONFIGURED', 'Set W3BS_NET_OPERATOR_USERNAMES before enabling operator actions.', 503);
    if (!operator) throw new W3bsError('NET_OPERATOR_REQUIRED', 'This W3BS account is not a Net operator.', 403);
    return user;
  }

  async function showPublicNet(req, res) {
    const data = await network.registry({ query: req.query.q ?? '', limit: 200 });
    const error = await network.getImportState(MOSHPIT, 'last-error', '');
    return res.type('html').send(shell(
      'W3BS Net',
      'An independent, moderated namespace registry and federated event relay.',
      '/net',
      registryMarkup(data.names, { query: req.query.q ?? '', moshpitError: error }),
    ));
  }

  async function showSearch(req, res) {
    const query = String(req.query.q ?? '').trim().slice(0, 200);
    const net = query ? await network.registry({ query, limit: 40 }) : { names: [] };
    const local = net.names.filter((item) => item.source === 'w3bs');
    const backfill = net.names.filter((item) => item.source !== 'w3bs');
    let web = [];
    let error = '';
    if (query) {
      try {
        const url = new URL('/api/search', SEARCH);
        url.searchParams.set('q', query);
        url.searchParams.set('n', '10');
        const response = await fetchImpl(url, {
          headers: { accept: 'application/json', 'user-agent': 'w3bs-net/2.0 (+https://w3bs.org/search)' },
          redirect: 'error',
          signal: AbortSignal.timeout(FETCH_TIMEOUT),
        });
        const data = await response.json();
        if (response.ok) web = Array.isArray(data?.results) ? data.results : Array.isArray(data) ? data : [];
        else error = 'The public Web search service did not return results.';
      } catch {
        error = 'The public Web search service could not be reached.';
      }
    }
    return res.type('html').send(shell(
      'Search W3BS Net',
      'Search the W3BS primary registry, Moshpit backfill and public Web.',
      '/search',
      searchMarkup(query, local, backfill, web, error),
    ));
  }

  function nameFromHost(hostname) {
    const host = String(hostname ?? '').toLowerCase().replace(/:\d+$/, '').replace(/\.$/, '');
    if (!host.endsWith('.w3bs.org')) return null;
    const name = safeName(host.slice(0, -'.w3bs.org'.length));
    return name;
  }

  async function gateway(req, res, name) {
    const record = await network.resolveName(name);
    if (!record) {
      return res.status(404).type('html').send(shell(
        'Name not found',
        `${name} is not registered on W3BS Net.`,
        '/net',
        `<section class="page-head compact"><div class="eyebrow">W3BS NET · ${e(name)}</div><h1>Name not found.</h1><p>This hostname is not in the active W3BS registry or the imported Moshpit backfill.</p><a class="button" href="/net">Browse W3BS Net</a></section>`,
      ));
    }
    if (record.source === MOSHPIT) {
      res.set('X-W3BS-Net-Source', 'moshpit-backfill');
      return res.redirect(302, `https://${name}.pit.world`);
    }
    if (record.source === 'w3bs' && record.status !== 'active') {
      return res.status(404).type('html').send(shell(
        'Name unavailable',
        `${name} is not currently active in W3BS Net.`,
        '/net',
        `<section class="page-head compact"><div class="eyebrow">W3BS NET · ${e(record.status)}</div><h1>${e(name)}</h1><p>This local W3BS record is not active, so the federation backfill is not used for it.</p><a class="button" href="https://w3bs.org/net">W3BS Net</a></section>`,
      ));
    }
    if (record.target) return res.redirect(302, record.target);
    res.set('X-W3BS-Net-Source', record.source === 'w3bs' ? 'w3bs-primary' : 'w3bs-federation');
    return res.type('html').send(shell(
      name,
      `${name} is an active W3BS Net name.`,
      '/net',
      `<section class="page-head compact"><div class="eyebrow">W3BS NET · ${record.source === 'w3bs' ? 'PRIMARY' : 'FEDERATED'} NAME</div><h1>${e(name)}</h1><p>${record.source === 'w3bs' ? 'This name is registered in the W3BS primary registry.' : `This name was announced by the opted-in federation peer ${e(record.source)}.`} Its owner has not published a Web destination yet.</p><p class="net-meta">${record.claimed ? 'Claimed by an account' : 'Operator-seeded'} · Active after moderation</p><div class="actions"><a class="button" href="https://w3bs.org/net">W3BS Net</a><a class="text-link" href="https://w3bs.org/api/net/v1/resolve/${encodeURIComponent(name)}">Registry record ↗</a></div></section>`,
    ));
  }

  function mount(app, options = {}) {
    helpers = options.helpers;
    app.get('/.well-known/w3bs-net.json', (_req, res) => res.json({
      specVersion: 'W3BS-NET-1-draft-1',
      instance: root,
      primaryRegistry: `${root}/api/net/v1/registry`,
      resolution: `${root}/api/net/v1/resolve/{name}`,
      events: `${root}/api/net/v1/events`,
      instances: `${root}/api/net/v1/instances`,
      federationInbox: `${root}/api/net/v1/federation/events`,
      operator: `${root}/net/operator`,
      authentication: `${root}/.well-known/oauth-authorization-server`,
      federation: 'Opt-in peer delivery and onward relay of public, moderated registration events and instance announcements.',
      backfill: `${MOSHPIT}/api/moshpit/log`,
    }));

    app.get('/api/net/v1/registry', async (req, res) => {
      res.set('Access-Control-Allow-Origin', '*');
      const registry = await network.registry({ query: req.query.q ?? '', limit: req.query.limit });
      res.json(registry);
    });
    app.get('/api/net/v1/resolve/:name', async (req, res) => {
      res.set('Access-Control-Allow-Origin', '*');
      const name = safeName(req.params.name);
      if (!name) return res.status(400).json({ error: 'invalid_name' });
      const record = await network.resolveName(name);
      if (!record) return res.status(404).json({ error: 'not_found', name });
      res.json({ ...record, primary: record.source === 'w3bs', url: `${root}/api/net/v1/resolve/${encodeURIComponent(name)}` });
    });
    app.get('/api/net/v1/events', async (req, res) => {
      res.set('Access-Control-Allow-Origin', '*');
      const since = Math.max(0, Number.parseInt(req.query.since, 10) || 0);
      const limit = Math.min(500, Math.max(1, Number.parseInt(req.query.limit, 10) || 100));
      const events = await network.events({ since, limit });
      const cursor = events.length ? events.at(-1).sequence : since;
      res.json({ instance: root, events, next: events.length === limit ? cursor : null });
    });
    app.get('/api/net/v1/instances', async (_req, res) => {
      res.set('Access-Control-Allow-Origin', '*');
      const peers = (await network.peerList())
        .filter((peer) => peer.kind === 'w3bs' && peer.status === 'active')
        .map((peer) => ({ instance: peer.endpoint, lastSeenAt: peer.lastSeenAt }));
      const known = await network.knownInstances();
      res.json({
        local: { instance: root, wellKnown: `${root}/.well-known/w3bs-net.json`, registry: `${root}/api/net/v1/registry` },
        optedIn: peers,
        known,
      });
    });
    app.options('/api/net/v1/federation/events', (_req, res) => res.status(204).end());
    app.post('/api/net/v1/federation/events', async (req, res) => {
      const token = /^Bearer ([A-Za-z0-9-]{20,100})$/.exec(req.headers.authorization ?? '')?.[1];
      const peer = await network.incomingPeer(token);
      if (!peer) throw new W3bsError('FEDERATION_AUTH_REQUIRED', 'Unknown federation peer token.', 401);
      try {
        if (typeof req.body?.instance !== 'string' || new URL(req.body.instance).origin !== peer.endpoint)
          throw new W3bsError('FEDERATION_INSTANCE_MISMATCH', 'Event source does not match the configured peer.', 403);
      } catch (error) {
        if (error instanceof W3bsError) throw error;
        throw new W3bsError('FEDERATION_INSTANCE_MISMATCH', 'Event source must be a valid HTTPS origin.', 403);
      }
      const batch = Array.isArray(req.body?.events) ? req.body.events.slice(0, 100) : [];
      const result = await network.receiveEvents(peer, batch);
      await network.markPeerActivity(peer.id);
      res.status(202).json({ accepted: result.accepted, source: peer.endpoint, storedAs: 'federated announcements' });
    });

    app.post('/api/net/v1/tlds', async (req, res) => {
      const user = await requireAccount(req);
      const tld = safeTld(req.body?.tld);
      if (!tld) throw new W3bsError('INVALID_TLD', 'Namespace labels use letters, digits and hyphens.', 400);
      const reserved = new Set(['w3bs', 'www', 'api', 'net', 'search', 'admin', 'root']);
      if (reserved.has(tld)) throw new W3bsError('RESERVED_TLD', 'That namespace label is reserved.', 409);
      const result = await network.requestTld({ tld, ownerId: user.id, policy: { open: Boolean(req.body?.open) } });
      if (!result.created) throw new W3bsError('TLD_TAKEN', 'That namespace is already registered or pending.', 409);
      res.status(202).json({ tld, status: 'pending_moderation', noSales: true });
    });
    app.post('/api/net/v1/names', async (req, res) => {
      const user = await requireAccount(req);
      const name = safeName(req.body?.name);
      if (!name) throw new W3bsError('INVALID_NAME', 'Names must be label.namespace.', 400);
      const target = publicTarget(req.body?.target);
      if (target === undefined) throw new W3bsError('INVALID_TARGET', 'A destination must be a public HTTPS URL.', 400);
      const [label, tld] = name.split('.');
      const result = await network.requestName({ tld, label, ownerId: user.id, ownerName: user.username, target });
      if (result.error === 'namespace_unavailable') throw new W3bsError('NAMESPACE_UNAVAILABLE', 'The W3BS namespace is not active.', 409);
      if (result.error === 'namespace_owner_required') throw new W3bsError('NAMESPACE_OWNER_REQUIRED', 'The namespace owner does not allow open registration.', 403);
      if (result.error === 'seed_owner_required') throw new W3bsError('SEEDED_NAME_OWNER_REQUIRED', 'Only the matching W3BS account can claim this operator-seeded name.', 403);
      if (!result.created) throw new W3bsError('NAME_TAKEN', 'That name is already registered or pending.', 409);
      res.status(202).json({ name, status: 'pending_moderation' });
    });

    app.get('/api/net/v1/operator/overview', async (req, res) => {
      await requireOperator(req);
      const [queue, peers, backfill, instances] = await Promise.all([
        network.pending(),
        network.peerList(),
        network.backfillSummary(),
        network.knownInstances(),
      ]);
      const managed = await network.managed();
      res.json({ instance: root, queue, managed, peers, instances, backfill, hostedTlds: hostedTlds(), noSales: true, operatorUsernamesConfigured: operators().size });
    });
    app.post('/api/net/v1/operator/moderate', async (req, res) => {
      const user = await requireOperator(req);
      if (!['approve', 'reject', 'suspend', 'restore'].includes(String(req.body?.action ?? '')))
        throw new W3bsError('INVALID_MODERATION_ACTION', 'Action must be approve, reject, suspend or restore.', 400);
      const result = await network.moderate({
        kind: String(req.body?.kind ?? ''),
        subject: String(req.body?.subject ?? ''),
        action: String(req.body?.action ?? ''),
        actor: user.id,
        note: String(req.body?.note ?? ''),
      });
      if (result.error) throw new W3bsError('MODERATION_FAILED', result.error, result.error === 'not_found' ? 404 : 400);
      res.json(result);
    });
    app.post('/api/net/v1/operator/peers', async (req, res) => {
      await requireOperator(req);
      let endpoint;
      try {
        endpoint = new URL(String(req.body?.endpoint ?? '')).origin;
      } catch {
        throw new W3bsError('INVALID_PEER', 'Enter a peer HTTPS origin.', 400);
      }
      const peerToken = String(req.body?.outboundToken ?? '');
      if (!endpoint.startsWith('https://') || endpoint === root || !/^[A-Za-z0-9_-]{20,200}$/.test(peerToken))
        throw new W3bsError('INVALID_PEER', 'Peers must use a different HTTPS origin and a valid inbound token.', 400);
      const peer = await network.addPeer({ endpoint, outboundToken: peerToken });
      res.status(201).json({ ...peer, endpoint, note: 'Give inboundToken to the peer operator so they can deliver events to this instance.' });
    });
    app.delete('/api/net/v1/operator/peers/:id', async (req, res) => {
      await requireOperator(req);
      const peers = await network.peerList();
      const peer = peers.find((item) => item.id === req.params.id);
      if (!peer) throw new W3bsError('PEER_NOT_FOUND', 'Peer not found.', 404);
      await network.disablePeer(req.params.id);
      res.json({ id: req.params.id, status: 'disabled' });
    });
    app.post('/api/net/v1/operator/backfill', async (req, res) => {
      await requireOperator(req);
      await network.importState(MOSHPIT, 'full-backfill-next', '0');
      await network.importState(MOSHPIT, 'full-backfill-offset', '0');
      void runDaemon();
      res.status(202).json({ status: 'started', source: MOSHPIT });
    });

    app.get('/net', showPublicNet);
    app.get('/search', showSearch);
    app.get('/net/register', async (req, res) => {
      const user = await helpers.currentUser(req);
      if (!user) return res.redirect(302, '/login?next=%2Fnet%2Fregister');
      res.type('html').send(shell(
        'Register on W3BS Net',
        'Request a W3BS namespace or name. Registrations are free and moderated.',
        '/net',
        `<section class="page-head compact"><div class="eyebrow">W3BS NET · REGISTRATION</div><h1>Claim a name.<br><em>Join the federation.</em></h1><p>Signed in as <strong>@${e(user.username)}</strong>. Namespace and name requests are free. Each request waits for moderation before becoming active or reaching peers.</p></section><div class="net-console"><section class="net-block"><h2>Request a namespace</h2><p class="net-help">A namespace lets you register names beneath it after approval. It has no price and cannot be pre-bought.</p><form id="net-register-tld"><label for="register-tld">Namespace label</label><input id="register-tld" name="tld" maxlength="63" required placeholder="example"><label><input name="open" type="checkbox" value="1"> Let any signed-in W3BS account request names under it</label><button class="button primary" type="submit">Request namespace</button><p class="net-status" data-status></p></form></section><section class="net-block"><h2>Request a name</h2><p class="net-help">Names can be requested under an active open namespace, or under a namespace you own.</p><form id="net-register-name"><label for="register-name">Name</label><input id="register-name" name="name" maxlength="127" required placeholder="riotcoder.agent"><label for="register-target">Optional HTTPS destination</label><input id="register-target" name="target" type="url" placeholder="https://example.org"><button class="button primary" type="submit">Request name</button><p class="net-status" data-status></p></form></section></div><script type="module" src="/assets/net-register.js"></script>`,
        'noindex',
      ));
    });
    app.get('/net/operator', async (req, res) => {
      const { user, operator } = await authorized(req);
      if (!user) return res.redirect(302, '/login?next=%2Fnet%2Foperator');
      if (!operators().size)
      return res.status(503).type('html').send(shell('Operator setup required', 'W3BS Net operator access is not configured.', '/net', '<section class="page-head compact"><h1>Operator setup required.</h1><p>Set <code>W3BS_NET_OPERATOR_USERNAMES</code> for the approved W3BS accounts before accessing the control room.</p><a class="button" href="/net">Back to W3BS Net</a></section>', 'noindex'));
      if (!operator) return res.status(403).type('html').send(shell('Operator access denied', 'This account is not a W3BS Net operator.', '/net', '<section class="page-head compact"><h1>No operator access.</h1><p>Your W3BS account is signed in, but it is not in the Net operator list.</p><a class="button" href="/net">Back to W3BS Net</a></section>', 'noindex'));
      return res.type('html').send(shell('W3BS Net Operator', 'Moderate W3BS names and manage federation peers.', '/net', operatorMarkup(user.username), 'noindex'));
    });
  }

  return {
    mount,
    nameFromHost,
    gateway,
    stop() {
      stopped = true;
      clearInterval(daemonTimer);
    },
  };
}
