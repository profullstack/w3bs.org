import test from 'node:test';
import assert from 'node:assert/strict';
import { startServer } from '../src/server.mjs';
import { openStore } from '../src/store.mjs';
import { CRAWLPROOF_SITE_ID, trackerSite } from '../src/site.mjs';
import { FLEET_LIMITS } from '../src/fleet.mjs';
import { osReleaseSource, resetOsReleaseCache } from '../src/os-releases.mjs';

const ORIGIN = 'http://localhost:3000';

async function boot() {
  const store = await openStore({ path: ':memory:' });
  const instance = await startServer({
    store,
    port: 0,
    publicOrigin: ORIGIN,
    mailer: { send: async () => ({ sent: true }) },
  });
  const base = `http://localhost:${instance.server.address().port}`;
  return { store, base, close: () => instance.close() };
}

test('the homepage leads with Fleet: the three commands, a sign-up button, the free note', async () => {
  const t = await boot();
  try {
    const html = await (await fetch(`${t.base}/`)).text();
    const hero = html.slice(html.indexOf('<main'), html.indexOf('hero-standards'));
    assert.match(hero, /<h1 id="fleet-title">Watch your servers <em>from one page.<\/em><\/h1>/);
    assert.match(hero, /Free\. Set up in 2 minutes\./);
    for (const command of [
      'curl -fsSL https://w3bs.org/install.sh | sh',
      'w3bs login',
      'w3bs fleet join &lt;token&gt;',
    ])
      assert.ok(hero.includes(command), command);
    assert.match(hero, /href="\/signup\?next=%2Ffleet"/);
    assert.match(hero, /href="\/fleet"/);
    assert.ok(hero.includes(`Up to ${FLEET_LIMITS.machines} machines`));
    // The standards story is still on the page, just after the offer.
    assert.match(html, /The open Web\.<br>For <em>all<\/em> of us\./);
    assert.equal((html.match(/<h1/g) || []).length, 1);
    // House style: no em or en dashes in the new copy (the title separator is site furniture).
    assert.ok(!/[–—]/.test(hero));
  } finally {
    await t.close();
  }
});

test('pricing, terms and privacy exist, state the limits from the code and carry no dashes', async () => {
  const t = await boot();
  try {
    for (const path of ['/pricing', '/free', '/terms', '/privacy']) {
      const response = await fetch(`${t.base}${path}`);
      assert.equal(response.status, 200, path);
      const html = await response.text();
      const main = html.slice(html.indexOf('<main'), html.indexOf('</main>'));
      assert.ok(!/[–—]/.test(main), `${path} has an em or en dash`);
      assert.match(main, /hello@w3bs\.org/, path);
    }
    const pricing = await (await fetch(`${t.base}/pricing`)).text();
    assert.match(pricing, /Fleet is <em>free\.<\/em>/);
    assert.match(pricing, new RegExp(`Machines per account</th><td>${FLEET_LIMITS.machines}<`));
    assert.match(pricing, /No uptime guarantee/);
    assert.match(pricing, /Latest report only/);
    assert.ok(!/\$\d/.test(pricing), 'no prices: there is no paid plan');
    const privacy = await (await fetch(`${t.base}/privacy`)).text();
    assert.match(privacy, /Plain-language beta version/);
    assert.match(privacy, /Profullstack, Inc\./);
    const sitemap = await (await fetch(`${t.base}/sitemap.xml`)).text();
    for (const path of ['/pricing', '/terms', '/privacy']) assert.ok(sitemap.includes(path));
  } finally {
    await t.close();
  }
});

test('every page has the support line in the footer', async () => {
  const t = await boot();
  try {
    for (const path of ['/mission', '/packages', '/specs', '/desktop', '/signup', '/login']) {
      const html = await (await fetch(`${t.base}${path}`)).text();
      const footer = html.slice(html.indexOf('<footer'));
      assert.match(footer, /mailto:hello@w3bs\.org/, path);
      assert.match(footer, /href="\/bbs\/f\/help"/, path);
    }
  } finally {
    await t.close();
  }
});

test('the footer links our socials and carries the Profullstack webring', async () => {
  const t = await boot();
  try {
    for (const path of ['/mission', '/login']) {
      const html = await (await fetch(`${t.base}${path}`)).text();
      const footer = html.slice(html.indexOf('<footer'));
      for (const href of [
        'https://www.reddit.com/r/w3bs/',
        'https://github.com/profullstack',
        'https://discord.gg/U7dEXfBA3s',
        'https://x.com/profullstackinc',
        'https://youtube.com/@profullstack',
      ]) {
        assert.ok(footer.includes(`href="${href}"`), `${path} ${href}`);
      }
      assert.match(footer, /rssamplifier\.com\/ring\/profullstack\/next/, path);
      // The CSP is style-src 'self': no inline <style> may ride in with it.
      assert.doesNotMatch(footer, /<style/, path);
    }
    const ring = await (await fetch(`${t.base}/.well-known/openwebring.json`)).json();
    assert.equal(ring.site.url, 'https://w3bs.org/');
  } finally {
    await t.close();
  }
});

