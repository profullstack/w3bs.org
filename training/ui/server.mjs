// Local W3BS Training app on 127.0.0.1. Access starts from a one-time launch
// link printed by `w3bs training open`; the link becomes an HttpOnly,
// SameSite=Strict session cookie. Writes also need a custom header and a
// same-origin Origin, and the Host header must be the loopback address, so a
// web page in the same browser cannot drive this app (no CORS is ever served).
import express from 'express';
import { randomBytes, timingSafeEqual } from 'node:crypto';
import { spawn } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';
import { trainingRouter } from '../http.mjs';

const here = fileURLToPath(new URL('./', import.meta.url));
const tokensCss = () => {
  try {
    return readFileSync(resolve(here, '../../public/tokens.css'), 'utf8');
  } catch {
    return ':root{--paper:#f6f4ed;--ink:#23352f;--muted:#6b7268;--orange:#d95231;--line:#dbded0;--sage:#e6ebdc;--green:#678c4f}';
  }
};
const CSP =
  "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob:; media-src 'self' blob:; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

const same = (a, b) => {
  const x = Buffer.from(String(a));
  const y = Buffer.from(String(b));
  return x.length === y.length && timingSafeEqual(x, y);
};

export function createLocalApp(service, { user = 'local-user', launchTtlMs = 60000 } = {}) {
  const app = express();
  app.disable('x-powered-by');
  const launch = {
    token: randomBytes(24).toString('base64url'),
    expires: Date.now() + launchTtlMs,
    used: false,
  };
  const sessions = new Set();
  const listeners = new Set();
  let port = 0;
  const allowedHosts = () => new Set([`127.0.0.1:${port}`, `localhost:${port}`]);
  const cookieOf = (req) =>
    /(?:^|;\s*)w3bs_training=([A-Za-z0-9_-]+)/.exec(req.get('cookie') || '')?.[1];
  const human = (req) => {
    const id = cookieOf(req);
    return id && sessions.has(id);
  };

  app.use((req, res, next) => {
    res.set({
      'content-security-policy': CSP,
      'x-content-type-options': 'nosniff',
      'referrer-policy': 'no-referrer',
      'cache-control': 'no-store',
      'cross-origin-resource-policy': 'same-origin',
      'cross-origin-opener-policy': 'same-origin',
    });
    if (!allowedHosts().has(req.get('host') || ''))
      return res.status(421).type('text').send('Wrong host.');
    if (req.method === 'OPTIONS') return res.status(405).end();
    if (!['GET', 'HEAD'].includes(req.method)) {
      const origin = req.get('origin');
      if (
        req.get('x-w3bs-training') !== '1' ||
        (origin && !allowedHosts().has(origin.replace(/^http:\/\//, '')))
      )
        return res
          .status(403)
          .json({ error: { code: 'FORBIDDEN', message: 'Cross-site request refused.' } });
    }
    next();
  });

  app.get('/launch', (req, res) => {
    if (launch.used || Date.now() > launch.expires || !same(req.query.t || '', launch.token))
      return res
        .status(401)
        .type('text')
        .send('This launch link has expired or was already used. Run `w3bs training open` again.');
    launch.used = true;
    const id = randomBytes(24).toString('base64url');
    sessions.add(id);
    res.set('set-cookie', `w3bs_training=${id}; HttpOnly; SameSite=Strict; Path=/`);
    res.redirect(303, '/');
  });

  app.get('/assets/app.css', (_req, res) =>
    res.type('css').send(tokensCss() + readFileSync(resolve(here, 'public/app.css'), 'utf8')),
  );
  app.get('/assets/app.js', (_req, res) =>
    res.type('js').send(readFileSync(resolve(here, 'public/app.js'), 'utf8')),
  );
  app.get('/assets/mark.svg', (_req, res) =>
    res.type('svg').send(readFileSync(resolve(here, '../../public/mark.svg'))),
  );

  app.use('/api', (req, res, next) => {
    if (!human(req))
      return res
        .status(401)
        .json({
          error: {
            code: 'HUMAN_SESSION_REQUIRED',
            message: 'Open this app with `w3bs training open`.',
          },
        });
    next();
  });
  app.get('/api/v1/training/events', (req, res) => {
    res.set({ 'content-type': 'text/event-stream', connection: 'keep-alive' });
    res.flushHeaders();
    const send = (kind) => res.write(`event: change\ndata: ${JSON.stringify({ kind })}\n\n`);
    listeners.add(send);
    const beat = setInterval(() => res.write(': keepalive\n\n'), 25000);
    req.on('close', () => {
      clearInterval(beat);
      listeners.delete(send);
    });
  });
  app.use(
    '/api/v1/training',
    trainingRouter(service, {
      actorFor: () => ({ kind: 'human', id: user }),
      onChange: (kind) => listeners.forEach((send) => send(kind)),
    }),
  );
  app.get(/^\/(?!api\/).*/, (req, res) => {
    if (!human(req))
      return res
        .status(401)
        .type('html')
        .send(readFileSync(resolve(here, 'public/locked.html'), 'utf8'));
    res.type('html').send(readFileSync(resolve(here, 'public/index.html'), 'utf8'));
  });
  return {
    app,
    launchPath: () => `/launch?t=${launch.token}`,
    setPort: (value) => {
      port = value;
    },
  };
}

export async function serveLocalApp(service, { port = 0, openBrowser = true, user } = {}) {
  const local = createLocalApp(service, { user });
  const server = await new Promise((done, fail) => {
    const s = local.app.listen(port, '127.0.0.1', () => done(s));
    s.on('error', fail);
  });
  local.setPort(server.address().port);
  const url = `http://127.0.0.1:${server.address().port}${local.launchPath()}`;
  process.stderr.write(
    `W3BS Training is running at http://127.0.0.1:${server.address().port}\nOne-time link (valid 60 s): ${url}\nPress Ctrl+C to stop.\n`,
  );
  if (openBrowser) {
    const opener =
      process.platform === 'darwin'
        ? 'open'
        : process.platform === 'win32'
          ? 'explorer'
          : 'xdg-open';
    try {
      spawn(opener, [url], { stdio: 'ignore', detached: true })
        .on('error', () => {})
        .unref();
    } catch {
      // The link above still works.
    }
  }
  const stop = () => {
    server.close();
    service.close();
    process.exit(0);
  };
  process.on('SIGINT', stop);
  process.on('SIGTERM', stop);
  return server;
}
