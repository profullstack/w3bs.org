// w3bs.org/social: the page for W3BS Social, an AT Protocol PDS (Bluesky's reference
// server) at social.w3bs.org with handles under .social.w3bs.org. The PDS runs as its
// own service on its own hostname, because atproto puts /xrpc, /oauth and
// /.well-known/atproto-did at the root of a host and cannot live under a path.
// Sign-up on the PDS needs an invite code; a signed-in W3BS account gets one here.
import { W3bsError } from './core.mjs';
import { escapeHtml as e } from './site.mjs';

const DESCRIBE_TTL = 5 * 60 * 1000;

export function socialConfig(env = process.env) {
  return {
    pds: (env.W3BS_SOCIAL_PDS || 'https://social.w3bs.org').replace(/\/+$/, ''),
    adminPassword: env.W3BS_SOCIAL_ADMIN_PASSWORD || '',
  };
}

export function mountSocial(app, { store, helpers, config = socialConfig(), fetchImpl = fetch }) {
  const { accountUser, currentUser, page, onMainHost } = helpers;
  const social = store.social;
  const host = new URL(config.pds).hostname;
  let described = { at: 0, value: null };

  // The PDS's own description (handle domains, invite rule, links), cached briefly so the
  // page never waits on it twice; null when the PDS does not answer.
  async function describe() {
    if (Date.now() - described.at < DESCRIBE_TTL) return described.value;
    let value = null;
    try {
      const response = await fetchImpl(`${config.pds}/xrpc/com.atproto.server.describeServer`, {
        signal: AbortSignal.timeout(3000),
      });
      if (response.ok) value = await response.json();
    } catch {
      value = null;
    }
    described = { at: Date.now(), value };
    return value;
  }
  const info = async () => {
    const server = await describe();
    return {
      name: 'W3BS Social',
      protocol: 'atproto',
      pds: config.pds,
      did: server?.did ?? `did:web:${host}`,
      online: Boolean(server),
      handleDomains: server?.availableUserDomains ?? [`.${host}`],
      inviteCodeRequired: server?.inviteCodeRequired ?? true,
      invitesFromW3bs: Boolean(config.adminPassword),
    };
  };

  async function mintInvite() {
    if (!config.adminPassword)
      throw new W3bsError('SOCIAL_UNAVAILABLE', 'Invites are not open yet.', 503);
    const response = await fetchImpl(`${config.pds}/xrpc/com.atproto.server.createInviteCode`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Basic ${Buffer.from(`admin:${config.adminPassword}`).toString('base64')}`,
      },
      body: JSON.stringify({ useCount: 1 }),
      signal: AbortSignal.timeout(10000),
    }).catch(() => null);
    const body = response?.ok ? await response.json().catch(() => null) : null;
    if (!body?.code)
      throw new W3bsError(
        'SOCIAL_UNAVAILABLE',
        'The social server did not answer. Try again soon.',
        502,
      );
    return body.code;
  }

  app.get('/api/social', async (_req, res) => res.json(await info()));
  // One invite per W3BS account, from a session or a `w3bs login` bearer.
  app.post('/api/social/invite', async (req, res) => {
    const user = await accountUser(req);
    if (!user) throw new W3bsError('SIGNED_OUT', 'Sign in first (w3bs login).', 401);
    const saved =
      (await social.invite(user.id)) ?? (await social.saveInvite(user.id, await mintInvite()));
    res.json({ ...saved, pds: config.pds, handle: `${user.username.toLowerCase()}.${host}` });
  });

  app.get('/social', onMainHost, async (req, res) => {
    const [user, about] = await Promise.all([currentUser(req), info()]);
    const invite = user ? await social.invite(user.id) : null;
    const domain = about.handleDomains[0] ?? `.${host}`;
    const example = `${user ? e(user.username.toLowerCase()) : 'you'}${e(domain)}`;
    const join = !about.invitesFromW3bs
      ? `<p>Invites are not open yet. Ask on <a class="inline-link" href="/bbs">the community forums</a> and we will send you one.</p>`
      : !user
        ? `<p>Sign in with your W3BS account and you get an invite code here, one per account.</p><a class="button primary" href="/login?next=${encodeURIComponent('/social')}">Sign in for an invite</a> <a class="button" href="/signup">Create a W3BS account</a>`
        : `<form class="auth-form inline" data-form="social-invite"${invite ? ' hidden' : ''}><p>One invite code for @${e(user.username)}. It works once.</p><button class="button primary" type="submit">Get my invite code</button><p class="form-status" role="status" aria-live="polite"></p></form><div class="social-invite"${invite ? '' : ' hidden'}><p>Your invite code:</p><pre class="instructions" id="social-code">${e(invite?.code ?? '')}</pre><button class="small-button" type="button" data-action="copy-code">Copy</button></div>`;
    page(res, {
      title: 'Social',
      active: '/social',
      robots: '',
      description:
        'W3BS Social: an open AT Protocol server at social.w3bs.org. Get a handle like you.social.w3bs.org and use it from Bluesky or any atproto app.',
      scripts: '<script type="module" src="/assets/social.js"></script>',
      content: `<section class="page-head"><div class="eyebrow">W3BS SOCIAL · AT PROTOCOL</div><h1>Your handle on<br><em>the open social web.</em></h1><p>A Bluesky-style social network at <a class="inline-link" href="${e(config.pds)}">${e(host)}</a>, run by W3BS on the AT Protocol. Your account, posts and follows live on our server, and you talk to everyone on Bluesky and every other atproto app. Handles look like <code>${example}</code>.</p><a class="button primary" href="${e(config.pds)}">Open W3BS Social</a></section><div class="draft-note">${about.online ? 'Server online' : 'Server not answering right now'} · Invite only · Federated with the Bluesky network</div><section class="inspector-panel"><h3>1. Get an invite</h3>${join}</section><section class="inspector-panel"><h3>2. Create your account</h3><ol class="social-steps"><li>Open <a class="inline-link" href="${e(config.pds)}">${e(host)}</a> and choose <strong>Create account</strong>. The hosting provider is already <code>${e(host)}</code>.</li><li>Paste your invite code, then choose a handle. It ends in <code>${e(domain)}</code>.</li><li>Sign in there, or from the Bluesky app with <strong>Hosting provider → Custom → ${e(host)}</strong>.</li></ol><p>Already own a domain? Set it as your handle later under Settings → Account → Handle.</p></section><section class="inspector-panel"><h3>For developers and agents</h3><pre class="instructions">PDS        ${e(config.pds)}
DID        ${e(about.did)}
Handles    *${e(domain)}
Describe   ${e(config.pds)}/xrpc/com.atproto.server.describeServer
This page  GET /api/social · POST /api/social/invite (session or w3bs login bearer)</pre><p>Agents get accounts the same way people do. Label an automated account as a bot in its profile.</p></section><section class="inspector-panel" id="policy"><h3>Terms and privacy</h3><p>W3BS Social is a community service run by the W3BS initiative with no guarantee of uptime. Your posts, likes and follows are public on the AT Protocol network and get copied by relays and apps we do not control, so deleting them here does not delete those copies. We store your email address and password hash only to run your account, and we never sell or share them. Spam, harassment, illegal content and impersonation get an account taken down. Bluesky's moderation service labels content across the network. Questions or reports: <a class="inline-link" href="mailto:hello@w3bs.org">hello@w3bs.org</a>.</p></section>`,
    });
  });
}
