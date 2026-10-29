// w3bs.org/fleet: the signed-in fleet page, the account API it and `w3bs fleet ls/run/rm`
// use (session or a `w3bs login` bearer), the agent API (`fm_` machine keys), and the screen
// viewer /fleet/<id>/screen with its WebSocket relay (src/fleet-screen.mjs).
// The "SSH access checks" section is @profullstack/fleetbar, mounted as published (the same
// panel threatcrush.com/fleet mounts): <fleet-bar> talks to /api/fleetbar, which proxies to
// the user's crawlproof Fleet project with the w3bs partner token (CRAWLPROOF_UPTIME_TOKEN).
import express from 'express';
import { createHash } from 'node:crypto';
import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { fleetbarExpress } from '@profullstack/fleetbar/server';
import { STYLES as FLEETBAR_STYLES } from '@profullstack/fleetbar/element';
import { W3bsError } from './core.mjs';
import { ACTIONS, FLEET_LIMITS, HISTORY, SCREEN_MODES } from './fleet.mjs';
import { createScreenRelay } from './fleet-screen.mjs';
import { bearerOf } from './auth-routes.mjs';
import { escapeHtml as e } from './site.mjs';

const fleetbarDist = dirname(
  fileURLToPath(import.meta.resolve('@profullstack/fleetbar/dist/fleetbar.js')),
);
const cspHash = (text) => `'sha256-${createHash('sha256').update(text).digest('base64')}'`;
// <fleet-bar> writes its stylesheet as a <style> in its shadow root and one inline style
// attribute. The site CSP is style-src 'self', so /fleet allows exactly those two by hash.
export const FLEETBAR_STYLE_SRC = `'self' ${cspHash(FLEETBAR_STYLES)} 'unsafe-hashes' ${cspHash('gap: var(--fleetbar-gap)')}`;

