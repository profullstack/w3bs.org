// w3bs.org/training/app and /api/v1/training on the W3BS site: the same review app and API as
// the local `w3bs training open`, backed by the signed-in account's workspace in PostgreSQL
// (training/hosted.mjs).
//
// Who is calling decides the actor, and the service enforces the rest:
//   - the w3bs.org session cookie (the person in their browser)    -> human
//   - a `w3bs login` bearer (the CLI, e.g. `w3bs training push`)   -> agent "cli:<username>"
//     (it can upload; it cannot confirm, approve, release, grant or export)
//   - a trn_ agent token made in the app (Consumers)               -> agent "<consumer>"
// Cookie-authenticated writes also need the X-W3BS-Training header (the app sends it), on top
// of the site's Origin check and SameSite cookie.
import express from 'express';
import { createHash, randomBytes } from 'node:crypto';
import { StreamableHTTPServerTransport } from '@modelcontextprotocol/sdk/server/streamableHttp.js';
import { W3bsError } from '../src/core.mjs';
import { bearerOf } from '../src/auth-routes.mjs';
import { trainingRouter } from './http.mjs';
import { createTrainingMcp } from './mcp.mjs';
import { APP_CSS, APP_JS, INDEX_HTML, MARK_SVG } from './ui/assets.mjs';

// trn_<base64url user id>.<base64url secret>: '.' never appears in base64url, so the split is exact.
const AGENT_TOKEN = /^trn_([A-Za-z0-9_-]{1,120})\.([A-Za-z0-9_-]{32,64})$/;
const APP_CSP =
  "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob:; media-src 'self' blob:; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
const hash = (token) => createHash('sha256').update(token).digest('hex');

const fail = (res, error) =>
  res
    .status(error.status >= 400 ? error.status : 500)
    .json({ error: { code: error.code || 'REQUEST_FAILED', message: error.message } });