test('CrawlProof: only for https://w3bs.org, never on pages that can carry account data', async () => {
  assert.equal(trackerSite({ PUBLIC_ORIGIN: 'https://w3bs.org' }), CRAWLPROOF_SITE_ID);
  assert.equal(trackerSite({ PUBLIC_ORIGIN: 'https://w3bs.org/' }), CRAWLPROOF_SITE_ID);
  assert.equal(trackerSite({ PUBLIC_ORIGIN: 'http://127.0.0.1:3000' }), '');
  assert.equal(trackerSite({}), '');
  assert.equal(trackerSite({ PUBLIC_ORIGIN: 'https://w3bs.org', W3BS_CRAWLPROOF_SITE: '' }), '');

  const saved = process.env.W3BS_CRAWLPROOF_SITE;
  process.env.W3BS_CRAWLPROOF_SITE = CRAWLPROOF_SITE_ID;
  const t = await boot();
  try {
    const tag = `<script async src="https://crawlproof.com/stats.js" data-site="${CRAWLPROOF_SITE_ID}" data-careers="off"></script>`;
    const home = await fetch(`${t.base}/`);
    assert.ok((await home.text()).includes(tag));
    const csp = home.headers.get('content-security-policy');
    assert.match(csp, /script-src 'self' https:\/\/crawlproof\.com;/);
    assert.match(csp, /connect-src 'self' https:\/\/crawlproof\.com;/);
    for (const path of ['/pricing', '/signup', '/login', '/login?next=%2Ffleet'])
      assert.ok((await (await fetch(`${t.base}${path}`)).text()).includes(tag), path);
    // Email links, device codes and OAuth requests travel in the address; machine names on
    // /fleet are button text. stats.js reports both, so these pages never load it.
    for (const path of [
      '/auth/link?token=abc',
      '/device?code=BCDF-GHJK',
      '/login?next=%2Fdevice%3Fcode%3DBCDFGHJK',
    ])
      assert.ok(!(await (await fetch(`${t.base}${path}`)).text()).includes('crawlproof'), path);
  } finally {
    await t.close();
    if (saved === undefined) delete process.env.W3BS_CRAWLPROOF_SITE;
    else process.env.W3BS_CRAWLPROOF_SITE = saved;
  }
});

test('/os offers only what exists: Docker now, the ISO from the newest os-v* release, no iOS install', async () => {
  const sha = 'e0c9a899516957dd53e79e6a6dc8bdb391ff7a49acce0ab0f4d64531e6e4991b';
  const dl = 'https://github.com/profullstack/w3bs.org/releases/download/os-v0.1.0-preview';
  let latest = null;
  osReleaseSource.fetchImpl = async () =>
    latest ? new Response(JSON.stringify(latest)) : new Response('Not Found', { status: 404 });
  resetOsReleaseCache();
  const t = await boot();
  try {
    const res = await fetch(`${t.base}/os`);
    assert.equal(res.status, 200);
    const html = await res.text();
    assert.match(html, /W3BS OS · SPECIFICATION/);
    assert.match(html, /resolves <strong>\d+<\/strong> Profullstack packages/);
    assert.match(html, /docker run -it --rm w3bs\.org\/os:server/);
    // No os-v* release yet: no ISO link.
    assert.match(html, /PCS AND MACS · COMING/);
    assert.doesNotMatch(html, /\.iso"/);
    assert.match(html, /nothing can replace iOS/);
    assert.match(html, /Debug desktop and phone apps in one place/);

    latest = {
      version: '0.1.0-preview',
      name: 'w3bs-os-0.1.0-x86_64.iso',
      url: `${dl}/w3bs-os-0.1.0-x86_64.iso`,
      sumsUrl: `${dl}/SHA256SUMS`,
      sha256: sha,
      size: 1459617792,
      preview: true,
    };
    resetOsReleaseCache();
    const live = await (await fetch(`${t.base}/os`)).text();
    assert.match(live, new RegExp(`href="${dl}/w3bs-os-0\\.1\\.0-x86_64\\.iso"`));
    assert.match(live, /Download the ISO \(1\.46 GB\)/);
    assert.match(live, new RegExp(sha));
    assert.match(live, /NETWORK INSTALLER 0\.1\.0-preview · PREVIEW/);
    const prd = await fetch(`${t.base}/docs/os-prd.md`);
    assert.equal(prd.status, 200);
    assert.match(await prd.text(), /^# W3BS OS Product Requirements/);
    assert.match(await (await fetch(`${t.base}/sitemap.xml`)).text(), /\/os<\/loc>/);
  } finally {
    resetOsReleaseCache();
    await t.close();
  }
});
