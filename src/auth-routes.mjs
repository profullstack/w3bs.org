// HTTP surface for w3bs.org accounts: the pages (/signup, /login, /account, the email
// link page and the OAuth consent page), the JSON API the browser talks to (/api/auth,
// /api/account, /api/me), the OAuth 2.1 endpoints and the @profullstack/bridges host.
import express from 'express';
import { randomBytes } from 'node:crypto';
import { createBridgeHost } from '@profullstack/bridges';
import { createFormGuard } from '@profullstack/stack/email';
import { W3bsError } from './core.mjs';
import { CLI_CLIENT_ID, OAuthError, SCOPES, TTL } from './accounts.mjs';
import { escapeHtml as e, pageShell } from './site.mjs';

// Paths a third party may call cross-origin: no cookies are read on them.
export const OPEN_CORS = [
  /^\/oauth\/(token|revoke)$/,
  /^\/\.well-known\/oauth-authorization-server$/,
];
export const bearerOf = (req) =>
  /^Bearer ([A-Za-z0-9_-]{1,100})$/.exec(req.headers.authorization || '')?.[1];

const safeNext = (value) => {
  const next = String(value ?? '');
  return next.startsWith('/') &&
    !next.startsWith('//') &&
    !next.includes('\\') &&
    next.length < 2000
    ? next
    : '/account';
};
const parseCookies = (header = '') =>
  Object.fromEntries(
    header
      .split(';')
      .map((part) => part.trim().split('='))
      .filter(([name, value]) => name && value !== undefined)
      .map(([name, ...rest]) => [name, decodeURIComponent(rest.join('='))]),
  );