export function mountFleet(app, { store, origin, helpers, fleetbar = {} }) {
  const { accountUser, currentUser, page, onMainHost } = helpers;
  const mainHost = new URL(origin).hostname;
  // The panel acts as the signed-in person on their crawlproof account, so only a browser
  // session on the main host counts: any bearer (even `w3bs login`'s) gets 401 here.
  // w3bs.org proves every address: an account exists only once its emailed link was opened
  // (accounts.consumeLink), and the address cannot be changed afterwards.
  const emailVerified = fleetbar.emailVerified ?? ((user) => Boolean(user.email));
  app.use(
    fleetbarExpress({
      token: fleetbar.token ?? (() => process.env.CRAWLPROOF_UPTIME_TOKEN),
      crawlproofUrl:
        fleetbar.crawlproofUrl ?? process.env.CRAWLPROOF_URL ?? 'https://crawlproof.com',
      ...(fleetbar.fetch ? { fetch: fleetbar.fetch } : {}),
      getUser: async (_request, { req }) => {
        if (bearerOf(req) || req.hostname !== mainHost) return null;
        const user = await currentUser(req);
        return user ? { email: user.email, emailVerified: emailVerified(user) === true } : null;
      },
    }),
  );
  app.use(
    '/vendor/fleetbar',
    express.static(fleetbarDist, { maxAge: '1h', index: false, fallthrough: false }),
  );
  const fleet = store.fleet;
  const screen = createScreenRelay({ fleet, origin, currentUser });
  const withScreen = (machines) =>
    machines.map((m) => ({ ...m, screen_listening: screen.listening(m.id) }));
  const owner = async (req) => {
    const user = await accountUser(req);
    if (!user) throw new W3bsError('SIGNED_OUT', 'Sign in first (w3bs login).', 401);
    return user;
  };
  const agentKey = (req) => {
    const key = bearerOf(req);
    if (!key?.startsWith('fm_')) throw new W3bsError('UNKNOWN_MACHINE', 'Agent key required.', 401);
    return key;
  };

  // Agent API: called by `w3bs fleet join` and `w3bs fleet agent` on the machine.
  app.post('/api/fleet/join', async (req, res) => res.json(await fleet.join(req.body ?? {})));
  app.post('/api/fleet/heartbeat', async (req, res) =>
    res.json(await fleet.heartbeat(agentKey(req), req.body?.info)),
  );
  app.post('/api/fleet/actions/:id/result', async (req, res) =>
    res.json(await fleet.actionResult(agentKey(req), req.params.id, req.body ?? {})),
  );
  app.post('/api/fleet/leave', async (req, res) => {
    const machine = await fleet.machineByKey(agentKey(req));
    if (machine) screen.drop(machine.id);
    res.json(await fleet.leave(agentKey(req)));
  });

  // Owner API.
  const overview = async (userId) => {
    const view = await fleet.overview(userId);
    return { ...view, machines: withScreen(view.machines) };
  };
  // Enrolled machines, each with its inventory device (or null). `w3bs fleet ls` reads this.
  app.get('/api/fleet/machines', async (req, res) =>
    res.json((await overview((await owner(req)).id)).machines),
  );
  // The whole fleet: machines, inventory devices without an agent, and the monthly cost.
  app.get('/api/fleet', async (req, res) => res.json(await overview((await owner(req)).id)));
  // The account's OpenDevice inventory, whole, so `fleet list <url>` can read it back.
  app.get('/api/fleet/inventory', async (req, res) => {
    const stored = await fleet.inventory((await owner(req)).id);
    if (!stored)
      throw new W3bsError(
        'NO_INVENTORY',
        'No inventory yet. Import one: w3bs fleet inventory import opendevice.json',
        404,
      );
    res.set('Last-Modified', new Date(stored.updated_at).toUTCString()).json(stored.document);
  });
  app.put('/api/fleet/inventory', async (req, res) => {
    const user = await owner(req);
    const saved = await fleet.setInventory(user.id, req.body);
    res.json({ ...saved, summary: (await fleet.overview(user.id)).summary });
  });
  app.delete('/api/fleet/inventory', async (req, res) =>
    res.json(await fleet.deleteInventory((await owner(req)).id)),
  );
  app.post('/api/fleet/tokens', async (req, res) => {
    const user = await owner(req);
    const { token, expiresAt } = await fleet.createToken(user.id, {
      name: req.body?.name,
      allowActions: req.body?.allowActions === true,
    });
    const api = origin === 'https://w3bs.org' ? '' : ` --api ${origin}`;
    const share = SCREEN_MODES.includes(req.body?.screen) ? ` --screen ${req.body.screen}` : '';
    res.json({
      token,
      expiresAt,
      command: `curl -fsSL ${origin}/install.sh | W3BS_YES=1 sh && ~/.local/bin/w3bs fleet join ${token}${share}${api}`,
    });
  });
  app.post('/api/fleet/machines/:id/actions', async (req, res) =>
    res.json(await fleet.queueAction((await owner(req)).id, req.params.id, req.body?.action)),
  );
  app.patch('/api/fleet/machines/:id', async (req, res) =>
    res.json(await fleet.rename((await owner(req)).id, req.params.id, req.body?.name)),
  );
  app.delete('/api/fleet/machines/:id', async (req, res) => {
    const result = await fleet.removeMachine((await owner(req)).id, req.params.id);
    screen.drop(req.params.id);
    res.json(result);
  });

  app.get('/fleet', onMainHost, async (req, res) => {
    const user = await currentUser(req);
    if (!user) return res.redirect(302, `/login?next=${encodeURIComponent('/fleet')}`);
    const view = await overview(user.id);
    res.set(
      'Content-Security-Policy',
      String(res.get('Content-Security-Policy')).replace(
        "style-src 'self'",
        `style-src ${FLEETBAR_STYLE_SRC}`,
      ),
    );
    page(res, {
      title: 'Fleet',
      active: '/fleet',
      scripts:
        '<script type="module" src="/assets/fleet.js"></script><script src="/vendor/fleetbar/fleetbar.js" defer></script>',
      content: `<section class="page-head compact"><div class="eyebrow">W3BS FLEET</div><h1>Your <em>devices.</em></h1><p>Every machine that ran <code>w3bs fleet join</code> for @${e(user.username)}, plus every device in your <a class="inline-link" href="https://logicsrc.com/docs/opendevice">OpenDevice</a> inventory: servers, desktops, phones and tablets, with what each costs a month. Joined machines report every ${FLEET_LIMITS.intervalSeconds} seconds. Free for up to ${FLEET_LIMITS.machines} machines; <a class="inline-link" href="/pricing">limits</a>.</p></section><script type="application/json" id="fleet-data">${JSON.stringify({ ...view, actions: ACTIONS, history: HISTORY }).replaceAll('<', '\\u003c')}</script><div id="fleet-summary" class="fleet-summary" aria-live="polite"></div><div id="fleet-devices" class="fleet-devices"></div><div id="fleet-list" class="fleet-list" aria-live="polite"></div><section class="inspector-panel fleet-add"><h3>Add a server</h3><p>Makes a one-time join command, valid for an hour. Run it on the server as the user the agent should run as.</p><form class="auth-form inline" data-form="fleet-token"><label class="input-label" for="fleet-name">Name <span class="muted">(optional, defaults to the hostname)</span></label><input id="fleet-name" name="name" maxlength="60" placeholder="dev2"><label class="consent"><input type="checkbox" name="allowActions" value="1"> Allow actions from this page (update w3bs, restart the desktop layer, report now). Nothing else can run.</label><label class="input-label" for="fleet-screen">Screen <span class="muted">(watch the desktop from this page; needs a VNC server on the machine, or run <code>w3bs fleet desktop start</code> on a server without a desktop; see <a class="inline-link" href="/docs/fleet-screen.md">screen sharing</a>)</span></label><select id="fleet-screen" name="screen"><option value="">Not shared</option><option value="view">View only</option><option value="control">View and control</option></select><button class="button primary" type="submit">Make join command</button><p class="form-status" role="status" aria-live="polite"></p></form><div class="fleet-command" hidden><p>Run this on the server:</p><pre class="instructions" id="fleet-command"></pre><button class="small-button" type="button" data-action="copy-command">Copy</button></div></section><section class="inspector-panel fleet-inventory"><h3>Inventory</h3><p>An <a class="inline-link" href="https://logicsrc.com/docs/opendevice">OpenDevice</a> file lists every device you run, with or without an agent, and what it costs. Devices match joined machines by name or hostname, or by a <code>{"name": "w3bs", "id": "m_…"}</code> entry in their <code>agents</code>. Up to ${FLEET_LIMITS.inventoryDevices} devices, ${FLEET_LIMITS.inventoryKB} KB. A cost with no amount shows as unstated; nothing is guessed or converted.</p><form class="auth-form inline" data-form="fleet-inventory"><label class="input-label" for="fleet-inventory-file">opendevice.json</label><input id="fleet-inventory-file" name="file" type="file" accept=".json,application/json" required><button class="button primary" type="submit">Import inventory</button><p class="form-status" role="status" aria-live="polite"></p></form><p class="muted fleet-note" id="fleet-inventory-state"></p></section><section class="inspector-panel fleet-checks" id="ssh-checks"><h3>SSH access checks</h3><p>Checks that run 24/7 on <a class="inline-link" href="https://crawlproof.com">crawlproof</a> and call or email you when one goes down: is each server still accepting your SSH key, plus HTTP, keyword, SSL and TCP checks. An SSH check never logs in and never needs a private key: crawlproof asks the server whether it would accept a public key from your <a class="inline-link" href="https://app.logicsrc.com/keyring">logicsrc keyring</a> and hangs up. These are the same checks threatcrush.com/fleet and your crawlproof Fleet project show.</p><fleet-bar endpoint="/api/fleetbar" theme="light"></fleet-bar></section><section class="inspector-panel"><h3>From the terminal</h3><pre class="instructions">w3bs login
w3bs fleet ls
w3bs fleet inventory import opendevice.json
w3bs fleet cost              # every device and the monthly total
w3bs fleet run &lt;name&gt; update
w3bs fleet rm &lt;name&gt;
w3bs fleet view &lt;name&gt;      # opens its screen here
w3bs fleet desktop start     # on a server: a Kubuntu desktop for AI agents, shared here</pre></section>`,
    });
  });

  // The screen viewer. noVNC (public/novnc.js) talks RFB over /api/fleet/machines/<id>/screen.
  app.get('/fleet/:id/screen', onMainHost, async (req, res) => {
    const user = await currentUser(req);
    if (!user)
      return res.redirect(
        302,
        `/login?next=${encodeURIComponent(`/fleet/${req.params.id}/screen`)}`,
      );
    const machine = await fleet.ownedMachine(user.id, req.params.id).catch(() => null);
    if (!machine)
      return page(
        res,
        {
          title: 'Not found',
          content:
            '<section class="page-head compact"><h1>No such machine.</h1><p><a class="inline-link" href="/fleet">Back to your fleet</a></p></section>',
        },
        404,
      );
    const data = {
      id: machine.id,
      name: machine.name,
      screen: machine.info.screen ?? null,
      online: machine.online,
      listening: screen.listening(machine.id),
    };
    page(res, {
      title: `${machine.name} screen`,
      active: '/fleet',
      scripts: '<script type="module" src="/assets/fleet-screen.js"></script>',
      content: `<section class="screen-bar"><a class="inline-link" href="/fleet">\u2190 Fleet</a><h1>${e(machine.name)}</h1><span class="pill" id="screen-state">Connecting\u2026</span><span class="muted" id="screen-mode"></span><span class="screen-tools"><label class="consent" id="screen-control-label" hidden><input type="checkbox" id="screen-control"> Control</label><button class="small-button" type="button" id="screen-fit">Fit</button><button class="small-button" type="button" id="screen-full">Full screen</button><button class="small-button" type="button" id="screen-reconnect" hidden>Reconnect</button></span></section><script type="application/json" id="screen-data">${JSON.stringify(data).replaceAll('<', '\\u003c')}</script><p class="screen-message" id="screen-message" role="status" aria-live="polite"></p><div class="screen-frame" id="screen"></div>`,
    });
  });
  return { upgrade: screen.upgrade, close: screen.close };
}
