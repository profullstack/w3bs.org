// w3bs.org/ab-testing: hosted OpenABTest experiments (src/abtest.mjs). The owner API takes
// the session or a `w3bs login` bearer; the producer API (assign, exposure, conversion)
// takes an `ab_` key from the page, so a server can call it without a person's login.
import { W3bsError } from './core.mjs';
import { AB_LIMITS, UNITS } from './abtest.mjs';
import { bearerOf } from './auth-routes.mjs';
import { escapeHtml as e } from './site.mjs';

const json = (value) => JSON.stringify(value).replaceAll('<', '\\u003c');
const pct = (x) => (x === null || x === undefined ? 'no data' : `${(x * 100).toFixed(1)}%`);
const money = ({ currency, totalMinor }) => `${currency} ${(totalMinor / 100).toFixed(2)}`;
const pValue = (p) => (p === null ? '' : p < 0.001 ? 'p < 0.001' : `p = ${p.toFixed(3)}`);

export function mountAbTest(app, { store, origin, helpers }) {
  const { accountUser, currentUser, page, onMainHost } = helpers;
  const ab = store.abtest;
  const owner = async (req) => {
    const user = await accountUser(req);
    if (!user) throw new W3bsError('SIGNED_OUT', 'Sign in first (w3bs login).', 401);
    return user.id;
  };
  // Producer calls: an ab_ key, or the owner themselves (handy from `w3bs ab assign`).
  const producer = async (req) => {
    const key = bearerOf(req);
    if (key?.startsWith('ab_')) {
      const userId = await ab.keyUser(key);
      if (!userId) throw new W3bsError('UNKNOWN_KEY', 'That A/B testing key is not valid.', 401);
      return userId;
    }
    return owner(req);
  };

  // Producer API.
  app.post('/api/ab/assign', async (req, res) =>
    res.json(await ab.assign(await producer(req), req.body ?? {})),
  );
  app.post('/api/ab/exposure', async (req, res) =>
    res.json(await ab.exposure(await producer(req), req.body ?? {})),
  );
  app.post('/api/ab/conversion', async (req, res) =>
    res.json(await ab.conversion(await producer(req), req.body ?? {})),
  );

  // Owner API.
  app.get('/api/ab/experiments', async (req, res) => res.json(await ab.list(await owner(req))));
  app.post('/api/ab/experiments', async (req, res) =>
    res.json(await ab.create(await owner(req), req.body ?? {})),
  );
  app.get('/api/ab/experiments/:slug', async (req, res) =>
    res.json(await ab.results(await owner(req), req.params.slug)),
  );
  app.patch('/api/ab/experiments/:slug', async (req, res) =>
    res.json(await ab.update(await owner(req), req.params.slug, req.body ?? {})),
  );
  app.post('/api/ab/experiments/:slug/state', async (req, res) =>
    res.json(await ab.setState(await owner(req), req.params.slug, req.body?.state)),
  );
  app.delete('/api/ab/experiments/:slug', async (req, res) =>
    res.json(await ab.remove(await owner(req), req.params.slug)),
  );
  app.get('/api/ab/experiments/:slug/manifest.json', async (req, res) =>
    res.json(await ab.manifest(await owner(req), req.params.slug)),
  );
  app.get('/api/ab/experiments/:slug/events.json', async (req, res) =>
    res.json(await ab.exportEvents(await owner(req), req.params.slug)),
  );
  app.get('/api/ab/keys', async (req, res) => res.json(await ab.keys(await owner(req))));
  app.post('/api/ab/keys', async (req, res) =>
    res.json(await ab.createKey(await owner(req), req.body?.name)),
  );
  app.delete('/api/ab/keys/:id', async (req, res) =>
    res.json(await ab.removeKey(await owner(req), req.params.id)),
  );

  const api = origin === 'https://w3bs.org' ? '' : ` --api ${origin}`;
  const quickStart = (
    slug = 'checkout-button',
    variant = 'control',
  ) => `# 1. Your server asks which variant this participant gets (sticky: same answer every time)
curl -s ${origin}/api/ab/assign \\
  -H "Authorization: Bearer $W3BS_AB_KEY" -H 'Content-Type: application/json' \\
  -d '{"experiment":"${slug}","participant":"user-1842"}'
# {"variant":"${variant}","parameters":{...},"assignmentId":"a_...","new":true,...}

# 2. You showed it
curl -s ${origin}/api/ab/exposure -H "Authorization: Bearer $W3BS_AB_KEY" \\
  -H 'Content-Type: application/json' -d '{"experiment":"${slug}","participant":"user-1842","surface":"checkout"}'

# 3. The goal happened (once per participant unless you send distinct conversionIds)
curl -s ${origin}/api/ab/conversion -H "Authorization: Bearer $W3BS_AB_KEY" \\
  -H 'Content-Type: application/json' -d '{"experiment":"${slug}","participant":"user-1842","valueMinor":4900,"currency":"USD"}'`;
  const terminal = `w3bs login${api}
w3bs ab ls
w3bs ab new checkout-button --variant control --variant green
w3bs ab start checkout-button
w3bs ab show checkout-button       # results per variant
w3bs ab assign checkout-button user-1842
w3bs ab pause|resume|close checkout-button`;

  const landing = `<section class="page-head"><div class="eyebrow">W3BS A/B TESTING · OPENABTEST 0.2 DRAFT</div><h1>A/B tests that <em>do not lie to you.</em></h1><p>Define variants here, ask w3bs.org which one each participant gets, and report what they saw and did. Assignments are sticky and decided on the server; results count unique participants per variant, show a 95% interval and never crown a winner for you. Everything exports as an <a class="inline-link" href="https://logicsrc.com/docs/openabtest" rel="noopener">OpenABTest</a> manifest and event stream, so the experiment is yours to take anywhere.</p><div class="actions"><a class="button primary" href="/signup?next=%2Fab-testing">Create a free account <span aria-hidden="true">↗</span></a><a class="text-link" href="/login?next=%2Fab-testing">Sign in <span aria-hidden="true">→</span></a></div></section><section class="section"><div class="section-heading"><div><div class="eyebrow">HOW IT WORKS</div><h2>Three calls from your server.</h2></div></div><pre class="instructions">${e(quickStart())}</pre><p class="muted">Free for up to ${AB_LIMITS.experiments} experiments and ${AB_LIMITS.variants} variants each. Participant ids are opaque: send an internal id or a hash, never an email.</p></section><section class="section"><div class="section-heading"><div><div class="eyebrow">WHAT IT WILL NOT DO</div><h2>Honest by default.</h2></div></div><ul class="prose"><li>No client-side coin flips: the first assignment is stored and every later call returns it, so a reload never switches someone's variant.</li><li>Assigned, exposed and converted are separate counts. A conversion without a recorded exposure is shown as such, not dropped.</li><li>Zero participants reads <strong>no data</strong>, not 0%.</li><li>No automatic winner. Meeting the sample floors is not significance, and a person makes the call.</li><li>A started experiment cannot be edited. Change the test, make a new experiment.</li></ul></section>`;

  app.get('/ab-testing', onMainHost, async (req, res) => {
    const user = await currentUser(req);
    if (!user)
      return page(res, {
        title: 'A/B testing',
        description:
          'Hosted A/B testing on the open OpenABTest format: sticky server-side assignment, separate exposure and conversion counts, no automatic winners.',
        active: '/ab-testing',
        robots: '',
        track: true,
        content: landing,
      });
    const [experiments, keys] = await Promise.all([ab.list(user.id), ab.keys(user.id)]);
    const rows = experiments.length
      ? `<div class="table-wrap"><table class="platform-table"><thead><tr><th>Experiment</th><th>State</th><th>Variants</th><th>Unit</th><th>Created</th></tr></thead><tbody>${experiments
          .map(
            (x) =>
              `<tr><td><a class="inline-link" href="/ab-testing/${e(x.slug)}"><strong>${e(x.name)}</strong></a><br><code>${e(x.slug)}</code></td><td><span class="pill">${e(x.state)}</span></td><td>${x.variants.map((v) => `<code>${e(v.id)}</code>`).join(' ')}</td><td>${e(x.unit)}</td><td>${e(x.created_at.slice(0, 10))}</td></tr>`,
          )
          .join('')}</tbody></table></div>`
      : '<p class="muted">No experiments yet. Make one below.</p>';
    const keyRows = keys.length
      ? `<ul class="account-list">${keys
          .map(
            (k) =>
              `<li><strong>${e(k.name)}</strong> <span class="muted">made ${e(k.created_at.slice(0, 10))} · ${k.last_used_at ? `last used ${e(k.last_used_at.slice(0, 16).replace('T', ' '))} UTC` : 'never used'}</span> <button class="small-button" type="button" data-action="remove-key" data-id="${e(k.id)}">Revoke</button></li>`,
          )
          .join('')}</ul>`
      : '<p class="muted">No keys yet.</p>';
    page(res, {
      title: 'A/B testing',
      active: '/ab-testing',
      scripts: '<script type="module" src="/assets/ab-testing.js"></script>',
      content: `<section class="page-head compact"><div class="eyebrow">W3BS A/B TESTING</div><h1>Your <em>experiments.</em></h1><p>Hosted <a class="inline-link" href="https://logicsrc.com/docs/openabtest" rel="noopener">OpenABTest</a> experiments for @${e(user.username)}. Up to ${AB_LIMITS.experiments} experiments, ${AB_LIMITS.variants} variants each. <a class="inline-link" href="/docs/ab-testing.md">Guide</a>.</p></section>${rows}<section class="inspector-panel"><h3>New experiment</h3><form class="auth-form" data-form="ab-create"><label class="input-label" for="ab-name">Name</label><input id="ab-name" name="name" maxlength="120" required placeholder="Checkout button colour"><label class="input-label" for="ab-slug">Id <span class="muted">(what your server sends; lowercase, digits, dashes)</span></label><input id="ab-slug" name="slug" maxlength="64" required pattern="[a-z0-9][a-z0-9\\-]{0,63}" placeholder="checkout-button"><label class="input-label" for="ab-unit">Randomization unit</label><select id="ab-unit" name="unit">${UNITS.map((u) => `<option value="${u}">${u}</option>`).join('')}</select><label class="input-label" for="ab-variants">Variants <span class="muted">(one per line: <code>id weight {"json":"parameters"}</code>; the first is the control)</span></label><textarea id="ab-variants" name="variants" rows="4" required>control 1 {"color":"blue"}
green 1 {"color":"green"}</textarea><label class="input-label" for="ab-goal">Goal <span class="muted">(what a conversion means; your server reports it)</span></label><input id="ab-goal" name="goal" maxlength="128" pattern="[A-Za-z0-9][A-Za-z0-9._:\\-]{0,127}" placeholder="signup-completed"><label class="input-label" for="ab-rule">Who is eligible <span class="muted">(optional, prose)</span></label><input id="ab-rule" name="eligibilityRule" maxlength="1000" placeholder="Signed-in users who reach checkout"><label class="input-label" for="ab-days">Runs for (days)</label><input id="ab-days" name="days" type="number" min="1" max="3650" value="${AB_LIMITS.defaultDays}"><label class="input-label" for="ab-floor">Minimum participants per variant before anyone reads the result</label><input id="ab-floor" name="minimumEligiblePerVariant" type="number" min="1" value="${AB_LIMITS.floorPerVariant}"><button class="button primary" type="submit">Create draft</button><p class="form-status" role="status" aria-live="polite"></p></form></section><section class="inspector-panel"><h3>Server keys</h3><p>Your server sends one of these as <code>Authorization: Bearer ab_…</code> to assign and report. It can only call assign, exposure and conversion for your experiments.</p>${keyRows}<form class="auth-form inline" data-form="ab-key"><label class="input-label" for="ab-key-name">Name</label><input id="ab-key-name" name="name" maxlength="60" placeholder="production"><button class="button" type="submit">Make key</button><p class="form-status" role="status" aria-live="polite"></p></form><div class="fleet-command" hidden><p>Copy it now; it is not shown again.</p><pre class="instructions" id="ab-key-value"></pre><button class="small-button" type="button" data-action="copy-key">Copy</button></div></section><section class="inspector-panel"><h3>From your server</h3><pre class="instructions">${e(quickStart())}</pre></section><section class="inspector-panel"><h3>From the terminal</h3><pre class="instructions">${e(terminal)}</pre></section>`,
    });
  });

  app.get('/ab-testing/:slug', onMainHost, async (req, res) => {
    const user = await currentUser(req);
    if (!user)
      return res.redirect(
        302,
        `/login?next=${encodeURIComponent(`/ab-testing/${req.params.slug}`)}`,
      );
    const r = await ab.results(user.id, req.params.slug).catch(() => null);
    if (!r)
      return page(
        res,
        {
          title: 'Not found',
          content:
            '<section class="page-head compact"><h1>No such experiment.</h1><p><a class="inline-link" href="/ab-testing">Back to your experiments</a></p></section>',
        },
        404,
      );
    const x = r.experiment;
    const moves = {
      draft: [['running', 'Start']],
      running: [
        ['paused', 'Pause'],
        ['closed', 'Close'],
      ],
      paused: [
        ['running', 'Resume'],
        ['closed', 'Close'],
      ],
      closed: [],
    }[x.state];
    const buttons = moves
      .map(
        ([state, label]) =>
          `<button class="${state === 'running' ? 'button primary' : 'small-button'}" type="button" data-action="state" data-state="${state}">${label}</button>`,
      )
      .join(' ');
    const table = `<div class="table-wrap"><table class="platform-table"><thead><tr><th>Variant</th><th>Weight</th><th>Assigned</th><th>Exposed</th><th>Converted</th><th>Conversion rate (95%)</th><th>vs ${e(r.control)}</th><th>Value</th></tr></thead><tbody>${r.variants
      .map(
        (v) =>
          `<tr><td><code>${e(v.id)}</code>${v.id === r.control ? ' <span class="pill">control</span>' : ''}<br><span class="muted">${e(JSON.stringify(v.parameters))}</span></td><td>${v.weight}</td><td>${v.assigned}${v.floorMet ? '' : ` <span class="muted">of ${r.floors.minimumEligiblePerVariant}</span>`}</td><td>${v.exposed}</td><td>${v.converted}${v.convertedWithoutExposure ? `<br><span class="muted">${v.convertedWithoutExposure} without a recorded exposure</span>` : ''}</td><td>${pct(v.conversionRate)}${v.interval95 ? `<br><span class="muted">${pct(v.interval95[0])} to ${pct(v.interval95[1])}</span>` : ''}</td><td>${v.vsControl ? `${v.vsControl.difference >= 0 ? '+' : ''}${(v.vsControl.difference * 100).toFixed(1)} pts<br><span class="muted">${pValue(v.vsControl.pValue)}</span>` : ''}</td><td>${v.value.map(money).join('<br>') || ''}</td></tr>`,
      )
      .join('')}</tbody></table></div>`;
    const days = (s) => `${Math.floor(s / 86400)}d ${Math.floor((s % 86400) / 3600)}h`;
    const floors = `<p class="table-note">${r.floors.met ? 'Both floors are met.' : 'Floors not met yet:'} ${r.floors.minimumEligiblePerVariant} participants per variant, ${days(r.floors.minimumObservationSeconds)} running (so far ${days(r.floors.observedSeconds)}). ${e(r.note)}</p>`;
    page(res, {
      title: x.name,
      active: '/ab-testing',
      scripts: '<script type="module" src="/assets/ab-testing.js"></script>',
      content: `<section class="page-head compact"><a class="inline-link" href="/ab-testing">← Experiments</a><div class="eyebrow">EXPERIMENT · ${e(x.unit.toUpperCase())} · <span class="pill">${e(x.state)}</span></div><h1>${e(x.name)}</h1><p><code>${e(x.slug)}</code> · goal <code>${e(x.goal)}</code>${x.window ? ` · runs ${e(x.window.startsAt.slice(0, 16).replace('T', ' '))} to ${e(x.window.endsAt.slice(0, 16).replace('T', ' '))} UTC` : ' · the window is fixed when you start it'}</p></section><script type="application/json" id="ab-data">${json({ slug: x.slug, state: x.state })}</script><div class="ab-actions">${buttons} <a class="small-button" href="/api/ab/experiments/${e(x.slug)}/manifest.json" download="${e(x.slug)}.openabtest.json">OpenABTest manifest</a> <a class="small-button" href="/api/ab/experiments/${e(x.slug)}/events.json" download="${e(x.slug)}.events.json">Events</a> <button class="small-button" type="button" data-action="delete">Delete</button></div><p class="form-status" id="ab-status" role="status" aria-live="polite"></p>${table}${floors}<section class="inspector-panel"><h3>From your server</h3><pre class="instructions">${e(quickStart(x.slug, r.control))}</pre><p class="muted">Make a key on <a class="inline-link" href="/ab-testing">the experiments page</a>. Paused and closed experiments still return existing assignments, and take no new participants: show your default experience when <code>variant</code> is null.</p></section>`,
    });
  });
}