export function mountAccounts(app, { store, origin, mailer, formGuard = {}, bridgeClients }) {
  const accounts = store.accounts(origin);
  const host = new URL(origin).hostname;
  const secure = origin.startsWith('https:');
  const cookieName = secure ? '__Host-w3bs_session' : 'w3bs_session';
  const formSecret = process.env.W3BS_FORM_SECRET || randomBytes(32).toString('hex');
  if (!process.env.W3BS_FORM_SECRET && process.env.NODE_ENV === 'production')
    process.stderr.write(
      'W3BS: W3BS_FORM_SECRET is not set; signup form tokens reset on restart\n',
    );
  const guards = Object.fromEntries(
    ['signup', 'email'].map((binding) => [
      binding,
      createFormGuard({
        secret: formSecret,
        binding,
        minAgeMs: 1500,
        rateLimit: { max: binding === 'signup' ? 5 : 10, windowMs: 60 * 60 * 1000 },
        ...formGuard,
      }),
    ]),
  );
  // dev2's nginx sets X-Real-IP and overwrites what the client sent; X-Forwarded-For
  // from the client is not trusted.
  const clientIp = (req) => String(req.headers['x-real-ip'] || req.socket.remoteAddress || '');

  const setSession = (res, id) =>
    res.cookie(cookieName, id, {
      httpOnly: true,
      secure,
      sameSite: 'lax',
      path: '/',
      maxAge: TTL.session,
    });
  const sessionId = (req) => parseCookies(req.headers.cookie)[cookieName];
  const currentUser = async (req) =>
    (req.w3bsUser ??= (await accounts.sessionUser(sessionId(req))) || null);
  const requireUser = async (req) => {
    const user = await currentUser(req);
    if (!user) throw new W3bsError('SIGNED_OUT', 'Sign in first.', 401);
    return user;
  };
  const page = (res, options, status = 200) =>
    res
      .status(status)
      .type('html')
      .send(
        pageShell({
          robots: 'noindex',
          scripts: '<script type="module" src="/assets/account.js"></script>',
          ...options,
        }),
      );
  // Account pages and cookies live on the main host only.
  const onMainHost = (req, res, next) =>
    req.hostname === host ? next() : res.redirect(302, `${origin}${req.originalUrl}`);

  const sendLink = async (result) => {
    if (!result.link) return;
    const url = `${origin}/auth/link?token=${encodeURIComponent(result.link)}`;
    const signup = result.purpose === 'signup';
    const subject = signup ? 'Confirm your email for W3BS' : 'Your W3BS sign-in link';
    const lead = signup
      ? `Confirm this address to finish creating the W3BS account @${result.username}.`
      : `Sign in to W3BS as @${result.username}.`;
    const text = `${lead}\n\n${url}\n\nThe link works once and expires in 20 minutes. If you did not ask for it, ignore this email.\n`;
    await mailer.send({
      to: result.email,
      subject,
      text,
      html: `<p>${e(lead)}</p><p><a href="${e(url)}">${signup ? 'Confirm and create account' : 'Sign in'}</a></p><p style="color:#6b7268">The link works once and expires in 20 minutes. If you did not ask for it, ignore this email.</p>`,
    });
  };
  const guarded = async (req, binding) => {
    const verdict = await guards[binding].check({
      fields: req.body ?? {},
      headers: req.headers,
      ip: clientIp(req),
    });
    if (verdict.action === 'retry') throw new W3bsError('RETRY', 'Please try again.', 400);
    if (verdict.action === 'limited')
      throw new W3bsError('RATE_LIMITED', 'Too many requests. Try again later.', 429);
    return verdict.allow;
  };
  const sentMessage = (email) => ({
    ok: true,
    message: `If ${email} can receive mail, a link is on its way. It expires in 20 minutes.`,
  });

  // ---- JSON API -------------------------------------------------------------
  const api = express.Router();
  api.use('/auth', onMainHost);
  api.use('/account', onMainHost);
  api.get('/auth/session', async (req, res) => res.json({ user: await currentUser(req) }));
  api.post('/auth/signup', async (req, res) => {
    const body = req.body ?? {};
    if (!(await guarded(req, 'signup'))) return res.json(sentMessage(String(body.email ?? '')));
    const result = await accounts.startSignup(body);
    await sendLink(result);
    res.json(sentMessage(result.email));
  });
  api.post('/auth/email', async (req, res) => {
    const body = req.body ?? {};
    if (!(await guarded(req, 'email'))) return res.json(sentMessage(String(body.email ?? '')));
    const result = await accounts.startEmailLogin(body.email);
    await sendLink(result);
    res.json(sentMessage(result.email));
  });
  api.post('/auth/link', async (req, res) => {
    const { user, created } = await accounts.consumeLink(req.body?.token);
    setSession(res, await accounts.createSession(user.id));
    res.json({ user, created });
  });
  api.post('/auth/login', async (req, res) => {
    const user = await accounts.passwordLogin(req.body?.login, req.body?.password);
    setSession(res, await accounts.createSession(user.id));
    res.json({ user });
  });
  api.post('/auth/passkey/options', async (_req, res) =>
    res.json(await accounts.passkeyLoginOptions()),
  );
  api.post('/auth/passkey', async (req, res) => {
    const user = await accounts.passkeyLogin(req.body ?? {});
    setSession(res, await accounts.createSession(user.id));
    res.json({ user });
  });
  api.post('/auth/logout', async (req, res) => {
    await accounts.endSession(sessionId(req));
    res.clearCookie(cookieName, { path: '/', secure, httpOnly: true, sameSite: 'lax' });
    res.json({ ok: true });
  });
  api.post('/account/passkeys/options', async (req, res) =>
    res.json(await accounts.passkeyRegistrationOptions(await requireUser(req))),
  );
  api.post('/account/passkeys', async (req, res) =>
    res.json(await accounts.passkeyRegister(await requireUser(req), req.body ?? {})),
  );
  api.get('/account/passkeys', async (req, res) =>
    res.json(await accounts.listPasskeys((await requireUser(req)).id)),
  );
  api.delete('/account/passkeys/:id', async (req, res) => {
    await accounts.deletePasskey((await requireUser(req)).id, req.params.id);
    res.json({ ok: true });
  });
  api.put('/account/password', async (req, res) => {
    await accounts.setPassword((await requireUser(req)).id, req.body?.password ?? null);
    res.json({ ok: true });
  });
  api.get('/account/grants', async (req, res) =>
    res.json(await accounts.grants((await requireUser(req)).id)),
  );
  api.delete('/account/grants/:id', async (req, res) => {
    await accounts.revokeGrant((await requireUser(req)).id, req.params.id);
    res.json({ ok: true });
  });
  api.get('/account/apps', async (req, res) =>
    res.json(await accounts.listClients((await requireUser(req)).id)),
  );
  api.post('/account/apps', async (req, res) =>
    res.json(await accounts.createClient((await requireUser(req)).id, req.body ?? {})),
  );
  api.delete('/account/apps/:id', async (req, res) => {
    await accounts.deleteClient((await requireUser(req)).id, req.params.id);
    res.json({ ok: true });
  });
  app.use('/api', api);

  // userinfo: an OAuth access token, or the browser session on the main host.
  app.get('/api/me', async (req, res) => {
    const bearer = bearerOf(req);
    const user = bearer
      ? await accounts.bearerUser(bearer)
      : req.hostname === host
        ? await currentUser(req)
        : null;
    if (!user) {
      if (bearer) res.set('WWW-Authenticate', 'Bearer error="invalid_token"');
      throw new W3bsError('SIGNED_OUT', 'Not signed in.', 401);
    }
    res.json({
      sub: user.id,
      username: user.username,
      email: user.email,
      email_verified: true,
      created_at: user.createdAt,
    });
  });

  // ---- OAuth 2.1 authorization server ---------------------------------------
  app.get('/.well-known/oauth-authorization-server', (_req, res) =>
    res.json({
      issuer: origin,
      authorization_endpoint: `${origin}/oauth/authorize`,
      token_endpoint: `${origin}/oauth/token`,
      revocation_endpoint: `${origin}/oauth/revoke`,
      userinfo_endpoint: `${origin}/api/me`,
      response_types_supported: ['code'],
      grant_types_supported: ['authorization_code', 'refresh_token'],
      code_challenge_methods_supported: ['S256'],
      token_endpoint_auth_methods_supported: ['none'],
      revocation_endpoint_auth_methods_supported: ['none'],
      scopes_supported: SCOPES,
      authorization_response_iss_parameter_supported: true,
      service_documentation: `${origin}/developers#accounts`,
    }),
  );
  const oauthBody = [
    express.urlencoded({ extended: false, limit: '16kb' }),
    express.json({ limit: '16kb' }),
  ];
  const oauthJson = (handler) => async (req, res) => {
    res.set({ 'Cache-Control': 'no-store', Pragma: 'no-cache' });
    try {
      await handler(req, res);
    } catch (error) {
      if (!(error instanceof OAuthError)) throw error;
      res.status(error.status).json({ error: error.error, error_description: error.message });
    }
  };
  app.post(
    '/oauth/token',
    ...oauthBody,
    oauthJson(async (req, res) => res.json(await accounts.exchange(req.body ?? {}))),
  );
  app.post(
    '/oauth/revoke',
    ...oauthBody,
    oauthJson(async (req, res) => {
      await accounts.revokeToken(req.body?.token, req.body?.client_id);
      res.json({});
    }),
  );
  const redirectWith = (request, params) => {
    const url = new URL(request.redirectUri);
    for (const [key, value] of Object.entries({ ...params, state: request.state, iss: origin }))
      if (value !== undefined) url.searchParams.set(key, value);
    return url.href;
  };
  const authorizeError = (res, error) => {
    if (!(error instanceof OAuthError)) throw error;
    if (error.redirect)
      return res.redirect(
        302,
        redirectWith(error.redirect, { error: error.error, error_description: error.message }),
      );
    return page(
      res,
      {
        title: 'Sign-in request not valid',
        content: `<section class="page-head compact"><div class="eyebrow">SIGN IN WITH W3BS</div><h1>This sign-in request<br>is <em>not valid.</em></h1><p>${e(error.message)}</p><p>Go back to the app and start again. If it keeps happening, tell the app's developer.</p></section>`,
      },
      400,
    );
  };
  const formFields = (query) =>
    [
      'client_id',
      'redirect_uri',
      'response_type',
      'code_challenge',
      'code_challenge_method',
      'scope',
      'state',
    ]
      .filter((name) => query[name] !== undefined)
      .map((name) => `<input type="hidden" name="${name}" value="${e(query[name])}">`)
      .join('');
  app.get('/oauth/authorize', onMainHost, async (req, res) => {
    let request;
    try {
      request = await accounts.authorizationRequest(req.query);
    } catch (error) {
      return authorizeError(res, error);
    }
    const user = await currentUser(req);
    if (!user) return res.redirect(302, `/login?next=${encodeURIComponent(req.originalUrl)}`);
    const cli = request.client.id === CLI_CLIENT_ID;
    const target = new URL(request.redirectUri);
    // The consent form posts here and is redirected to the app; form-action must allow it.
    res.set(
      'Content-Security-Policy',
      `default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self' ${target.origin}`,
    );
    page(res, {
      title: `Sign in to ${request.client.name}`,
      active: '/account',
      content: `<section class="auth-card"><div class="eyebrow">SIGN IN WITH W3BS</div><h1>${cli ? 'Sign in to the <em>w3bs CLI?</em>' : `Sign in to <em>${e(request.client.name)}?</em>`}</h1><p>${
        cli
          ? 'The w3bs CLI on this computer is asking to act as you. Only continue if you just ran <code>w3bs login</code>.'
          : `<strong>${e(request.client.name)}</strong> will receive your username and email address. You will be sent back to <code>${e(target.origin)}</code>.`
      }</p><p class="auth-who">Signed in as <strong>@${e(user.username)}</strong> · <a href="/login?switch=1&amp;next=${encodeURIComponent(req.originalUrl)}">Not you?</a></p><form method="post" action="/oauth/authorize" class="auth-actions">${formFields(req.query)}<button class="button primary" name="decision" value="allow" type="submit">Allow</button><button class="button" name="decision" value="deny" type="submit">Deny</button></form><p class="muted auth-small">You can revoke this at any time from <a href="/account">your account</a>.</p></section>`,
    });
  });
  app.post(
    '/oauth/authorize',
    onMainHost,
    express.urlencoded({ extended: false, limit: '16kb' }),
    async (req, res) => {
      let request;
      try {
        request = await accounts.authorizationRequest(req.body ?? {});
      } catch (error) {
        return authorizeError(res, error);
      }
      const user = await currentUser(req);
      if (!user)
        return res.redirect(
          303,
          `/login?next=${encodeURIComponent(`/oauth/authorize?${new URLSearchParams(req.body)}`)}`,
        );
      if (req.body.decision !== 'allow')
        return res.redirect(
          303,
          redirectWith(request, {
            error: 'access_denied',
            error_description: 'The user denied the request.',
          }),
        );
      res.redirect(
        303,
        redirectWith(request, { code: await accounts.issueCode(user.id, request) }),
      );
    },
  );

  // ---- @profullstack/bridges host: silent sign-in for first-party sites ------
  if (bridgeClients && Object.keys(bridgeClients).length) {
    const bridge = createBridgeHost({
      clients: bridgeClients,
      async getUser(request) {
        const id = parseCookies(request.headers.get('cookie') ?? '')[cookieName];
        const user = await accounts.sessionUser(id);
        return (
          user && {
            sub: user.id,
            username: user.username,
            name: user.username,
            email: user.email,
            emailVerified: true,
          }
        );
      },
      loginUrl: (returnTo) => `/login?next=${encodeURIComponent(returnTo)}`,
    });
    const toRequest = (req) =>
      new Request(`${origin}${req.originalUrl}`, {
        method: req.method,
        headers: Object.entries(req.headers).flatMap(([name, value]) =>
          Array.isArray(value) ? value.map((item) => [name, item]) : [[name, String(value)]],
        ),
        body: ['GET', 'HEAD'].includes(req.method) ? undefined : req.rawBody,
      });
    const send = async (res, response) => {
      res.status(response.status);
      response.headers.forEach((value, name) => res.append(name, value));
      res.send(Buffer.from(await response.arrayBuffer()));
    };
    app.get('/api/v1/bridge/authorize', onMainHost, async (req, res) =>
      send(res, await bridge.authorize(toRequest(req))),
    );
    app.post(
      '/api/v1/bridge/token',
      express.raw({ type: () => true, limit: '16kb' }),
      async (req, res) => {
        req.rawBody = req.body;
        await send(res, await bridge.token(toRequest(req)));
      },
    );
  }

  // ---- Pages ----------------------------------------------------------------
  // form-guard's hiddenHTML() hides the honeypot with an inline style, which this site's
  // CSP blocks (leaving a visible "Website" box), so the markup is ours and the class
  // hides it from the stylesheet.
  const guardFields = async (binding) => {
    const { token, honeypot } = guards[binding].fields(await guards[binding].issue());
    return `<input type="hidden" name="${e(token.name)}" value="${e(token.value)}"><div class="hp-field" aria-hidden="true"><label>Website<input type="text" name="${e(honeypot.name)}" tabindex="-1" autocomplete="off"></label></div>`;
  };
  const emailForm = async (next) =>
    `<form class="auth-form" data-form="email" data-next="${e(next)}"><label class="input-label" for="email-link">Email</label><input id="email-link" name="email" type="email" autocomplete="email" required maxlength="254">${await guardFields('email')}<button class="button" type="submit">Email me a sign-in link</button><p class="form-status" role="status" aria-live="polite"></p></form>`;
  app.get('/login', onMainHost, async (req, res) => {
    const next = safeNext(req.query.next);
    if (await currentUser(req)) {
      if (req.query.switch === undefined) return res.redirect(302, next);
    }
    page(res, {
      title: 'Sign in',
      active: '/account',
      content: `<section class="auth-card"><div class="eyebrow">W3BS ACCOUNT</div><h1>Sign <em>in.</em></h1><button class="button primary auth-wide" type="button" data-action="passkey-login" data-next="${e(next)}">Sign in with a passkey</button><div class="auth-or"><span>or</span></div><form class="auth-form" data-form="login" data-next="${e(next)}"><label class="input-label" for="login">Username or email</label><input id="login" name="login" autocomplete="username webauthn" required maxlength="254"><label class="input-label" for="password">Password</label><input id="password" name="password" type="password" autocomplete="current-password" required maxlength="256"><button class="button dark" type="submit">Sign in</button><p class="form-status" role="status" aria-live="polite"></p></form><details class="auth-fallback"><summary>Forgot your password, or no passkey on this device?</summary>${await emailForm(next)}</details><p class="auth-small">New here? <a href="/signup${next !== '/account' ? `?next=${encodeURIComponent(next)}` : ''}">Create an account</a></p></section>`,
    });
  });
  app.get('/signup', onMainHost, async (req, res) => {
    if (await currentUser(req)) return res.redirect(302, '/account');
    page(res, {
      title: 'Create an account',
      active: '/account',
      content: `<section class="auth-card"><div class="eyebrow">W3BS ACCOUNT</div><h1>Create an <em>account.</em></h1><p>One account for w3bs.org, the <code>w3bs</code> CLI and apps that offer "Sign in with W3BS".</p><form class="auth-form" data-form="signup"><label class="input-label" for="username">Username</label><input id="username" name="username" autocomplete="username" required minlength="3" maxlength="32" pattern="[a-z0-9](?:[a-z0-9-]*[a-z0-9])?" spellcheck="false" autocapitalize="none"><p class="field-description">3 to 32 lowercase letters, digits and hyphens. Public.</p><label class="input-label" for="email">Email</label><input id="email" name="email" type="email" autocomplete="email" required maxlength="254"><p class="field-description">We send a link to confirm it. Never shown publicly.</p><label class="input-label" for="new-password">Password <span class="muted">(optional)</span></label><input id="new-password" name="password" type="password" autocomplete="new-password" minlength="10" maxlength="256"><p class="field-description">At least 10 characters. Leave it empty to use a passkey instead; you add one right after confirming.</p>${await guardFields('signup')}<button class="button primary" type="submit">Create account</button><p class="form-status" role="status" aria-live="polite"></p></form><p class="auth-small">Have an account? <a href="/login">Sign in</a></p></section>`,
    });
  });
  app.get('/auth/link', onMainHost, async (req, res) => {
    const token = String(req.query.token ?? '');
    const link = await accounts.peekLink(token);
    // Opening the link never uses it (mail scanners open links); the button does.
    page(res, {
      title: link?.purpose === 'signup' ? 'Confirm your email' : 'Sign in',
      active: '/account',
      content: link
        ? `<section class="auth-card"><div class="eyebrow">W3BS ACCOUNT</div><h1>${link.purpose === 'signup' ? 'Confirm and <em>create.</em>' : 'Continue to <em>sign in.</em>'}</h1><p>${link.purpose === 'signup' ? 'Confirm' : 'Sign in with'} <strong>${e(link.email)}</strong>.</p><form class="auth-form" data-form="link"><input type="hidden" name="token" value="${e(token)}"><button class="button primary" type="submit">${link.purpose === 'signup' ? 'Confirm and create account' : 'Sign in'}</button><p class="form-status" role="status" aria-live="polite"></p></form></section>`
        : `<section class="auth-card"><div class="eyebrow">W3BS ACCOUNT</div><h1>This link has <em>expired.</em></h1><p>Links work once and for 20 minutes. <a href="/login">Request a new one</a>.</p></section>`,
    });
  });
  app.get('/account', onMainHost, async (req, res) => {
    const user = await currentUser(req);
    if (!user) return res.redirect(302, `/login?next=${encodeURIComponent(req.originalUrl)}`);
    const [passkeys, grants, apps] = await Promise.all([
      accounts.listPasskeys(user.id),
      accounts.grants(user.id),
      accounts.listClients(user.id),
    ]);
    const date = (value) => (value ? e(value.slice(0, 10)) : 'never');
    page(res, {
      title: 'Your account',
      active: '/account',
      content: `<section class="page-head compact"><div class="eyebrow">W3BS ACCOUNT</div><h1>@${e(user.username)}</h1><p>${e(user.email)} · member since ${date(user.createdAt)}</p></section>${
        req.query.welcome !== undefined && !passkeys.length
          ? `<div class="callout"><strong>Welcome. Add a passkey next.</strong><p>A passkey signs you in with your fingerprint, face or device PIN, and cannot be phished.</p></div>`
          : ''
      }<div class="account-grid"><section class="inspector-panel"><h3>Passkeys</h3>${
        passkeys.length
          ? `<ul class="account-list">${passkeys.map((key) => `<li><span><strong>${e(key.name)}</strong><span class="muted">added ${date(key.createdAt)} · last used ${date(key.lastUsedAt)}</span></span><button class="small-button" type="button" data-action="delete-passkey" data-id="${e(key.id)}">Remove</button></li>`).join('')}</ul>`
          : '<p>No passkeys yet.</p>'
      }<form class="auth-form inline" data-form="add-passkey"><label class="input-label" for="passkey-name">Name</label><input id="passkey-name" name="name" maxlength="60" placeholder="e.g. Laptop"><button class="button primary" type="submit">Add a passkey</button><p class="form-status" role="status" aria-live="polite"></p></form></section><section class="inspector-panel"><h3>Password</h3><p>${user.hasPassword ? 'A password is set.' : 'No password set. You sign in with passkeys or an email link.'}</p><form class="auth-form inline" data-form="password"><label class="input-label" for="set-password">${user.hasPassword ? 'New password' : 'Set a password'}</label><input id="set-password" name="password" type="password" autocomplete="new-password" minlength="10" maxlength="256" required><button class="button dark" type="submit">${user.hasPassword ? 'Change password' : 'Set password'}</button><p class="form-status" role="status" aria-live="polite"></p></form>${user.hasPassword ? '<button class="small-button" type="button" data-action="remove-password">Remove password</button>' : ''}</section><section class="inspector-panel"><h3>Signed-in apps</h3>${
        grants.length
          ? `<ul class="account-list">${grants.map((grant) => `<li><span><strong>${e(grant.app)}</strong><span class="muted">${e(grant.scope)} · refreshed ${date(grant.lastRefreshedAt)}</span></span><button class="small-button" type="button" data-action="revoke-grant" data-id="${e(grant.id)}">Sign out</button></li>`).join('')}</ul>`
          : '<p>No apps are signed in. Run <code>w3bs login</code> to sign in the CLI.</p>'
      }</section><section class="inspector-panel"><h3>Your OAuth apps</h3><p>Let people sign in to your app with their W3BS account. Apps are public clients: authorization code with PKCE (S256), no client secret. <a href="/developers#accounts">Developer guide</a>.</p>${
        apps.length
          ? `<ul class="account-list">${apps.map((item) => `<li><span><strong>${e(item.name)}</strong><code>${e(item.id)}</code><span class="muted">${item.redirectUris.map(e).join('<br>')}</span></span><button class="small-button" type="button" data-action="delete-app" data-id="${e(item.id)}">Delete</button></li>`).join('')}</ul>`
          : ''
      }<form class="auth-form inline" data-form="app"><label class="input-label" for="app-name">App name</label><input id="app-name" name="name" required minlength="2" maxlength="60"><label class="input-label" for="app-redirects">Redirect URIs, one per line</label><textarea id="app-redirects" name="redirectUris" rows="3" required placeholder="https://example.com/auth/w3bs/callback"></textarea><button class="button" type="submit">Register app</button><p class="form-status" role="status" aria-live="polite"></p></form></section></div><p class="auth-small"><button class="small-button" type="button" data-action="logout">Sign out</button></p>`,
    });
  });
}
