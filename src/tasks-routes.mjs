// w3bs.org/tasks and w3bs.org/workflows (src/tasks.mjs). Reading is public: the pages, the
// OpenTask and OpenWorkflow documents, the .mosh files and /.well-known/openworkflow.json.
// Writing takes the session or a `w3bs login` bearer; `w3bs training publish` is the usual
// way a task arrives, after the person confirmed it in a terminal or the training app.
import { W3bsError } from './core.mjs';
import { escapeHtml as e } from './site.mjs';

const TASK_FILE = /^\/tasks\/([a-z0-9][a-z0-9-]{0,63})\.(json|mosh)$/;
const WORKFLOW_FILE = /^\/workflows\/([a-z0-9][a-z0-9-]{0,63})\.json$/;
const SPEC = {
  task: 'https://logicsrc.com/docs/opentask',
  workflow: 'https://logicsrc.com/docs/openworkflow',
};
const day = (iso) => e(String(iso).slice(0, 10));

const describe = (s) =>
  [
    `<strong>${e(s.action)}</strong>`,
    s.app && e(s.app),
    s.window && `<span class="muted">(${e(s.window)})</span>`,
    s.target && `[${e([s.target.role, s.target.name].filter(Boolean).join(' '))}]`,
    s.keys && `<code>${e(s.keys.join('+'))}</code>`,
    s.text && `<code>${e(s.text)}</code>`,
    s.note && `<span class="muted">${e(s.note)}</span>`,
  ]
    .filter(Boolean)
    .join(' ');