export function mountTraining(app, { helpers, hosted, origin }) {
  const { accountUser, currentUser } = helpers;
  const listeners = new Map(); // userId -> Set(send)

  // Resolve the caller to a workspace and an actor, or throw 401.
  async function caller(req) {
    if (!hosted.configured)
      throw new W3bsError(
        'TRAINING_UNAVAILABLE',
        'W3BS Training is not enabled on this server.',
        503,
      );
    // The site's bearerOf only takes [A-Za-z0-9_-]; agent tokens carry a '.', so read them here.
    const raw = /^Bearer (\S{1,300})$/.exec(req.headers.authorization || '')?.[1];
    const agent = raw && AGENT_TOKEN.exec(raw);
    const bearer = agent ? raw : bearerOf(req);
    if (agent) {
      const userId = Buffer.from(agent[1], 'base64url').toString('utf8');
      const svc = await hosted.forUser(userId);
      const consumer = await svc.agentTokenConsumer(hash(bearer));
      if (!consumer)
        throw new W3bsError(
          'UNAUTHENTICATED',
          'This agent token was revoked or never existed.',
          401,
        );
      return { userId, svc, actor: { kind: 'agent', id: consumer }, cookie: false };
    }
    const user = await accountUser(req);
    if (!user)
      throw new W3bsError('SIGNED_OUT', 'Sign in at w3bs.org (or `w3bs login` for the CLI).', 401);
    const svc = await hosted.forUser(user.id);
    return bearer
      ? {
          userId: user.id,
          svc,
          actor: { kind: 'agent', id: `cli:${user.username}` },
          cookie: false,
        }
      : { userId: user.id, svc, actor: { kind: 'human', id: user.username }, cookie: true };
  }

  const api = express.Router();
  api.use(async (req, res, next) => {
    try {
      req.training = await caller(req);
      if (
        req.training.cookie &&
        !['GET', 'HEAD'].includes(req.method) &&
        req.get('x-w3bs-training') !== '1'
      )
        throw new W3bsError('FORBIDDEN', 'Missing X-W3BS-Training header.', 403);
      next();
    } catch (error) {
      fail(res, error);
    }
  });
  const human = (req) => {
    if (req.training.actor.kind !== 'human')
      throw new W3bsError(
        'HUMAN_APPROVAL_REQUIRED',
        'Only the account holder, signed in at w3bs.org, can do this.',
        403,
      );
  };
  const route = (fn) => async (req, res) => {
    try {
      const out = await fn(req, res);
      if (out !== undefined) res.json(out);
    } catch (error) {
      fail(res, error);
    }
  };

  // Agent tokens: made and revoked by the person; the token is shown once.
  api.get(
    '/agent-tokens',
    route((req) => req.training.svc.listAgentTokens()),
  );
  api.post(
    '/agent-tokens',
    express.json({ limit: '4kb' }),
    route(async (req) => {
      human(req);
      const consumer = String(req.body?.consumer ?? '');
      const token = `trn_${Buffer.from(String(req.training.userId)).toString('base64url')}.${randomBytes(32).toString('base64url')}`;
      const row = await req.training.svc.addAgentToken(
        { consumer, tokenHash: hash(token) },
        req.training.actor,
      );
      const mcp = `${origin}/api/v1/training/mcp`;
      return {
        ...row,
        token,
        mcp_url: mcp,
        claude_code: `claude mcp add --transport http w3bs-training ${mcp} --header "Authorization: Bearer ${token}"`,
        // Claude's desktop app config takes stdio servers; mcp-remote bridges to HTTP with the header.
        claude_desktop: {
          mcpServers: {
            'w3bs-training': {
              command: 'npx',
              args: ['-y', 'mcp-remote', mcp, '--header', `Authorization: Bearer ${token}`],
            },
          },
        },
        note: 'Shown once. The agent still needs a retrieval grant for a released dataset version (Consumers > Grant).',
      };
    }),
  );
  api.delete(
    '/agent-tokens/:id',
    route(
      (req) => (human(req), req.training.svc.revokeAgentToken(req.params.id, req.training.actor)),
    ),
  );

  // Live updates for the account's open app tabs.
  api.get('/events', (req, res) => {
    res.set({
      'content-type': 'text/event-stream',
      'cache-control': 'no-store',
      connection: 'keep-alive',
    });
    res.flushHeaders();
    const send = (kind) => res.write(`event: change\ndata: ${JSON.stringify({ kind })}\n\n`);
    const id = req.training.userId;
    if (!listeners.has(id)) listeners.set(id, new Set());
    listeners.get(id).add(send);
    const beat = setInterval(() => res.write(': keepalive\n\n'), 25000);
    req.on('close', () => {
      clearInterval(beat);
      listeners.get(id)?.delete(send);
    });
  });

  // MCP over Streamable HTTP, for agents holding a trn_ token (the same tools as stdio).
  api.post('/mcp', express.json({ limit: '256kb' }), async (req, res) => {
    if (req.training.actor.kind !== 'agent' || req.training.actor.id.startsWith('cli:'))
      return fail(
        res,
        new W3bsError(
          'UNAUTHENTICATED',
          'MCP needs an agent token (create one under Consumers in w3bs.org/training/app).',
          401,
        ),
      );
    const server = createTrainingMcp(req.training.svc, req.training.actor.id);
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
      fail(res, error);
    }
  });
  api.all('/mcp', (_req, res) =>
    res
      .status(405)
      .set('Allow', 'POST')
      .json({ error: { code: 'METHOD_NOT_ALLOWED', message: 'Use Streamable HTTP POST.' } }),
  );

  api.use(
    trainingRouter((req) => req.training.svc, {
      actorFor: (req) => req.training.actor,
      hosted: true,
      uploadLimit: '64mb',
      onChange: (kind, req) => {
        for (const send of listeners.get(req?.training?.userId) ?? []) send(kind);
      },
    }),
  );
  app.use('/api/v1/training', api);

  // The app itself: signed-in only, no analytics or third-party scripts.
  const appPage = async (req, res) => {
    const user = await currentUser(req);
    if (!user) return res.redirect(302, `/login?next=${encodeURIComponent('/training/app/')}`);
    if (!hosted.configured)
      return res.status(503).type('text').send('W3BS Training is not enabled on this server yet.');
    res
      .set({ 'Content-Security-Policy': APP_CSP, 'Cache-Control': 'no-store' })
      .type('html')
      .send(INDEX_HTML);
  };
  // Express matches /training/app and /training/app/ alike; the app needs the slash so its
  // relative asset paths resolve under /training/app/.
  app.get('/training/app', (req, res, next) =>
    req.originalUrl.split('?')[0].endsWith('/')
      ? appPage(req, res).catch(next)
      : res.redirect(301, '/training/app/'),
  );
  const asset = (type, body) => (_req, res) =>
    res.set('Cache-Control', 'no-cache').type(type).send(body);
  app.get('/training/app/assets/app.js', asset('js', APP_JS));
  app.get('/training/app/assets/app.css', asset('css', APP_CSS));
  app.get('/training/app/assets/mark.svg', asset('svg', MARK_SVG));
}
