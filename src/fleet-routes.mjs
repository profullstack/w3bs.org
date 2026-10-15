// w3bs.org/fleet: the signed-in fleet page, the account API it and `w3bs fleet ls/run/rm`
// use (session or a `w3bs login` bearer), the agent API (`fm_` machine keys), and the screen
// viewer /fleet/<id>/screen with its WebSocket relay (src/fleet-screen.mjs).
import express from 'express';
import { W3bsError } from './core.mjs';
import { ACTIONS, HISTORY, SCREEN_MODES } from './fleet.mjs';
import { createScreenRelay } from './fleet-screen.mjs';
import { bearerOf } from './auth-routes.mjs';
import { escapeHtml as e } from './site.mjs';

export function mountFleet(app, { store, origin, helpers }) {
  const { accountUser, currentUser, page, onMainHost } = helpers;
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
  app.get('/api/fleet/machines', async (req, res) =>
    res.json(withScreen(await fleet.machines((await owner(req)).id))),
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
    const machines = withScreen(await fleet.machines(user.id));
    page(res, {
      title: 'Fleet',
      active: '/fleet',
      scripts: '<script type="module" src="/assets/fleet.js"></script>',
      content: `<section class="page-head compact"><div class="eyebrow">W3BS FLEET</div><h1>Your <em>servers.</em></h1><p>Every machine that ran <code>w3bs fleet join</code> for @${e(user.username)}. Each one reports every 30 seconds.</p></section><script type="application/json" id="fleet-data">${JSON.stringify({ machines, actions: ACTIONS, history: HISTORY }).replaceAll('<', '\\u003c')}</script><div id="fleet-list" class="fleet-list" aria-live="polite"></div><section class="inspector-panel fleet-add"><h3>Add a server</h3><p>Makes a one-time join command, valid for an hour. Run it on the server as the user the agent should run as.</p><form class="auth-form inline" data-form="fleet-token"><label class="input-label" for="fleet-name">Name <span class="muted">(optional, defaults to the hostname)</span></label><input id="fleet-name" name="name" maxlength="60" placeholder="dev2"><label class="consent"><input type="checkbox" name="allowActions" value="1"> Allow actions from this page (update w3bs, restart the desktop layer, report now). Nothing else can run.</label><label class="input-label" for="fleet-screen">Screen <span class="muted">(watch the desktop from this page; needs a VNC server on the machine, or run <code>w3bs fleet desktop start</code> on a server without a desktop; see <a class="inline-link" href="/docs/fleet-screen.md">screen sharing</a>)</span></label><select id="fleet-screen" name="screen"><option value="">Not shared</option><option value="view">View only</option><option value="control">View and control</option></select><button class="button primary" type="submit">Make join command</button><p class="form-status" role="status" aria-live="polite"></p></form><div class="fleet-command" hidden><p>Run this on the server:</p><pre class="instructions" id="fleet-command"></pre><button class="small-button" type="button" data-action="copy-command">Copy</button></div></section><section class="inspector-panel"><h3>From the terminal</h3><pre class="instructions">w3bs login
w3bs fleet ls
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