export function mountTasks(app, { store, origin, helpers }) {
  const { accountUser, currentUser, page } = helpers;
  const tasks = store.tasksFor(origin);
  const owner = async (req) => {
    const user = await accountUser(req);
    if (!user) throw new W3bsError('SIGNED_OUT', 'Sign in first (w3bs login).', 401);
    return user.id;
  };
  const notFound = (res, what, back) =>
    page(
      res,
      {
        title: 'Not found',
        content: `<section class="page-head compact"><h1>No such ${what}.</h1><p><a class="inline-link" href="/${back}">All ${back}</a></p></section>`,
      },
      404,
    );

  // API.
  app.get('/api/v1/tasks', async (_req, res) => res.json({ tasks: await tasks.list() }));
  app.get('/api/v1/tasks/mine', async (req, res) =>
    res.json({ tasks: await tasks.list({ userId: await owner(req) }) }),
  );
  app.post('/api/v1/tasks', async (req, res) =>
    res.status(201).json(await tasks.publish(await owner(req), req.body ?? {})),
  );
  app.delete('/api/v1/tasks/:id', async (req, res) =>
    res.json(await tasks.unpublish(await owner(req), req.params.id)),
  );
  app.get('/api/v1/workflows', async (_req, res) =>
    res.json({ workflows: await tasks.workflows() }),
  );
  app.post('/api/v1/workflows', async (req, res) =>
    res.status(201).json(await tasks.createWorkflow(await owner(req), req.body ?? {})),
  );
  app.put('/api/v1/workflows/:id', async (req, res) =>
    res.json(await tasks.updateWorkflow(await owner(req), req.params.id, req.body ?? {})),
  );
  app.delete('/api/v1/workflows/:id', async (req, res) =>
    res.json(await tasks.deleteWorkflow(await owner(req), req.params.id)),
  );

  // Documents. The task bytes are stored, so their digests match what workflows carry.
  app.get('/.well-known/openworkflow.json', async (_req, res) =>
    res.set('Access-Control-Allow-Origin', '*').json(await tasks.index()),
  );
  app.get(TASK_FILE, async (req, res, next) => {
    const [, id, kind] = TASK_FILE.exec(req.path);
    const t = await tasks.get(id);
    if (!t) return next();
    res.set({ 'Access-Control-Allow-Origin': '*', 'X-Content-Type-Options': 'nosniff' });
    if (kind === 'json') return res.type('application/json').send(t.body);
    res
      .type('text/plain; charset=utf-8')
      .set('Content-Disposition', `inline; filename="${id}.mosh"`)
      .send(t.script);
  });
  app.get(WORKFLOW_FILE, async (req, res, next) => {
    const w = await tasks.workflow(WORKFLOW_FILE.exec(req.path)[1]);
    if (!w) return next();
    res.set('Access-Control-Allow-Origin', '*').json(w.doc);
  });

  const scripts = '<script type="module" src="/assets/tasks.js"></script>';
  const publishHow = `w3bs training publish EPISODE_ID                      # window titles removed
w3bs training publish EPISODE_ID --workflow "Release a package"   # and add it to a workflow
w3bs training unpublish TASK_ID`;

  // Pages.
  app.get('/tasks', async (req, res) => {
    const [user, all] = await Promise.all([currentUser(req), tasks.list()]);
    const rows = all.length
      ? `<div class="table-wrap"><table class="platform-table"><thead><tr><th>Task</th><th>Apps</th><th>Steps</th><th>Status</th><th>Publisher</th><th>Published</th></tr></thead><tbody>${all
          .map(
            (t) =>
              `<tr><td><a class="inline-link" href="/tasks/${e(t.id)}"><strong>${e(t.title)}</strong></a></td><td>${e(t.apps.join(', '))}</td><td>${t.steps}</td><td><span class="pill">${e(t.status)}</span></td><td>${e(t.publisher)}</td><td>${day(t.published_at)}</td></tr>`,
          )
          .join('')}</tbody></table></div>`
      : '<p class="muted">Nothing published yet. Tasks appear here when someone publishes a reviewed demonstration from W3BS Training.</p>';
    page(res, {
      title: 'Tasks',
      description:
        'Desktop tasks people taught W3BS Training and published as OpenTasks: the goal and the steps as meaning, never coordinates or typed text, each with a moshscript to run it.',
      robots: '',
      track: true,
      scripts,
      content: `<section class="page-head"><div class="eyebrow">W3BS TASKS · OPENTASK 0.1</div><h1>Tasks people <em>showed once.</em></h1><p>Each task here is a reviewed demonstration from <a class="inline-link" href="/training">W3BS Training</a>, published as an <a class="inline-link" href="${SPEC.task}" rel="noopener">OpenTask</a>: the goal, the apps and the steps as meaning, with no coordinates, no typed text and no window titles unless the person kept them. Each one comes with a moshscript: <code>moshcode run</code> hands it to an agent that drives the desktop through the <a class="inline-link" href="/desktop">Agentic Desktop Layer</a>, looking before every action. Collect them into <a class="inline-link" href="/workflows">workflows</a>.</p></section>${rows}<section class="inspector-panel"><h3>Publish yours</h3><p>Record and review a task in W3BS Training, mark it complete, then press <strong>Publish to w3bs.org/tasks</strong> in the app or run:</p><pre class="instructions">${e(publishHow)}</pre>${user ? '' : '<p class="muted"><a class="inline-link" href="/login?next=%2Ftasks">Sign in</a> or run <code>w3bs login</code> first.</p>'}<p class="muted">Every task is <span class="pill">untested</span> until someone other than the person who recorded it runs it and it passes. A task grants no permission to act.</p></section>`,
    });
  });

  app.get('/tasks/:id', async (req, res) => {
    const t = await tasks.get(req.params.id);
    if (!t) return notFound(res, 'task', 'tasks');
    const user = await currentUser(req);
    const task = t.task;
    const params = (task.parameters ?? []).length
      ? `<h3>Values it asks for</h3><ul class="prose">${task.parameters.map((p) => `<li><code>{{${e(p.name)}}}</code> <span class="pill">${e(p.sensitivity)}</span></li>`).join('')}</ul>`
      : '';
    page(res, {
      title: task.title,
      description: `${task.goal} An OpenTask with ${task.steps.length} step(s), published on W3BS.`,
      robots: '',
      track: true,
      scripts,
      content: `<section class="page-head compact"><a class="inline-link" href="/tasks">← Tasks</a><div class="eyebrow">OPENTASK · <span class="pill">${e(task.status)}</span> · ${e(t.publisher)} · ${day(t.updated_at)}</div><h1>${e(task.title)}</h1><p>${e(task.goal)}</p>${task.explanation ? `<p class="muted">In their words: ${e(task.explanation)}</p>` : ''}</section><div class="ab-actions"><a class="button primary" href="/tasks/${e(t.id)}.mosh" download="${e(t.id)}.mosh">Download .mosh</a> <a class="small-button" href="/tasks/${e(t.id)}.json">OpenTask JSON</a>${user?.id === t.user_id ? ` <button class="small-button" type="button" data-action="unpublish" data-id="${e(t.id)}">Unpublish</button>` : ''}</div><p class="form-status" id="tasks-status" role="status" aria-live="polite"></p><section class="inspector-panel"><h3>Run it</h3><pre class="instructions">${e(`curl -fsSLO ${origin}/tasks/${t.id}.mosh
moshcode run ${t.id}.mosh --dry-run   # see what it will do
moshcode run ${t.id}.mosh             # opens claude with the task; or add: codex`)}</pre><p class="muted">Read it first: it is plain text. The agent drives the desktop through the W3BS Agentic Desktop Layer (<code>w3bs desktop setup claude-code</code>), observes before every step, and asks before anything it cannot undo.</p></section><section class="inspector-panel"><h3>Steps</h3><ol class="prose">${task.steps.map((s) => `<li>${describe(s)}${s.evidence === 'inferred' ? ' <span class="muted">(inferred)</span>' : ''}</li>`).join('')}</ol>${params}<h3>Stops when</h3><ul class="prose">${task.stop_conditions.map((c) => `<li>${e(c)}</li>`).join('')}</ul>${task.approvals.length ? `<h3>Asks</h3><ul class="prose">${task.approvals.map((c) => `<li>${e(c)}</li>`).join('')}</ul>` : ''}<h3>Done when</h3><p>${e(task.success.check)}</p><p class="muted">Apps: ${e(task.apps.map((a) => a.name).join(', '))}. Captured: ${e(task.capture?.profile ?? 'unknown')}, window titles ${e(task.capture?.window_titles ?? 'removed')}${task.capture?.gaps?.length ? `, gaps: ${e(task.capture.gaps.join(', '))}` : ''}. Recorded with ${e(task.source.recorder ?? 'W3BS Training')}.</p></section>${t.workflows.length ? `<section class="inspector-panel"><h3>In workflows</h3><ul class="prose">${t.workflows.map((w) => `<li><a class="inline-link" href="/workflows/${e(w.id)}">${e(w.title)}</a></li>`).join('')}</ul></section>` : ''}`,
    });
  });

  app.get('/workflows', async (req, res) => {
    const user = await currentUser(req);
    const [all, mine] = await Promise.all([
      tasks.workflows(),
      user ? tasks.list({ userId: user.id }) : [],
    ]);
    const rows = all.length
      ? `<div class="table-wrap"><table class="platform-table"><thead><tr><th>Workflow</th><th>Tasks</th><th>Order</th><th>Publisher</th><th>Updated</th></tr></thead><tbody>${all
          .map(
            (w) =>
              `<tr><td><a class="inline-link" href="/workflows/${e(w.id)}"><strong>${e(w.title)}</strong></a>${w.description ? `<br><span class="muted">${e(w.description.slice(0, 160))}</span>` : ''}</td><td>${w.tasks}</td><td>${e(w.order)}</td><td>${e(w.publisher)}</td><td>${day(w.updated_at)}</td></tr>`,
          )
          .join('')}</tbody></table></div>`
      : '<p class="muted">No workflows yet.</p>';
    const form = !user
      ? `<p class="muted"><a class="inline-link" href="/login?next=%2Fworkflows">Sign in</a> to collect your published tasks into a workflow, or pass <code>--workflow</code> to <code>w3bs training publish</code>.</p>`
      : mine.length
        ? `<form class="auth-form" data-form="workflow-create"><label class="input-label" for="wf-title">Title</label><input id="wf-title" name="title" maxlength="200" required placeholder="Release a package"><label class="input-label" for="wf-description">What it is for <span class="muted">(optional)</span></label><input id="wf-description" name="description" maxlength="4000"><label class="input-label" for="wf-order">Order</label><select id="wf-order" name="order"><option value="sequence">sequence: one after another, as listed</option><option value="any">any order</option></select><fieldset class="task-picks"><legend class="input-label">Your published tasks, in the order you tick them</legend>${mine.map((t) => `<label><input type="checkbox" name="tasks" value="${e(t.id)}"> ${e(t.title)}</label>`).join('<br>')}</fieldset><button class="button primary" type="submit">Create workflow</button><p class="form-status" role="status" aria-live="polite"></p></form>`
        : '<p class="muted">Publish a task first (<code>w3bs training publish</code>), then collect tasks here.</p>';
    page(res, {
      title: 'Workflows',
      description:
        'Workflows are collections of published OpenTasks, served as OpenWorkflow documents: each task by URL and digest, run in sequence or any order.',
      robots: '',
      track: true,
      scripts,
      content: `<section class="page-head"><div class="eyebrow">W3BS WORKFLOWS · OPENWORKFLOW 0.1</div><h1>Tasks that go <em>together.</em></h1><p>A workflow is a named collection of <a class="inline-link" href="/tasks">published tasks</a>: the ones people taught <a class="inline-link" href="/training">W3BS Training</a> and automated with moshscript. Each is served as an <a class="inline-link" href="${SPEC.workflow}" rel="noopener">OpenWorkflow</a> document that lists every task by URL and SHA-256, so what you reviewed cannot change underneath you. Agents find them all at <a class="inline-link" href="/.well-known/openworkflow.json"><code>/.well-known/openworkflow.json</code></a>.</p></section>${rows}<section class="inspector-panel"><h3>New workflow</h3>${form}</section>`,
    });
  });

  app.get('/workflows/:id', async (req, res) => {
    const w = await tasks.workflow(req.params.id);
    if (!w) return notFound(res, 'workflow', 'workflows');
    const user = await currentUser(req);
    const run = w.tasks
      .map((t) => `curl -fsSLO ${origin}/tasks/${t.id}.mosh && moshcode run ${t.id}.mosh`)
      .join('\n');
    page(res, {
      title: w.title,
      description: `${w.description || w.title}: an OpenWorkflow of ${w.tasks.length} task(s) on W3BS.`,
      robots: '',
      track: true,
      scripts,
      content: `<section class="page-head compact"><a class="inline-link" href="/workflows">← Workflows</a><div class="eyebrow">OPENWORKFLOW · ${e(w.order.toUpperCase())} · ${e(w.publisher)} · ${day(w.updated_at)}</div><h1>${e(w.title)}</h1>${w.description ? `<p>${e(w.description)}</p>` : ''}</section><div class="ab-actions"><a class="small-button" href="/workflows/${e(w.id)}.json">OpenWorkflow JSON</a>${user?.id === w.user_id ? ` <button class="small-button" type="button" data-action="delete-workflow" data-id="${e(w.id)}">Delete workflow</button>` : ''}</div><p class="form-status" id="tasks-status" role="status" aria-live="polite"></p><ol class="workflow-tasks prose">${w.tasks.map((t) => `<li><a class="inline-link" href="/tasks/${e(t.id)}"><strong>${e(t.title)}</strong></a> <span class="pill">${e(t.task.status)}</span><br><span class="muted">${e(t.task.goal)} · ${t.task.steps.length} step(s) · ${e(t.task.apps.map((a) => a.name).join(', '))}</span></li>`).join('')}</ol><section class="inspector-panel"><h3>Run it</h3><pre class="instructions">${e(run)}</pre><p class="muted">${w.order === 'sequence' ? 'One after another: stop the workflow if a task stops.' : 'In any order.'} Each script opens an agent on your desktop through the W3BS Agentic Desktop Layer and asks before anything it cannot undo.</p></section>`,
    });
  });
}
