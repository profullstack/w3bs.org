// w3bs.org/i/<package>: one link to install a package from anywhere (docs/os-prd.md §21).
// The page reads the device from its User-Agent and offers that device's route; a link never
// installs anything by itself. "Email me this link" sends only to the signed-in account's own
// verified address, a few times an hour, with no message of the sender's choosing.
import express from 'express';
import { W3bsError } from './core.mjs';
import { escapeHtml as e } from './site.mjs';

export const SEND_LIMIT = { perHour: 5 };
const INSTALL = 'curl -fsSL https://w3bs.org/install.sh | sh';

export function deviceOf(userAgent = '') {
  const ua = String(userAgent);
  if (/iPhone|iPad|iPod/i.test(ua) || (/Macintosh/.test(ua) && /Mobile\//.test(ua))) return 'ios';
  if (/Android/i.test(ua)) return 'android';
  if (/Windows/i.test(ua)) return 'windows';
  if (/Macintosh|Mac OS X/i.test(ua)) return 'mac';
  if (/Linux|X11|CrOS/i.test(ua)) return 'linux';
  return 'other';
}

const masked = (email) => {
  const [user, host] = String(email).split('@');
  return `${user.slice(0, 1)}•••@${host}`;
};

export function mountInstallLinks(app, { store, origin, mailer, helpers }) {
  const { currentUser, page, onMainHost } = helpers;
  const sent = new Map(); // user id -> send times in the last hour

  async function linkPage(req, res, notice = '', status = 200) {
    let id;
    try {
      id = `${decodeURIComponent(req.params.namespace)}/${decodeURIComponent(req.params.name)}`;
    } catch {
      throw new W3bsError('NOT_FOUND', 'Page not found.', 404);
    }
    const item = await store.package(id);
    const user = await currentUser(req);
    const device = deviceOf(req.get('user-agent'));
    const link = `${origin}/i/${id}`;
    const command = `w3bs install ${id}`;
    const copy = (text) =>
      `<div class="identity-line"><code>${e(text)}</code><button class="small-button" type="button" data-copy="${e(text)}">Copy</button></div>`;
    const publisher = item.signed
      ? `<p><span class="pill green">Signed</span> Version ${e(item.signed.statement.version)}, signed by the owner of <strong>${e(item.signed.statement.package.split('/')[0])}</strong> with SSH key <code class="break">${e(item.signed.fingerprint)}</code>. <code>w3bs</code> checks that signature and the installer before anything runs.</p>`
      : `<p><span class="pill">Registry listing</span> Listed by the W3BS registry; not signed by its publisher yet.</p>`;
    const desktop = `<h3>On this computer</h3><p>With <code>w3bs</code> installed, one command installs it, after showing what will run:</p>${copy(command)}<p class="muted">No <code>w3bs</code> yet? Install it first (macOS and Linux):</p>${copy(INSTALL)}`;
    const elsewhere = (why) =>
      `<h3>Install it on a computer</h3><p>${why} Open this link on a Mac or Linux computer, or email it to yourself below.</p>${copy(link)}`;
    const route = {
      mac: desktop,
      linux: desktop,
      other: desktop,
      windows: elsewhere(
        'The w3bs command line is not on Windows yet (it runs in WSL: open a WSL terminal and use the commands on the Linux page).',
      ),
      android: elsewhere(
        'The W3BS app for Android, which will install from links like this one, is coming.',
      ),
      ios: elsewhere('iPhone and iPad install apps only from the App Store.'),
    }[device];
    const send =
      user && !user.email
        ? `<p class="muted"><a class="inline-link" href="/account">Add an email to your account</a> to email this link to yourself.</p>`
        : user
          ? `<form method="post" action="/i/${e(id)}/send" class="auth-form inline"><button class="button" type="submit">Email me this link</button><p class="muted">Sends ${e(link)} to ${e(masked(user.email))}, your account's address. Nobody else can be sent this.</p></form>`
          : `<p class="muted"><a class="inline-link" href="/login?next=${encodeURIComponent(`/i/${id}`)}">Sign in</a> to email this link to yourself.</p>`;
    page(
      res,
      {
        title: `Install ${item.name}`,
        active: '/packages',
        content: `<section class="page-head compact"><div class="eyebrow">INSTALL · ${e(id)}</div><h1>Install <em>${e(item.name)}.</em></h1><p>${e(item.description)}</p></section>${notice ? `<div class="draft-note" role="status">${notice}</div>` : ''}<section class="inspector-panel"><h3>Who published it</h3>${publisher}<a class="text-link" href="/packages/${e(id)}">Package details <span aria-hidden="true">→</span></a></section><section class="inspector-panel">${route}</section><section class="inspector-panel"><h3>Send it to yourself</h3>${send}</section>`,
      },
      status,
    );
  }

  app.get('/i/:namespace/:name', onMainHost, (req, res) => linkPage(req, res));

  app.post(
    '/i/:namespace/:name/send',
    onMainHost,
    express.urlencoded({ extended: false, limit: '1kb' }),
    async (req, res) => {
      const user = await currentUser(req);
      if (!user)
        return res.redirect(
          303,
          `/login?next=${encodeURIComponent(req.path.replace(/\/send$/, ''))}`,
        );
      // An account CoinPay created has no confirmed address to send to yet.
      if (!user.email) return res.redirect(303, '/account');
      const id = `${req.params.namespace}/${req.params.name}`;
      const item = await store.package(id);
      const now = Date.now();
      const recent = (sent.get(user.id) || []).filter((t) => now - t < 3600_000);
      if (recent.length >= SEND_LIMIT.perHour)
        return linkPage(
          req,
          res,
          `You have sent ${SEND_LIMIT.perHour} links in the last hour. Try again later.`,
          429,
        );
      recent.push(now);
      sent.set(user.id, recent);
      const link = `${origin}/i/${id}`;
      const signer = item.signed
        ? `Signed by the owner of ${item.signed.statement.package.split('/')[0]} (${item.signed.fingerprint}).`
        : 'Listed by the W3BS registry.';
      const result = await mailer.send({
        to: user.email,
        subject: `Install ${item.name}`,
        text: `Open this link on the computer you want ${item.name} on:\n\n${link}\n\n${item.description}\n${signer}\n\nYou asked for this on w3bs.org while signed in as @${user.username}.`,
      });
      return linkPage(
        req,
        res,
        result.sent
          ? `Sent to ${e(masked(user.email))}.`
          : 'The email could not be sent. Try again in a minute.',
        result.sent ? 200 : 502,
      );
    },
  );
}
