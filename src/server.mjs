import express from 'express';
import { createServer } from 'node:http';
import { readFileSync, existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { z } from 'zod';
import { StreamableHTTPServerTransport } from '@modelcontextprotocol/sdk/server/streamableHttp.js';
import { openStore, ROOT } from './store.mjs';
import { W3bsError, manifestJsonSchema } from './core.mjs';
import { dispatch, operations } from './operations.mjs';
import { createMcp } from './mcp.mjs';
import { renderPage, scanBadgeSvg, refreshHouseFooter, FOOTER_SCRIPT_SRC } from './site.mjs';
import { descriptor } from '@profullstack/footer';
import { createScanner } from './scan.mjs';
import { mountAccounts, OPEN_CORS, bearerOf } from './auth-routes.mjs';
import { mountFleet } from './fleet-routes.mjs';
import { mountDe } from './de-routes.mjs';
import { mountSocial, socialConfig } from './social-routes.mjs';
import { createMailer } from './mail.mjs';
import { createHostedTraining } from '../training/hosted.mjs';
import { mountTraining } from '../training/hosted-routes.mjs';
import { mountAbTest } from './abtest-routes.mjs';
import { mountTasks } from './tasks-routes.mjs';
import { mountPublishing } from './publishing-routes.mjs';
import { mountInstallLinks } from './install-links.mjs';
import { mountRegistryFront } from './registry-front.mjs';
import { createBrowser } from './browse.mjs';

export function createApp({
  store,
  publishToken = process.env.W3BS_PUBLISH_TOKEN,
  publicOrigin = process.env.PUBLIC_ORIGIN || 'http://localhost:3000',
  allowedHosts = (process.env.ALLOWED_HOSTS || 'localhost,127.0.0.1').split(','),
  mailer = createMailer(),
  formGuard,
  bridgeClients = process.env.W3BS_BRIDGE_CLIENTS
    ? JSON.parse(process.env.W3BS_BRIDGE_CLIENTS)
    : undefined,
  social = socialConfig(),
  socialFetch = fetch,
  // /browse and /api/browse: Moshpit names over DoH, pit.world gateways, discovery documents.
  browser = createBrowser(),
  // W3BS Training workspaces need PostgreSQL and a master key (W3BS_TRAINING_MASTER_KEY, from the
  // vault); without them /training/app and /api/v1/training answer "not enabled".
  training = {
    databaseUrl: process.env.DATABASE_URL,
    masterKey: process.env.W3BS_TRAINING_MASTER_KEY,
  },
} = {}) {
  if (!store) throw new Error('createApp requires an opened store; use startServer or openStore.');
  const app = express();
  const origin = new URL(publicOrigin).origin;
  const hosts = new Set(allowedHosts.map((host) => host.trim().toLowerCase()));
  hosts.add(new URL(origin).hostname);
  app.disable('x-powered-by');
  app.use((req, res, next) => {
    res.set({
      'X-Content-Type-Options': 'nosniff',
      'Referrer-Policy': 'strict-origin-when-cross-origin',
      'Cache-Control': 'no-store',
      'Content-Security-Policy':
        `default-src 'self'; script-src 'self' https://crawlproof.com ${FOOTER_SCRIPT_SRC}; style-src 'self'; img-src 'self' data: https://built.indie.pit.world https://pwamart.com; connect-src 'self' https://crawlproof.com https://pit.world; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self' https://pit.world`,
      'Permissions-Policy': 'camera=(), microphone=(), geolocation=()',
    });
    const railwayHealth =
      req.method === 'GET' && req.path === '/healthz' && req.hostname === 'healthcheck.railway.app';
    if (!hosts.has(req.hostname.toLowerCase()) && !railwayHealth)
      return next(new W3bsError('INVALID_HOST', 'Unrecognized service host.', 403));
    // OAuth token/revoke/metadata, and /api/me with a bearer token, are called by apps on
    // other origins. They read no cookies, so any origin may call them.
    if (
      OPEN_CORS.some((pattern) => pattern.test(req.path)) ||
      (req.path === '/api/me' && bearerOf(req))
    ) {
      res.set({
        'Access-Control-Allow-Origin': '*',
        'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
        'Access-Control-Allow-Headers': 'Authorization, Content-Type',
        'Access-Control-Max-Age': '600',
      });
      return req.method === 'OPTIONS' ? res.status(204).end() : next();
    }
    if (req.path === '/api/me' && req.method === 'OPTIONS')
      return res
        .status(204)
        .set({
          'Access-Control-Allow-Origin': '*',
          'Access-Control-Allow-Methods': 'GET, OPTIONS',
          'Access-Control-Allow-Headers': 'Authorization',
        })
        .end();
    if (req.headers.origin) {
      try {
        const caller = new URL(req.headers.origin);
        const samePort = caller.port === new URL(origin).port;
        if (
          !hosts.has(caller.hostname) ||
          caller.protocol !== new URL(origin).protocol ||
          !samePort
        )
          throw new Error('origin');
      } catch {
        return next(new W3bsError('INVALID_ORIGIN', 'Origin is not allowed.', 403));
      }
    }
    next();
  });
  app.use(express.json({ limit: '256kb', strict: true }));
  const token = (req) => /^Bearer (.+)$/.exec(req.headers.authorization || '')?.[1];
  const invoke = (req, name, args) =>
    dispatch(store, name, args, { token: token(req), publishToken });
  app.get('/healthz', async (_req, res) =>
    res.json({
      status: 'ok',
      version: '0.1.0',
      storage: store.dialect,
      resources: (await store.search()).length,
    }),
  );
  // The Profullstack webring's verifier reads this to count w3bs.org a member.
  app.get('/.well-known/openwebring.json', (_req, res) =>
    res
      .set('access-control-allow-origin', '*')
      .set('cache-control', 'public, max-age=3600')
      .json(descriptor({ site: 'https://w3bs.org/', name: 'W3BS' })),
  );
  app.get('/.well-known/w3bs.json', (_req, res) =>
    res.json({
      specVersion: 'W3BS-DISCOVERY-1-draft-1',
      status: 'Community Draft',
      registry: origin,
      resolution: `${origin}/api/resolve`,
      manifest: `${origin}/api/manifest`,
      search: `${origin}/api/search`,
      browse: `${origin}/api/browse`,
      mcp: `${origin}/mcp`,
      schemas: [`${origin}/schemas/manifest.json`],
      trust: `${origin}/api/trust`,
      accounts: {
        signup: `${origin}/signup`,
        oauth: `${origin}/.well-known/oauth-authorization-server`,
        userinfo: `${origin}/api/me`,
      },
      abTesting: {
        format: 'https://logicsrc.com/docs/openabtest',
        guide: `${origin}/docs/ab-testing.md`,
        assign: `${origin}/api/ab/assign`,
        exposure: `${origin}/api/ab/exposure`,
        conversion: `${origin}/api/ab/conversion`,
        experiments: `${origin}/api/ab/experiments`,
      },
      specs: `${origin}/specs`,
      surfaces: {
        web: `${origin}/mission`,
        browse: `${origin}/browse`,
        prompt: `${origin}/prompts`,
        cli: `${origin}/developers`,
        native: `${origin}/developers#native`,
      },
      operations: Object.keys(operations),
      trustNotice:
        'Discovery keys are informational. Pin publisher keys through a separate operator-approved channel.',
    }),
  );
  app.get('/api/trust', (_req, res) => res.json(store.trust));
  app.get('/schemas/manifest.json', (_req, res) => res.json(manifestJsonSchema));
  app.get('/api', (_req, res) =>
    res.json({
      version: '0.1.0',
      status: 'Community Draft',
      operations: Object.entries(operations).map(([name, value]) => ({
        name,
        endpoint: `/api/${name}`,
        method: 'POST',
        description: value.description,
        authentication: value.write ? 'Bearer publisher token' : 'none',
        input: z.toJSONSchema(value.schema),
      })),
    }),
  );
  app.get('/api/conformance-report', (_req, res) => {
    const path = resolve(ROOT, 'fixtures/conformance-report.json');
    if (!existsSync(path))
      throw new W3bsError(
        'NO_EVIDENCE',
        'No recorded conformance run is bundled with this build.',
        404,
      );
    res.type('json').send(readFileSync(path, 'utf8'));
  });
  app.get('/api/manifest', async (req, res) =>
    res.json((await store.inspect(req.query.uri)).manifest),
  );
  // What /browse shows, as JSON: a W3BS URI, a Moshpit name, a pit.world URL or an https URL.
  const browse = (q) => browser.resolve(q, { inspect: (uri) => store.inspect(uri) });
  app.get('/api/browse', async (req, res) => res.json(await browse(String(req.query.q ?? ''))));
  // POST { q } is what the stdio MCP server (src/mcp-stdio.mjs) calls, as for the operations.
  app.post('/api/browse', async (req, res) => res.json(await browse(String(req.body?.q ?? ''))));
  // w3bs://chovy.hacker or w3bs://example.com/path is an address, so /browse opens the site the way
  // typing it would: a claimed Moshpit name at its pit.world gateway (which every browser reaches),
  // anything else at https. Resource URIs (w3bs://prompt/...) and names that do not resolve fall
  // through to the page. Browsers land here when they search a typed w3bs:// address.
  app.get('/browse', async (req, res, next) => {
    const q = String(req.query.q ?? '').trim();
    if (!/^w3bs:\/\/[^/?#]*\./i.test(q)) return next();
    let target = null;
    try {
      const r = await browse(q);
      if (r.kind === 'moshpit') target = r.moshpit?.minted ? r.fallback : null;
      else if (r.kind === 'web') target = r.primary;
    } catch {
      // The page explains what went wrong.
    }
    if (!target) return next();
    res.set('cache-control', 'no-store');
    res.redirect(302, target);
  });
  app.get('/api/packages', async (req, res) => {
    const args = { query: String(req.query.q || '') };
    if (req.query.agentic === 'true' || req.query.agentic === 'false')
      args.agentic = req.query.agentic === 'true';
    if (req.query.publisher) args.publisher = String(req.query.publisher);
    res.json(await invoke(req, 'packages', args));
  });
  app.get('/api/packages/:publisher/:name', async (req, res) =>
    res.json(await invoke(req, 'package', { id: `${req.params.publisher}/${req.params.name}` })),
  );
  // A ThreatCrush scan badge publishers can embed in a README. Cached briefly, as
  // shields-style badges are.
  app.get('/api/packages/:publisher/:name/badge.svg', async (req, res) => {
    const item = await store.package(`${req.params.publisher}/${req.params.name}`);
    res
      .type('image/svg+xml')
      .set('Cache-Control', 'public, max-age=300')
      .send(scanBadgeSvg(item.scan));
  });
  // W3BS Package Manager: the bootstrap installer and the official signed repository.
  const registry = resolve(ROOT, 'registry');
  app.get('/install.sh', (_req, res) => {
    const path = resolve(registry, 'install.sh');
    if (!existsSync(path)) throw new W3bsError('NOT_FOUND', 'Installer not published yet.', 404);
    res
      .type('text/x-shellscript; charset=utf-8')
      .set('Cache-Control', 'public, max-age=300')
      .send(readFileSync(path));
  });
  const safeTarget = /^(?:[a-z0-9][a-z0-9._-]*\/)*[a-z0-9][a-z0-9._-]*$/i;
  app.get('/repo/metadata/:file', (req, res) => {
    if (!/^(?:[0-9]+\.)?(root|targets|snapshot|timestamp)\.json$/.test(req.params.file))
      throw new W3bsError('NOT_FOUND', 'No such metadata.', 404);
    const path = resolve(registry, 'repo/metadata', req.params.file);
    if (!existsSync(path)) throw new W3bsError('NOT_FOUND', 'No such metadata.', 404);
    res.type('application/json').set('Cache-Control', 'no-cache').send(readFileSync(path));
  });
  app.get(/^\/repo\/targets\/(.+)$/, (req, res) => {
    const target = req.params[0];
    if (!safeTarget.test(target) || target.split('/').some((part) => part === '..' || part === '.'))
      throw new W3bsError('NOT_FOUND', 'No such target.', 404);
    // Release binaries live on GitHub releases; TUF verifies length and hash either way.
    const artifact =
      /^artifacts\/w3bs\/([0-9][0-9A-Za-z.+-]*)\/(w3bs-[0-9A-Za-z._+-]+\.tar\.gz)$/.exec(target);
    if (artifact)
      return res.redirect(
        302,
        `https://github.com/profullstack/w3bs.org/releases/download/w3bs-v${artifact[1]}/${artifact[2]}`,
      );
    const path = resolve(registry, 'repo/targets', target);
    if (!path.startsWith(resolve(registry, 'repo/targets') + '/') || !existsSync(path))
      throw new W3bsError('NOT_FOUND', 'No such target.', 404);
    res.type('application/json').set('Cache-Control', 'no-cache').send(readFileSync(path));
  });
  for (const name of Object.keys(operations)) {
    app.post(`/api/${name}`, async (req, res) => res.json(await invoke(req, name, req.body)));
    if (['search', 'resolve', 'inspect', 'verify', 'conformance'].includes(name)) {
      app.get(`/api/${name}`, async (req, res) =>
        res.json(
          await invoke(
            req,
            name,
            name === 'search'
              ? { query: req.query.q || '', ...(req.query.type ? { type: req.query.type } : {}) }
              : { uri: req.query.uri },
          ),
        ),
      );
    }
  }
  app.post('/mcp', async (req, res, next) => {
    const server = createMcp((name, args) => invoke(req, name, args), { browse });
    const transport = new StreamableHTTPServerTransport({
      sessionIdGenerator: undefined,
      enableJsonResponse: true,
    });
    res.on('close', () => {
      void transport.close();
      void server.close();
    });
    try {
      await server.connect(transport);
      await transport.handleRequest(req, res, req.body);
    } catch (error) {
      next(error);
    }
  });
  app.all('/mcp', (_req, res) =>
    res
      .status(405)
      .set('Allow', 'POST')
      .json({ error: { code: 'METHOD_NOT_ALLOWED', message: 'Use Streamable HTTP POST.' } }),
  );
  const helpers = mountAccounts(app, { store, origin, mailer, formGuard, bridgeClients });
  // WebSocket upgrades (fleet screens) skip Express; startServer hands them to this.
  app.locals.fleet = mountFleet(app, { store, origin, helpers });
  mountSocial(app, { store, helpers, config: social, fetchImpl: socialFetch });
  mountAbTest(app, { store, origin, helpers });
  mountTasks(app, { store, origin, helpers });
  mountPublishing(app, { store, helpers });
  mountInstallLinks(app, { store, origin, mailer, helpers });
  mountRegistryFront(app);
  const hostedTraining =
    training?.databaseUrl && training?.masterKey
      ? createHostedTraining(training)
      : { configured: false, forUser: async () => null, close: async () => {} };
  app.locals.training = hostedTraining;
  const publishedTasks = store.tasksFor(origin);
  mountTraining(app, {
    helpers,
    hosted: hostedTraining,
    origin,
    publishTask: (userId, payload) => publishedTasks.publish(userId, payload),
  });
  app.use(
    '/assets',
    express.static(resolve(ROOT, 'public'), { maxAge: '1h', index: false, fallthrough: false }),
  );
  app.get('/manifest.webmanifest', (_req, res) =>
    res
      .type('application/manifest+json')
      .send(readFileSync(resolve(ROOT, 'public/manifest.webmanifest'))),
  );
  app.get('/sw.js', (_req, res) =>
    res.type('text/javascript').send(readFileSync(resolve(ROOT, 'public/sw.js'))),
  );
  app.get('/robots.txt', (_req, res) =>
    res.type('text/plain').send(`User-agent: *\nAllow: /\nSitemap: ${origin}/sitemap.xml\n`),
  );
  app.get('/sitemap.xml', (_req, res) =>
    res
      .type('application/xml')
      .send(
        `<?xml version="1.0" encoding="UTF-8"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">${['/mission', '/pricing', '/packages', '/packages/publish', '/specs', '/browse', '/prompts', '/desktop', '/de', '/training', '/training/liquid-agent', '/tasks', '/workflows', '/os', '/ab-testing', '/social', '/governance', '/developers', '/signup', '/terms', '/privacy'].map((path) => `<url><loc>${origin}${path}</loc></url>`).join('')}</urlset>`,
      ),
  );
  app.get('/docs/:file', (req, res) => {
    if (!/^[a-zA-Z0-9-]+\.md$/.test(req.params.file))
      throw new W3bsError('NOT_FOUND', 'Document not found.', 404);
    const path = resolve(ROOT, 'docs', req.params.file);
    if (!existsSync(path)) throw new W3bsError('NOT_FOUND', 'Document not found.', 404);
    res.type('text/markdown').send(readFileSync(path, 'utf8'));
  });
  // The PRD's original typo, and trailing-slash variants, land on the canonical page.
  // Agentic Desktop Layer installers: install.sh (Linux, macOS) and install.ps1 (Windows).
  for (const [route, file, type] of [
    ['/desktop/install.sh', 'install.sh', 'text/x-shellscript; charset=utf-8'],
    ['/desktop/install.ps1', 'install.ps1', 'text/plain; charset=utf-8'],
  ]) {
    app.get(route, (_req, res) => {
      const path = resolve(ROOT, 'desktop/packaging', file);
      if (!existsSync(path)) throw new W3bsError('NOT_FOUND', 'Installer not published yet.', 404);
      res.type(type).set('Cache-Control', 'public, max-age=300').send(readFileSync(path));
    });
  }
  // W3BS DE: installer, file manifest and files (w3bs.org/de).
  mountDe(app);
  // W3BS Training installers (the recorder in the desktop layer hands recordings to it).
  for (const [route, file, type] of [
    ['/training/install.sh', 'install.sh', 'text/x-shellscript; charset=utf-8'],
    ['/training/install.ps1', 'install.ps1', 'text/plain; charset=utf-8'],
  ]) {
    app.get(route, (_req, res) => {
      const path = resolve(ROOT, 'training/packaging', file);
      if (!existsSync(path)) throw new W3bsError('NOT_FOUND', 'Installer not published yet.', 404);
      res.type(type).set('Cache-Control', 'public, max-age=300').send(readFileSync(path));
    });
  }
  app.get([/^\/desktpo\/?$/, /^\/desktop\/$/], (req, res) => {
    const query = req.originalUrl.indexOf('?');
    res.redirect(301, `/desktop${query === -1 ? '' : req.originalUrl.slice(query)}`);
  });
  app.get(/.*/, async (req, res, next) => {
    try {
      res.type('html').send(
        await renderPage({
          path: req.path,
          host: req.hostname,
          query: req.query,
          store,
          origin,
          browser,
        }),
      );
    } catch (error) {
      next(error);
    }
  });
  app.use((error, req, res, _next) => {
    if (res.headersSent) return;
    const status =
      error instanceof W3bsError
        ? error.status
        : error.status === 413
          ? 413
          : error.type === 'entity.parse.failed'
            ? 400
            : error.status === 404
              ? 404
              : 500;
    const code =
      error.code ||
      (status === 413
        ? 'BODY_TOO_LARGE'
        : status === 400
          ? 'INVALID_JSON'
          : status === 404
            ? 'NOT_FOUND'
            : 'INTERNAL_ERROR');
    if (status === 500) process.stderr.write(`W3BS ${req.method} ${req.path}: ${error.message}\n`);
    res.status(status).json({
      error: { code, message: status === 500 ? 'Internal server error.' : error.message },
    });
  });
  return { app, store };
}
export async function startServer(options = {}) {
  const { app, store } = createApp({ ...options, store: options.store ?? (await openStore()) });
  // Background ThreatCrush re-scans of the package catalog. On in production (it
  // downloads every installer and repository), off elsewhere unless asked for.
  const scanSetting = process.env.W3BS_PACKAGE_SCAN;
  const scanner =
    options.scanner ??
    (scanSetting === 'on' || (process.env.NODE_ENV === 'production' && scanSetting !== 'off')
      ? createScanner(store)
      : undefined);
  scanner?.start();
  const server = createServer({ requestTimeout: 30000, headersTimeout: 10000 }, app);
  server.on('upgrade', (req, socket, head) => {
    if (!app.locals.fleet.upgrade(req, socket, head)) socket.destroy();
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(
      options.port ?? Number(process.env.PORT || 3000),
      options.host ?? process.env.HOST ?? '127.0.0.1',
      resolve,
    );
  });
  // Swap the bundled house footer for @latest now and every 15 min. A failed
  // fetch keeps the copy already in hand.
  const refreshFooter = () => refreshHouseFooter().catch(() => {});
  refreshFooter();
  const footerTimer = setInterval(refreshFooter, 15 * 60 * 1000);
  footerTimer.unref();
  return {
    server,
    store,
    close: async () => {
      scanner?.stop();
      app.locals.fleet.close();
      await app.locals.training.close();
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
      await store.close();
    },
  };
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const instance = await startServer();
  process.stderr.write(`W3BS listening on port ${instance.server.address().port}\n`);
  for (const signal of ['SIGINT', 'SIGTERM'])
    process.once(signal, async () => {
      await instance.close();
      process.exit(0);
    });
}
