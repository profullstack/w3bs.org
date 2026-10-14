// w3bs.org/fleet: the signed-in fleet page, the account API it and `w3bs fleet ls/run/rm`
// use (session or a `w3bs login` bearer), and the agent API (`fm_` machine keys).
import express from 'express';
import { W3bsError } from './core.mjs';
import { ACTIONS } from './fleet.mjs';
import { bearerOf } from './auth-routes.mjs';
import { escapeHtml as e } from './site.mjs';

export function mountFleet(app, { store, origin, helpers }) {
  const { accountUser, currentUser, page, onMainHost } = helpers;
  const fleet = store.fleet;
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
  app.post('/api/fleet/leave', async (req, res) => res.json(await fleet.leave(agentKey(req))));

  // Owner API.
  app.get('/api/fleet/machines', async (req, res) =>
    res.json(await fleet.machines((await owner(req)).id)),
  );
  app.post('/api/fleet/tokens', async (req, res) => {
    const user = await owner(req);
    const { token, expiresAt } = await fleet.createToken(user.id, {
      name: req.body?.name,
      allowActions: req.body?.allowActions === true,
    });
    const api = origin === 'https://w3bs.org' ? '' : ` --api ${origin}`;
    res.json({
      token,
      expiresAt,
      command: `curl -fsSL ${origin}/install.sh | W3BS_YES=1 sh && ~/.local/bin/w3bs fleet join ${token}${api}`,
    });
  });
  app.post('/api/fleet/machines/:id/actions', async (req, res) =>
    res.json(await fleet.queueAction((await owner(req)).id, req.params.id, req.body?.action)),
  );
  app.patch('/api/fleet/machines/:id', async (req, res) =>
    res.json(await fleet.rename((await owner(req)).id, req.params.id, req.body?.name)),
  );
  app.delete('/api/fleet/machines/:id', async (req, res) =>
    res.json(await fleet.removeMachine((await owner(req)).id, req.params.id)),
  );

  app.get('/fleet', onMainHost, async (req, res) => {
    const user = await currentUser(req);
    if (!user) return res.redirect(302, `/login?next=${encodeURIComponent('/fleet')}`);
    const machines = await fleet.machines(user.id);
    page(res, {
      title: 'Fleet',
      active: '/fleet',
      scripts: '<script type="module" src="/assets/fleet.js"></script>',
      content: `<section class="page-head compact"><div class="eyebrow">W3BS FLEET</div><h1>Your <em>servers.</em></h1><p>Every machine that ran <code>w3bs fleet join</code> for @${e(user.username)}. Each one reports every 30 seconds.</p></section><script type="application/json" id="fleet-data">${JSON.stringify({ machines, actions: ACTIONS }).replaceAll('<', '\\u003c')}</script><div id="fleet-list" class="fleet-list" aria-live="polite"></div><section class="inspector-panel fleet-add"><h3>Add a server</h3><p>Makes a one-time join command, valid for an hour. Run it on the server as the user the agent should run as.</p><form class="auth-form inline" data-form="fleet-token"><label class="input-label" for="fleet-name">Name <span class="muted">(optional, defaults to the hostname)</span></label><input id="fleet-name" name="name" maxlength="60" placeholder="dev2"><label class="consent"><input type="checkbox" name="allowActions" value="1"> Allow actions from this page (update w3bs, restart the desktop layer, report now). Nothing else can run.</label><button class="button primary" type="submit">Make join command</button><p class="form-status" role="status" aria-live="polite"></p></form><div class="fleet-command" hidden><p>Run this on the server:</p><pre class="instructions" id="fleet-command"></pre><button class="small-button" type="button" data-action="copy-command">Copy</button></div></section><section class="inspector-panel"><h3>From the terminal</h3><pre class="instructions">w3bs login
w3bs fleet ls
w3bs fleet run &lt;name&gt; update
w3bs fleet rm &lt;name&gt;</pre></section>`,
    });
  });
}
