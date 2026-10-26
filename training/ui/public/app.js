// W3BS Training local app. Every value from the store is rendered with
// textContent: imported and captured text is data and never becomes markup.
const API = '/api/v1/training';
const main = document.getElementById('main');
// Set from /status: true on w3bs.org/training/app (server-managed key), false for the local app.
let HOSTED = false;
const PURPOSES = [
  'human_review',
  'private_retrieval',
  'external_retrieval',
  'model_training',
  'evaluation',
  'public_distribution',
];

function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
    else if (k === 'class') el.className = v;
    else el.setAttribute(k, v === true ? '' : String(v));
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return el;
}

async function api(path, { method = 'GET', body, raw, headers = {} } = {}) {
  const response = await fetch(API + path, {
    method,
    credentials: 'same-origin',
    headers: {
      'x-w3bs-training': '1',
      ...(body !== undefined ? { 'content-type': 'application/json' } : {}),
      ...headers,
    },
    body: raw ?? (body !== undefined ? JSON.stringify(body) : undefined),
  });
  const data = response.headers.get('content-type')?.includes('json')
    ? await response.json()
    : null;
  if (!response.ok) {
    const error = new Error(data?.error?.message || response.statusText);
    error.code = data?.error?.code;
    error.details = data?.error?.details;
    throw error;
  }
  return data;
}

let toastTimer;
function toast(message, isError = false) {
  const el = document.getElementById('toast');
  el.textContent = message;
  el.className = `toast${isError ? ' error' : ''}`;
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (el.hidden = true), isError ? 8000 : 3500);
}
const run = (fn) => async (event) => {
  event?.preventDefault?.();
  try {
    await fn(event);
  } catch (error) {
    const details = Array.isArray(error.details)
      ? ` (${error.details
          .slice(0, 3)
          .map((d) =>
            typeof d === 'string'
              ? d
              : `${d.episode_id ?? ''} ${d.purpose ?? ''}: ${(d.reasons ?? [d.reason]).join(', ')}`,
          )
          .join('; ')})`
      : '';
    toast(`${error.code ? `${error.code}: ` : ''}${error.message}${details}`, true);
  }
};
const chip = (text, kind) => h('span', { class: `chip${kind ? ` ${kind}` : ''}` }, text);
const kv = (pairs) =>
  h(
    'dl',
    { class: 'kv' },
    pairs.flatMap(([k, v]) => [h('dt', {}, k), h('dd', {}, v ?? '—')]),
  );
const when = (iso) => (iso ? new Date(iso).toLocaleString() : '—');

async function refreshStatus() {
  try {
    const s = await api('/status');
    const el = document.getElementById('status');
    el.replaceChildren(
      h('strong', {}, `${s.pending_review} to review`),
      ` · ${s.released_versions} released · agent access ${s.agent_access} · key ${s.key_store} · cloud budget $${s.cloud_budget_usd}`,
    );
  } catch {
    /* status is decorative */
  }
}

// ------------------------------------------------------------------ views
const views = {
  async overview() {
    const [s, inbox] = await Promise.all([api('/status'), api('/inbox')]);
    return [
      h('h1', {}, 'Overview'),
      h('p', { class: 'muted' }, s.recorder),
      h(
        'div',
        { class: 'grid' },
        h(
          'div',
          { class: 'card' },
          h('h3', {}, 'Review'),
          h('p', {}, `${s.pending_review} task(s) waiting; today's budget shows ${inbox.budget}.`),
          h('a', { href: '#/inbox' }, 'Open inbox'),
        ),
        h(
          'div',
          { class: 'card' },
          h('h3', {}, 'Datasets'),
          h('p', {}, `${s.datasets} dataset(s), ${s.released_versions} released version(s).`),
          h('a', { href: '#/datasets' }, 'Open datasets'),
        ),
        h(
          'div',
          { class: 'card' },
          h('h3', {}, 'Agents'),
          h('p', {}, `${s.active_grants} active grant(s). Agent access is ${s.agent_access}.`),
          h('a', { href: '#/consumers' }, 'Manage consumers'),
        ),
        h(
          'div',
          { class: 'card' },
          h('h3', {}, 'Storage'),
          h(
            'p',
            {},
            `${(s.artifact_bytes / 1048576).toFixed(1)} MiB of sealed artifacts (${s.key_store}).`,
          ),
          h('a', { href: '#/settings' }, 'Retention and deletion'),
        ),
        h(
          'div',
          { class: 'card' },
          h('h3', {}, 'Costs'),
          h(
            'p',
            {},
            `Cloud budget $${s.cloud_budget_usd}. Training adapters: ${s.training_adapters}. Nothing here calls a paid model.`,
          ),
        ),
      ),
    ];
  },

  async inbox() {
    const inbox = await api('/inbox');
    const cards = inbox.cards.map((c) => {
      const decide = (decision) =>
        run(async () => {
          await api(`/episodes/${c.episode_id}/review`, {
            method: 'POST',
            body: { decision, confirm_goal: true, expected_revision: c.revision },
          });
          toast(`Saved: ${decision}`);
          render();
        });
      return h(
        'article',
        { class: `card${c.privacy_warnings ? ' warn' : ''}`, 'data-episode': c.episode_id },
        h('h3', {}, String(c.goal.value)),
        h(
          'p',
          { class: 'meta' },
          `${c.observed_steps} observed steps · ${c.uncertain_labels} uncertain labels · ${c.privacy_warnings} privacy warning(s)`,
        ),
        h(
          'div',
          { class: 'chips' },
          chip(
            `Goal: ${c.goal.status === 'confirmed' ? 'confirmed' : 'suggested, awaiting confirmation'}`,
            c.goal.status === 'confirmed' ? 'ok' : '',
          ),
          chip(`Result: ${c.outcome.value} (${c.outcome.status})`),
          chip(c.capture_profile),
          c.privacy_warnings ? chip('Privacy warning: review before approval', 'warn') : null,
          ...c.reasons
            .filter(
              (r) => !['goal unconfirmed', 'outcome unconfirmed', 'privacy warning'].includes(r),
            )
            .map((r) => chip(r)),
        ),
        h(
          'div',
          { class: 'actions' },
          h(
            'button',
            { class: 'primary', onclick: decide('complete'), title: 'Shortcut c' },
            'Complete',
          ),
          h('button', { onclick: decide('partial') }, 'Partial'),
          h('button', { onclick: decide('failed') }, 'Failed'),
          h('button', { onclick: decide('unknown') }, 'Unknown'),
          h(
            'button',
            {
              onclick: run(async () => {
                const goal = prompt('Goal for this task', String(c.goal.value));
                if (goal === null) return;
                await api(`/episodes/${c.episode_id}/review`, {
                  method: 'POST',
                  body: { decision: 'rename', goal, expected_revision: c.revision },
                });
                render();
              }),
            },
            'Rename',
          ),
          h('a', { href: `#/episode/${c.episode_id}`, class: 'button-link' }, 'Review details'),
          h(
            'button',
            {
              class: 'danger',
              onclick: run(async () => {
                if (!confirm('Discard this task and delete the frames only it uses?')) return;
                await api(`/episodes/${c.episode_id}/review`, {
                  method: 'POST',
                  body: { decision: 'discard' },
                });
                toast('Discarded and deleted');
                render();
              }),
            },
            'Discard',
          ),
        ),
      );
    });
    const approvals = inbox.approval_requests.length
      ? [
          h('h2', {}, 'Agent requests'),
          h(
            'p',
            { class: 'muted' },
            'Approving records your decision only. Do the action yourself afterwards; nothing an agent sent runs automatically.',
          ),
          ...inbox.approval_requests.map((r) =>
            h(
              'div',
              { class: 'card' },
              h('p', {}, `${r.requested_by} asks: ${r.kind}`),
              h(
                'div',
                { class: 'actions' },
                h(
                  'button',
                  {
                    onclick: run(async () => {
                      await api(`/approvals/${r.request_id}`, {
                        method: 'POST',
                        body: { approve: true },
                      });
                      render();
                    }),
                  },
                  'Approve',
                ),
                h(
                  'button',
                  {
                    onclick: run(async () => {
                      await api(`/approvals/${r.request_id}`, {
                        method: 'POST',
                        body: { approve: false },
                      });
                      render();
                    }),
                  },
                  'Deny',
                ),
              ),
            ),
          ),
        ]
      : [];
    const runs = inbox.runs_to_review.length
      ? [
          h('h2', {}, 'Agent runs to review'),
          h(
            'p',
            { class: 'muted' },
            'Failed or uncertain runs never become trusted examples on their own.',
          ),
          ...inbox.runs_to_review.map((r) =>
            h(
              'div',
              { class: 'card' },
              h('p', {}, `${r.consumer}: ${r.outcome} (${when(r.created_at)})`),
              h(
                'button',
                {
                  onclick: run(async () => {
                    await api(`/runs/${r.run_id}/review`, {
                      method: 'POST',
                      body: { decision: 'acknowledged' },
                    });
                    render();
                  }),
                },
                'Acknowledge',
              ),
            ),
          ),
        ]
      : [];
    return [
      h('h1', {}, 'Inbox'),
      h(
        'p',
        { class: 'muted' },
        `${inbox.total} task(s) need review; showing ${cards.length} (daily budget ${inbox.budget}). Unreviewed tasks stay unapproved and expire.`,
      ),
      cards.length ? h('div', { class: 'grid' }, cards) : h('p', {}, 'Nothing to review.'),
      ...approvals,
      ...runs,
    ];
  },

  async sessions() {
    const [episodes, imports] = await Promise.all([api('/episodes?limit=200'), api('/imports')]);
    const file = h('input', {
      type: 'file',
      accept: '.zip,.json,.jsonl,.webm,.mp4',
      'aria-label': 'File to import',
    });
    const mapping = h('textarea', {
      placeholder:
        '{"timestamp":"ts","timestamp_unit":"ms","action_type":"event","target_name":"label","app":"app","episode_key":"task"}',
      'aria-label': 'Mapping for JSON or JSONL',
    });
    const preview = h('div');
    const upload = run(async () => {
      const f = file.files[0];
      if (!f) return toast('Choose a file first.', true);
      const meta = mapping.value.trim() ? { mapping: JSON.parse(mapping.value) } : {};
      const result = await api('/imports', {
        method: 'POST',
        raw: f,
        headers: {
          'x-filename': f.name,
          'x-w3bs-import': JSON.stringify(meta),
          'content-type': 'application/octet-stream',
        },
      });
      const p = result.preview;
      preview.replaceChildren(
        h('h3', {}, 'Preview'),
        kv([
          ['Profile', p.capture_profile],
          [
            'Records',
            `${p.counts.episodes} episodes, ${p.counts.actions} actions, ${p.counts.observations} observations, ${p.counts.artifacts} artifacts`,
          ],
          ['Rights', `${p.rights.source_rights}; permitted: ${p.rights.permitted_uses.join(', ')}`],
          [
            'Privacy',
            `${p.privacy.blocking_warnings} blocking warning(s); ${p.privacy.text_values_removed} text value(s) removed`,
          ],
          ['Alignment', `${p.alignment.verified} verified, ${p.alignment.uncertain} uncertain`],
        ]),
        ...p.notes.map((n) => h('p', { class: 'notice' }, n)),
        h(
          'button',
          {
            class: 'primary',
            onclick: run(async () => {
              await api(`/imports/${result.import_id}/complete`, { method: 'POST' });
              toast('Imported. New tasks are in the inbox.');
              render();
            }),
          },
          'Commit import',
        ),
      );
    });
    return [
      h('h1', {}, 'Sessions'),
      h(
        'div',
        { class: 'split' },
        h(
          'section',
          {},
          h('h2', {}, 'Episodes'),
          h(
            'div',
            { class: 'scroll' },
            h(
              'table',
              {},
              h(
                'thead',
                {},
                h(
                  'tr',
                  {},
                  h('th', {}, 'Task'),
                  h('th', {}, 'State'),
                  h('th', {}, 'Result'),
                  h('th', {}, 'Steps'),
                  h('th', {}, 'Captured'),
                ),
              ),
              h(
                'tbody',
                {},
                episodes.map((e) =>
                  h(
                    'tr',
                    {},
                    h('td', {}, h('a', { href: `#/episode/${e.episode_id}` }, String(e.goal))),
                    h('td', {}, e.state),
                    h('td', {}, e.outcome),
                    h('td', {}, e.steps),
                    h('td', {}, when(e.captured_from)),
                  ),
                ),
              ),
            ),
          ),
        ),
        h(
          'section',
          {},
          h('h2', {}, 'Import'),
          h(
            'p',
            { class: 'muted' },
            'A W3BS bundle (.zip), a JSON/JSONL event file with a mapping, or a video. Files are quarantined and validated; nothing in them is executed.',
          ),
          h('label', {}, h('span', {}, 'File'), file),
          h('label', {}, h('span', {}, 'Mapping (JSON/JSONL only)'), mapping),
          h('button', { class: 'primary', onclick: upload }, 'Validate and preview'),
          preview,
          h('h3', {}, 'Recent imports'),
          h(
            'ul',
            {},
            imports.slice(0, 10).map((i) => h('li', {}, `${i.filename}: ${i.status}`)),
          ),
        ),
      ),
    ];
  },

  async episode(id) {
    const e = await api(`/episodes/${id}`);
    const body = e.episode;
    const frames = e.observations.filter((o) => o.artifact_id);
    const imgs = h('div', { class: 'frames' });
    for (const o of frames.slice(0, 40)) {
      const fig = h('figure', {});
      imgs.append(fig);
      fetch(`${API}/artifacts/${o.artifact_id}`, { credentials: 'same-origin' }).then(async (r) => {
        if (!r.ok) return;
        const url = URL.createObjectURL(await r.blob());
        const media = r.headers.get('content-type').startsWith('video/')
          ? h('video', { src: url, controls: true })
          : h('img', {
              src: url,
              alt: `Frame ${o.observation_id}${o.semantic?.window ? `: ${o.semantic.window}` : ''}`,
            });
        fig.append(
          media,
          h(
            'figcaption',
            {},
            `${o.semantic?.app ?? ''} ${o.pre_action_verified ? '· pre-action' : ''}`,
          ),
        );
      });
    }
    const goal = h('input', { value: String(body.goal.value), 'aria-label': 'Goal' });
    const explanation = h(
      'textarea',
      { 'aria-label': 'Your explanation (optional)' },
      body.explanation?.text ?? '',
    );
    const family = h('input', {
      value: body.workflow_family ?? '',
      'aria-label': 'Workflow family',
    });
    const save = (input) =>
      run(async () => {
        await api(`/episodes/${id}/review`, {
          method: 'POST',
          body: { ...input, expected_revision: e.revision },
        });
        toast('Saved');
        render();
      });
    const open = e.warnings.filter((w) => !w.resolved_at && w.blocking);
    const runnable = body.outcome.value === 'success' && e.state === 'reviewed';
    const runHint = h('p', { class: 'muted', hidden: true });
    const exportMosh = run(async () => {
      const response = await fetch(`${API}/episodes/${id}/moshscript`, {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'x-w3bs-training': '1' },
      });
      if (!response.ok) {
        const data = await response.json().catch(() => null);
        const err = new Error(data?.error?.message || response.statusText);
        err.code = data?.error?.code;
        throw err;
      }
      const filename =
        /filename="([^"]+)"/.exec(response.headers.get('content-disposition') || '')?.[1] ||
        'task.mosh';
      const url = URL.createObjectURL(await response.blob());
      const link = h('a', { href: url, download: filename });
      document.body.append(link);
      link.click();
      link.remove();
      setTimeout(() => URL.revokeObjectURL(url), 10000);
      runHint.replaceChildren(
        'Saved ',
        h('code', {}, filename),
        '. Run it with ',
        h('code', {}, `moshcode run ${filename}`),
        ' (add ',
        h('code', {}, '--dry-run'),
        ' to see what it does first). The copy lives outside W3BS.',
      );
      runHint.hidden = false;
      toast(`Exported ${filename}`);
    });
    // Publishing makes the task public on w3bs.org/tasks; window titles stay home by default.
    const publishBox = h('div', { class: 'notice publish-box', hidden: true });
    const keepTitles = h('input', { type: 'checkbox', id: 'publish-titles' });
    const workflowName = h('input', {
      id: 'publish-workflow',
      maxlength: '200',
      placeholder: 'Workflow (optional), e.g. Release a package',
    });
    const publishResult = h('p', { class: 'muted', hidden: true });
    const publish = run(async () => {
      const result = await api(`/episodes/${id}/publish`, {
        method: 'POST',
        body: {
          window_titles: keepTitles.checked,
          workflow: workflowName.value.trim() || undefined,
        },
      });
      publishResult.replaceChildren(
        'Published: ',
        h('a', { href: result.url, target: '_blank', rel: 'noopener' }, result.url),
        result.workflow ? ' · in ' : '',
        result.workflow
          ? h('a', { href: result.workflow.url, target: '_blank', rel: 'noopener' }, 'the workflow')
          : '',
        '. Unpublish there, or with ',
        h('code', {}, `w3bs training unpublish ${result.id}`),
        '.',
      );
      publishResult.hidden = false;
      toast(result.republished ? 'Republished' : 'Published');
    });
    publishBox.append(
      h('strong', {}, 'Publish to w3bs.org/tasks'),
      h(
        'p',
        {},
        'Anyone can read the goal, your explanation, the app names and the steps. No screenshots, coordinates or typed text leave this computer. Deleting the task here does not unpublish it.',
      ),
      h(
        'label',
        { for: 'publish-titles' },
        keepTitles,
        ' Keep window titles (they can show addresses, subjects and order numbers)',
      ),
      workflowName,
      h('button', { class: 'primary', onclick: publish }, 'Publish'),
      publishResult,
    );
    return [
      h('p', {}, h('a', { href: '#/sessions' }, '← Sessions')),
      h('h1', {}, String(body.goal.value)),
      h(
        'div',
        { class: 'chips' },
        chip(e.state),
        chip(body.capture_profile),
        chip(`goal ${body.goal.status} (${body.goal.source})`),
        chip(`result ${body.outcome.value} (${body.outcome.status})`),
        e.pinned ? chip('pinned', 'ok') : null,
      ),
      h(
        'div',
        { class: 'actions episode-actions' },
        h(
          'button',
          {
            class: 'primary',
            disabled: !runnable,
            title: runnable
              ? 'Download this task as a .mosh script for moshcode'
              : 'Mark the result complete first: only reviewed, successful tasks export',
            onclick: exportMosh,
          },
          'Export as moshscript',
        ),
        h(
          'button',
          {
            disabled: !runnable,
            title: runnable
              ? 'Publish this task as an OpenTask on w3bs.org/tasks, with a moshscript to run it'
              : 'Mark the result complete first: only reviewed, successful tasks publish',
            onclick: () => (publishBox.hidden = !publishBox.hidden),
          },
          'Publish to w3bs.org/tasks…',
        ),
        runnable ? null : h('span', { class: 'muted' }, 'Mark the result complete to export.'),
      ),
      runHint,
      publishBox,
      open.length
        ? h(
            'div',
            { class: 'notice' },
            h('strong', {}, 'Privacy warnings'),
            h(
              'ul',
              {},
              open.map((w) => h('li', {}, `${w.kind} at ${w.path}`)),
            ),
            h(
              'button',
              { onclick: save({ decision: 'clear_privacy' }) },
              'I reviewed these: clear warnings',
            ),
          )
        : null,
      e.gaps.length
        ? h(
            'div',
            { class: 'notice' },
            `${e.gaps.length} capture gap(s): ${[...new Set(e.gaps.map((g) => g.kind))].join(', ')}. Missing data is shown, never filled in.`,
          )
        : null,
      frames.length ? [h('h2', {}, 'Frames'), imgs] : null,
      h(
        'div',
        { class: 'split' },
        h(
          'section',
          {},
          h('h2', {}, 'Steps'),
          h(
            'ol',
            { class: 'timeline' },
            e.steps.map((s, n) =>
              h(
                'li',
                {},
                h('span', { class: 'n' }, n + 1),
                h(
                  'div',
                  {},
                  h('strong', {}, s.action.type),
                  ' ',
                  [s.action.target?.app, s.action.target?.role, s.action.target?.name]
                    .filter(Boolean)
                    .join(' / '),
                  h(
                    'div',
                    { class: 'chips' },
                    chip(
                      s.action.status === 'confirmed' ? 'observed' : 'inferred',
                      s.action.status === 'confirmed' ? 'ok' : 'warn',
                    ),
                    chip(`origin ${s.action.origin}`),
                    chip(
                      `alignment ${s.alignment.status}`,
                      s.alignment.status === 'verified' ? '' : 'warn',
                    ),
                    s.action.arguments.text_placeholder
                      ? chip(`text: ${s.action.arguments.text_placeholder}`)
                      : null,
                  ),
                  n > 0
                    ? h(
                        'button',
                        {
                          onclick: run(async () => {
                            await api(`/episodes/${id}/split`, {
                              method: 'POST',
                              body: { at_step: s.step_id, expected_revision: e.revision },
                            });
                            toast('Split');
                            render();
                          }),
                        },
                        'Split here',
                      )
                    : null,
                  h(
                    'button',
                    { onclick: save({ decision: 'exclude_steps', steps: [s.step_id] }) },
                    'Exclude step',
                  ),
                ),
              ),
            ),
          ),
        ),
        h(
          'section',
          {},
          h('h2', {}, 'Labels'),
          h('label', {}, h('span', {}, 'Goal'), goal),
          h(
            'button',
            { onclick: run(async () => save({ decision: 'rename', goal: goal.value })()) },
            'Save goal',
          ),
          h('label', {}, h('span', {}, 'Workflow family'), family),
          h(
            'button',
            { onclick: run(async () => save({ decision: 'family', family: family.value })()) },
            'Save family',
          ),
          h('h3', {}, 'Result'),
          h(
            'div',
            { class: 'actions' },
            ['complete', 'partial', 'failed', 'interrupted', 'unknown'].map((d) =>
              h('button', { onclick: save({ decision: d, confirm_goal: true }) }, d),
            ),
          ),
          h('label', {}, h('span', {}, 'Your explanation (your words, optional)'), explanation),
          h(
            'button',
            {
              onclick: run(async () =>
                save({ decision: 'annotate', explanation: explanation.value })(),
              ),
            },
            'Save explanation',
          ),
          h('h3', {}, 'Keep or delete'),
          h(
            'div',
            { class: 'actions' },
            h(
              'button',
              { onclick: save({ decision: e.pinned ? 'unpin' : 'pin' }) },
              e.pinned ? 'Unpin' : 'Pin past retention',
            ),
            body.outcome.value === 'success' && e.state === 'reviewed'
              ? h(
                  'button',
                  {
                    onclick: run(async () => {
                      await api('/workflows', { method: 'POST', body: { episode_id: id } });
                      toast('Workflow created (untested)');
                    }),
                  },
                  'Create workflow',
                )
              : null,
            h(
              'button',
              {
                class: 'danger',
                onclick: run(async () => {
                  if (!confirm('Delete this task permanently?')) return;
                  const report = await api('/deletion-requests', {
                    method: 'POST',
                    body: { episodes: [id] },
                  });
                  toast(
                    `Deleted. ${report.exports_outside_w3bs.length} exported copy(ies) are outside W3BS and cannot be recalled.`,
                  );
                  location.hash = '#/sessions';
                }),
              },
              'Delete',
            ),
          ),
          h('h3', {}, 'History'),
          h(
            'ul',
            {},
            e.labels.map((l) =>
              h(
                'li',
                {},
                `${l.field_path} = ${JSON.stringify(l.value)} · ${l.status} by ${l.author.kind}:${l.author.id} (${l.source})`,
              ),
            ),
          ),
        ),
      ),
    ];
  },

  async datasets() {
    const list = await api('/datasets');
    const name = h('input', { 'aria-label': 'Dataset name', placeholder: 'Release workflows' });
    return [
      h('h1', {}, 'Datasets'),
      h(
        'div',
        { class: 'row' },
        h('label', {}, h('span', {}, 'New dataset'), name),
        h(
          'button',
          {
            class: 'primary',
            onclick: run(async () => {
              const d = await api('/datasets', { method: 'POST', body: { name: name.value } });
              location.hash = `#/dataset/${d.dataset_id}`;
            }),
          },
          'Create',
        ),
      ),
      h(
        'div',
        { class: 'grid' },
        list.map((d) =>
          h(
            'div',
            { class: 'card' },
            h('h3', {}, h('a', { href: `#/dataset/${d.dataset_id}` }, d.name)),
            h('p', { class: 'meta' }, `${d.episodes} episodes · ${d.versions.length} version(s)`),
            h(
              'div',
              { class: 'chips' },
              d.versions.map((v) =>
                chip(
                  `v${v.version} ${v.state}`,
                  v.state === 'released' ? 'ok' : v.state === 'revoked' ? 'warn' : '',
                ),
              ),
            ),
          ),
        ),
      ),
    ];
  },

  async dataset(id) {
    const [d, reviewed] = await Promise.all([
      api(`/datasets/${id}`),
      api('/episodes?state=reviewed&limit=500'),
    ]);
    const members = new Set(d.episodes.map((e) => e.episode_id));
    const candidates = reviewed.filter((e) => !members.has(e.episode_id));
    const pick = h(
      'select',
      {
        multiple: true,
        size: Math.min(8, Math.max(2, candidates.length)),
        'aria-label': 'Reviewed episodes to add',
      },
      candidates.map((e) => h('option', { value: e.episode_id }, `${e.goal} (${e.outcome})`)),
    );
    const r = d.readiness;
    return [
      h('p', {}, h('a', { href: '#/datasets' }, '← Datasets')),
      h('h1', {}, d.name),
      h('h2', {}, 'Readiness'),
      kv([
        ['Episodes', r.episodes],
        ...PURPOSES.map((p) => [p.replace('_', ' '), `${r.admissible_by_purpose[p]} admissible`]),
        [
          'Action supervision',
          `${r.profiles['action-supervision'].eligible_steps}/${r.profiles['action-supervision'].steps} steps eligible`,
        ],
      ]),
      h(
        'p',
        { class: 'muted' },
        '"Ready for human review" is not "ready for action-model training": each purpose is checked separately.',
      ),
      h('h2', {}, 'Members'),
      h(
        'ul',
        {},
        d.episodes.map((e) =>
          h(
            'li',
            {},
            h('a', { href: `#/episode/${e.episode_id}` }, String(e.goal)),
            ` · ${e.state} · ${e.outcome} `,
            h(
              'button',
              {
                onclick: run(async () => {
                  await api(`/datasets/${id}/episodes`, {
                    method: 'POST',
                    body: { remove: [e.episode_id] },
                  });
                  render();
                }),
              },
              'Remove',
            ),
          ),
        ),
      ),
      candidates.length
        ? h(
            'div',
            { class: 'row' },
            h('label', {}, h('span', {}, 'Add reviewed episodes'), pick),
            h(
              'button',
              {
                onclick: run(async () => {
                  await api(`/datasets/${id}/episodes`, {
                    method: 'POST',
                    body: { add: [...pick.selectedOptions].map((o) => o.value) },
                  });
                  render();
                }),
              },
              'Add',
            ),
          )
        : h('p', { class: 'muted' }, 'Every reviewed episode is already a member.'),
      h('h2', {}, 'Versions'),
      h(
        'button',
        {
          class: 'primary',
          onclick: run(async () => {
            const v = await api(`/datasets/${id}/versions`, { method: 'POST', body: {} });
            toast(
              `Version ${v.version} created; ${v.excluded.length} unreviewed member(s) left out`,
            );
            render();
          }),
        },
        'Freeze a new version',
      ),
      ...d.versions.map((v) => versionCard(id, v)),
      h('h2', {}, 'Delete'),
      h(
        'button',
        {
          class: 'danger',
          onclick: run(async () => {
            const plan = await api(`/datasets/${id}`, { method: 'DELETE' });
            if (
              !confirm(
                `Delete "${plan.name}", ${plan.versions.length} version(s) and revoke ${plan.grants_revoked} grant(s)? Episodes stay. ${plan.exports_outside_w3bs.length} export(s) cannot be recalled.`,
              )
            )
              return;
            await api(`/datasets/${id}?confirm=1`, { method: 'DELETE' });
            location.hash = '#/datasets';
          }),
        },
        'Delete dataset',
      ),
    ];
  },

  async workflows() {
    const list = await api('/workflows');
    return [
      h('h1', {}, 'Workflows'),
      h(
        'p',
        { class: 'muted' },
        'A workflow is a reviewable procedure derived from one successful, reviewed episode. It stays untested until an evaluation passes, and it grants no permission to act.',
      ),
      list.length
        ? h(
            'div',
            { class: 'grid' },
            list.map((w) =>
              h(
                'div',
                { class: 'card' },
                h('h3', {}, String(w.purpose)),
                chip(w.status, 'warn'),
                h(
                  'ol',
                  {},
                  w.steps.map((s) =>
                    h('li', {}, `${s.type} ${s.target ? Object.values(s.target).join(' / ') : ''}`),
                  ),
                ),
                h('p', { class: 'meta' }, `Stops if: ${w.stop_conditions.join('; ')}`),
              ),
            ),
          )
        : h(
            'p',
            {},
            'No workflows yet. Open a reviewed, successful episode and choose Create workflow.',
          ),
    ];
  },

  async consumers() {
    const [grants, status, datasets] = await Promise.all([
      api('/grants'),
      api('/status'),
      api('/datasets'),
    ]);
    const released = datasets.flatMap((d) =>
      d.versions
        .filter((v) => v.state === 'released')
        .map((v) => ({ ...v, dataset_id: d.dataset_id, name: d.name })),
    );
    const consumer = h('input', { 'aria-label': 'Agent name', placeholder: 'claude-code' });
    const version = h(
      'select',
      { 'aria-label': 'Released version' },
      released.map((v) =>
        h('option', { value: `${v.dataset_id}@${v.version}` }, `${v.name} v${v.version}`),
      ),
    );
    const purpose = h(
      'select',
      { 'aria-label': 'Purpose' },
      h('option', { value: 'private_retrieval' }, 'private retrieval'),
      h('option', { value: 'external_retrieval' }, 'external retrieval'),
    );
    const days = h('input', { type: 'number', min: 1, max: 365, value: 30, 'aria-label': 'Days' });
    return [
      h('h1', {}, 'Consumers'),
      h(
        'div',
        { class: 'notice' },
        `Agent access is ${status.agent_access}. `,
        h(
          'button',
          {
            onclick: run(async () => {
              await api('/access', { method: 'POST', body: { on: status.agent_access !== 'on' } });
              render();
            }),
          },
          status.agent_access === 'on' ? 'Switch off every agent' : 'Switch agent access on',
        ),
      ),
      h('h2', {}, 'Share with an agent'),
      (() => {
        const who = h('input', { 'aria-label': 'Agent name', value: 'claude-code' });
        return h(
          'div',
          {},
          h(
            'p',
            { class: 'muted' },
            'One click: every reviewed task goes into a "Reviewed tasks" dataset, released for private retrieval and granted to this agent for 30 days. Click again after reviewing more.',
          ),
          h(
            'div',
            { class: 'row' },
            who,
            h(
              'button',
              {
                class: 'primary',
                onclick: run(async () => {
                  const r = await api('/share', { method: 'POST', body: { consumer: who.value } });
                  toast(
                    `Shared ${r.episodes} reviewed task(s) with ${r.consumer} until ${r.expires_at.slice(0, 10)}`,
                  );
                  render();
                }),
              },
              'Share all reviewed tasks',
            ),
          ),
        );
      })(),
      h('h2', {}, 'Grant retrieval'),
      released.length
        ? h(
            'div',
            { class: 'row' },
            h('label', {}, h('span', {}, 'Agent'), consumer),
            h('label', {}, h('span', {}, 'Version'), version),
            h('label', {}, h('span', {}, 'Purpose'), purpose),
            h('label', {}, h('span', {}, 'Days'), days),
            h(
              'button',
              {
                class: 'primary',
                onclick: run(async () => {
                  const [dataset_id, v] = version.value.split('@');
                  await api('/grants', {
                    method: 'POST',
                    body: {
                      consumer: consumer.value,
                      dataset_id,
                      version: Number(v),
                      purpose: purpose.value,
                      days: Number(days.value),
                    },
                  });
                  toast('Granted');
                  render();
                }),
              },
              'Grant',
            ),
          )
        : h('p', { class: 'muted' }, 'Release a dataset version first.'),
      h('h2', {}, 'Connect an agent'),
      HOSTED
        ? await agentTokens()
        : h(
            'pre',
            {},
            [
              '# Claude Code',
              'claude mcp add w3bs-training -- w3bs training mcp serve --stdio --consumer claude-code',
              '# Claude (desktop app): prints what to add to claude_desktop_config.json',
              'w3bs training setup claude --consumer claude',
              '# Codex',
              'w3bs training setup codex --consumer codex',
              '# OpenCode',
              'w3bs training setup opencode --consumer opencode',
            ].join('\n'),
          ),
      h(
        'p',
        { class: 'muted' },
        'MCP access reads approved demonstrations only. It does not make the model behind a client trainable, and it grants no desktop actions.',
      ),
      h('h2', {}, 'Grants'),
      h(
        'div',
        { class: 'scroll' },
        h(
          'table',
          {},
          h(
            'thead',
            {},
            h(
              'tr',
              {},
              ['Agent', 'Dataset', 'Purpose', 'Expires', ''].map((t) => h('th', {}, t)),
            ),
          ),
          h(
            'tbody',
            {},
            grants.map((g) =>
              h(
                'tr',
                {},
                h('td', {}, g.consumer),
                h('td', {}, `${g.dataset_id}@${g.version}`),
                h('td', {}, g.purpose),
                h('td', {}, g.revoked_at ? `revoked ${when(g.revoked_at)}` : when(g.expires_at)),
                h(
                  'td',
                  {},
                  g.revoked_at
                    ? ''
                    : h(
                        'button',
                        {
                          onclick: run(async () => {
                            await api(`/grants/${g.grant_id}`, { method: 'DELETE' });
                            render();
                          }),
                        },
                        'Revoke',
                      ),
                ),
              ),
            ),
          ),
        ),
      ),
    ];
  },

  async runs() {
    const [runs, evaluation] = await Promise.all([api('/runs?limit=200'), api('/evaluations')]);
    const pct = (x) => (x === null || x === undefined ? '—' : `${(x * 100).toFixed(0)}%`);
    const arm = (name, a) => [
      name,
      `${a.runs} runs · success ${pct(a.success_rate.rate)} (95% CI ${pct(a.success_rate.low)}–${pct(a.success_rate.high)}) · ${a.task_families} families · ${a.unsafe_attempts} unsafe attempts`,
    ];
    return [
      h('h1', {}, 'Runs'),
      h('h2', {}, 'With versus without demonstrations'),
      kv([
        arm('With demonstrations', evaluation.with_demonstrations),
        arm('Baseline', evaluation.baseline),
        [
          'Difference',
          evaluation.difference
            ? `${evaluation.difference.percentage_points.toFixed(1)} pp (${evaluation.difference.low.toFixed(1)} to ${evaluation.difference.high.toFixed(1)})`
            : '—',
        ],
      ]),
      h('p', { class: 'notice' }, evaluation.verdict),
      h('h2', {}, 'Recent runs'),
      h(
        'div',
        { class: 'scroll' },
        h(
          'table',
          {},
          h(
            'thead',
            {},
            h(
              'tr',
              {},
              ['When', 'Agent', 'Kind', 'Condition', 'Outcome', 'Examples used'].map((t) =>
                h('th', {}, t),
              ),
            ),
          ),
          h(
            'tbody',
            {},
            runs.map((r) =>
              h(
                'tr',
                {},
                h('td', {}, when(r.created_at)),
                h('td', {}, r.consumer.name),
                h('td', {}, r.kind),
                h('td', {}, r.condition ?? '—'),
                h(
                  'td',
                  {},
                  `${r.outcome}${r.review_state === 'review_required' ? ' (review)' : ''}`,
                ),
                h('td', {}, r.used_episodes.length),
              ),
            ),
          ),
        ),
      ),
    ];
  },

  async settings() {
    const status = await api('/status');
    const forget = (minutes) =>
      run(async () => {
        if (
          !confirm(
            `Delete everything captured in the last ${minutes >= 60 ? `${minutes / 60} hour(s)` : `${minutes} minutes`}?`,
          )
        )
          return;
        const report = await api('/deletion-requests', {
          method: 'POST',
          body: { from: new Date(Date.now() - minutes * 60000).toISOString() },
        });
        toast(
          `Deleted ${report.removed.episodes} task(s), ${report.removed.artifacts} frame(s). ${report.exports_outside_w3bs.length} export(s) cannot be recalled.`,
        );
        render();
      });
    return [
      h('h1', {}, 'Settings'),
      kv([
        [
          'Store',
          HOSTED ? 'w3bs.org, your account only (PostgreSQL, one schema per account)' : status.home,
        ],
        ['Key store', status.key_store],
        ['Schema', `W3BS Demonstration Dataset Format ${status.schema_version} (community draft)`],
        ['Cloud inference', 'off: budget $0'],
        ['Review budget', `${status.review_budget} cards per day`],
      ]),
      h('h2', {}, 'Delete recent activity'),
      h(
        'p',
        { class: 'muted' },
        'Deletes by capture time: every task, frame, event, label and index entry in the window, and uploads not yet committed. This works even when the desktop layer is disabled.',
      ),
      h(
        'div',
        { class: 'actions' },
        h('button', { class: 'danger', onclick: forget(10) }, 'Last 10 minutes'),
        h('button', { class: 'danger', onclick: forget(60) }, 'Last hour'),
        h('button', { class: 'danger', onclick: forget(1440) }, 'Last 24 hours'),
      ),
      h('h2', {}, 'Retention'),
      h(
        'p',
        {},
        'Unreviewed recordings expire after 7 days; approved private demonstrations after 90 days unless pinned. Pinning never overrides a deletion request.',
      ),
      h('h2', {}, 'What deletion cannot do'),
      h(
        'p',
        {},
        'Copies you export leave W3BS. Revoking a version or deleting tasks here cannot recall them or reverse a model trained from them.',
      ),
    ];
  },
};

// Hosted only: MCP over HTTP with per-agent tokens, made and revoked here.
async function agentTokens() {
  const tokens = await api('/agent-tokens');
  const name = h('input', { 'aria-label': 'Agent name for the token', placeholder: 'claude-code' });
  const shown = h('div');
  return h(
    'div',
    {},
    h(
      'p',
      { class: 'muted' },
      'An agent connects to this workspace over MCP with its own token. It sees only what you grant it above, and nothing else in your account.',
    ),
    h(
      'div',
      { class: 'row' },
      h('label', {}, h('span', {}, 'Agent name'), name),
      h(
        'button',
        {
          class: 'primary',
          onclick: run(async () => {
            const r = await api('/agent-tokens', {
              method: 'POST',
              body: { consumer: name.value },
            });
            shown.replaceChildren(
              h(
                'div',
                { class: 'notice' },
                h('strong', {}, 'Copy this now; it is shown once.'),
                h('p', {}, 'Claude Code:'),
                h('pre', {}, r.claude_code),
                h('p', {}, 'Claude (desktop app), in claude_desktop_config.json:'),
                h('pre', {}, JSON.stringify(r.claude_desktop, null, 2)),
                h('p', { class: 'muted' }, `MCP endpoint: ${r.mcp_url}. ${r.note}`),
              ),
            );
          }),
        },
        'Create agent token',
      ),
    ),
    shown,
    tokens.length
      ? h(
          'div',
          { class: 'scroll' },
          h(
            'table',
            {},
            h(
              'thead',
              {},
              h(
                'tr',
                {},
                ['Agent', 'Created', 'Last used', ''].map((t) => h('th', {}, t)),
              ),
            ),
            h(
              'tbody',
              {},
              tokens.map((t) =>
                h(
                  'tr',
                  {},
                  h('td', {}, t.consumer),
                  h('td', {}, when(t.created_at)),
                  h(
                    'td',
                    {},
                    t.revoked_at ? `revoked ${when(t.revoked_at)}` : when(t.last_used_at),
                  ),
                  h(
                    'td',
                    {},
                    t.revoked_at
                      ? ''
                      : h(
                          'button',
                          {
                            onclick: run(async () => {
                              await api(`/agent-tokens/${t.id}`, { method: 'DELETE' });
                              render();
                            }),
                          },
                          'Revoke',
                        ),
                  ),
                ),
              ),
            ),
          ),
        )
      : null,
  );
}

function versionCard(id, v) {
  const boxes = PURPOSES.map((p) =>
    h(
      'label',
      {},
      h('input', {
        type: 'checkbox',
        value: p,
        checked: v.purposes.includes(p) || undefined,
        disabled: v.purposes.includes(p) || undefined,
      }),
      p.replace('_', ' '),
    ),
  );
  const format = h(
    'select',
    { 'aria-label': 'Export format' },
    ['bundle', 'jsonl', 'next-action'].map((f) => h('option', { value: f }, f)),
  );
  // An export is for one approved purpose; the service refuses any other.
  const exportPurpose = h(
    'select',
    { 'aria-label': 'Export purpose' },
    v.purposes.map((p) => h('option', { value: p }, p.replace('_', ' '))),
  );
  const out = HOSTED
    ? null
    : h('input', {
        'aria-label': 'Export path',
        placeholder: '/home/me/exports/release-v1',
      });
  return h(
    'div',
    { class: 'card' },
    h('h3', {}, `Version ${v.version}`),
    h(
      'div',
      { class: 'chips' },
      chip(v.state, v.state === 'released' ? 'ok' : v.state === 'revoked' ? 'warn' : ''),
      ...v.purposes.map((p) => chip(`approved: ${p}`, 'ok')),
    ),
    h('p', { class: 'meta' }, `Digest ${v.digest.slice(0, 16)}… · created ${when(v.created_at)}`),
    v.state === 'draft'
      ? [
          h('fieldset', { class: 'checks' }, h('legend', {}, 'Approve for'), boxes),
          h(
            'div',
            { class: 'actions' },
            h(
              'button',
              {
                onclick: run(async () => {
                  const purposes = boxes
                    .map((b) => b.querySelector('input'))
                    .filter((i) => i.checked && !i.disabled)
                    .map((i) => i.value);
                  if (!purposes.length) return toast('Pick at least one purpose.', true);
                  await api(`/datasets/${id}/versions/${v.version}/approve`, {
                    method: 'POST',
                    body: { purposes },
                  });
                  toast('Approved');
                  render();
                }),
              },
              'Approve selected',
            ),
            h(
              'button',
              {
                class: 'primary',
                disabled: !v.purposes.length || undefined,
                onclick: run(async () => {
                  await api(`/datasets/${id}/versions/${v.version}/release`, { method: 'POST' });
                  toast('Released');
                  render();
                }),
              },
              'Release',
            ),
          ),
        ]
      : null,
    v.state === 'released'
      ? [
          h(
            'div',
            { class: 'row' },
            h('label', {}, h('span', {}, 'Format'), format),
            h('label', {}, h('span', {}, 'For'), exportPurpose),
            out ? h('label', {}, h('span', {}, 'Export to'), out) : null,
            h(
              'button',
              {
                onclick: run(async () => {
                  if (
                    !confirm(
                      'The exported copy will live outside W3BS. Disabling the desktop layer or revoking this version cannot recall it. Export?',
                    )
                  )
                    return;
                  const body = {
                    dataset_id: id,
                    version: v.version,
                    format: format.value,
                    purpose: exportPurpose.value,
                    ...(out ? { out: out.value } : {}),
                  };
                  if (HOSTED) {
                    // The server answers with the zip; save it from the browser.
                    const response = await fetch(`${API}/exports`, {
                      method: 'POST',
                      credentials: 'same-origin',
                      headers: { 'x-w3bs-training': '1', 'content-type': 'application/json' },
                      body: JSON.stringify(body),
                    });
                    if (!response.ok)
                      throw new Error(
                        (await response.json()).error?.message || response.statusText,
                      );
                    const name =
                      /filename="([^"]+)"/.exec(
                        response.headers.get('content-disposition') || '',
                      )?.[1] || 'export.zip';
                    const url = URL.createObjectURL(await response.blob());
                    const link = h('a', { href: url, download: name });
                    document.body.append(link);
                    link.click();
                    link.remove();
                    setTimeout(() => URL.revokeObjectURL(url), 60000);
                    return toast(`Downloaded ${name}`);
                  }
                  const r = await api('/exports', { method: 'POST', body });
                  toast(`Exported to ${r.destination}`);
                }),
              },
              HOSTED ? 'Download' : 'Export',
            ),
          ),
          h(
            'button',
            {
              class: 'danger',
              onclick: run(async () => {
                if (!confirm('Revoke this version and every grant to it?')) return;
                await api(`/datasets/${id}/versions/${v.version}/revoke`, { method: 'POST' });
                render();
              }),
            },
            'Revoke',
          ),
        ]
      : null,
  );
}

// ------------------------------------------------------------------ Teach an agent (Liquid Agent)
// Local app only. Everything starts off: recording, training-data use, upload, remote
// inference and action execution are separate permissions a person turns on here.
const LQ = (path, options) => api(`/liquid${path}`, options);
const TEACH_TASKS = ['email.reply_draft', 'reddit.post_compose', 'linkedin.notifications_review'];
const TEACH_APPS = {
  'email.reply_draft': 'mail',
  'reddit.post_compose': 'browser',
  'linkedin.notifications_review': 'browser',
};
const MODE_NAMES = {
  off: 'Off',
  suggest: 'Suggest',
  approve: 'Execute with approval',
  bounded: 'Bounded automation',
};
const short = (hash) => (hash ? `${hash.slice(0, 15)}…${hash.slice(-6)}` : 'none');
const pct = (x) => (typeof x === 'number' ? `${Math.round(x * 100)}%` : 'n/a');

async function refreshIndicator() {
  const el = document.getElementById('teach-indicator');
  if (!el || HOSTED) return;
  try {
    const s = await LQ('/status');
    const rec = s.recording;
    el.hidden = !(rec.state === 'recording' || rec.state === 'paused' || s.kill_switch);
    el.replaceChildren(
      rec.state === 'recording'
        ? h(
            'a',
            { href: '#/teach', class: 'rec' },
            `● Recording ${rec.app} · ${rec.task} · ${rec.steps} step(s)`,
          )
        : rec.state === 'paused'
          ? h('a', { href: '#/teach' }, `❚❚ Recording paused (${rec.app})`)
          : null,
      s.kill_switch ? h('a', { href: '#/teach', class: 'killed' }, 'Kill switch ON') : null,
    );
  } catch {
    el.hidden = true;
  }
}

function teachEpisode(id) {
  return (async () => {
    const d = await LQ(`/episodes/${encodeURIComponent(id)}`);
    const ep = d.episode;
    const goal = h('textarea', { 'aria-label': 'Goal', rows: 2 }, ep.labels.goal ?? '');
    const success = h(
      'select',
      { 'aria-label': 'Outcome' },
      h('option', { value: 'yes', selected: ep.labels.success || undefined }, 'Success'),
      h('option', { value: 'no', selected: !ep.labels.success || undefined }, 'Failure'),
    );
    const reason = h('input', {
      'aria-label': 'Failure reason',
      value: ep.labels.failure_reason ?? '',
      placeholder: 'Why it failed (failures only)',
    });
    const risk = h(
      'select',
      { 'aria-label': 'Risk' },
      ['low', 'medium', 'high'].map((r) =>
        h('option', { value: r, selected: ep.labels.risk === r || undefined }, r),
      ),
    );
    const save = (reviewed) =>
      run(async () => {
        const ok = success.value === 'yes';
        await LQ(`/episodes/${id}/review`, {
          method: 'POST',
          body: {
            goal: goal.value,
            success: ok,
            failure_reason: ok ? '' : reason.value,
            risk: risk.value,
            reviewed,
          },
        });
        toast(reviewed ? 'Reviewed' : 'Saved');
        render();
      });
    const redact = (step, field) =>
      run(async () => {
        await LQ(`/episodes/${id}/redact`, { method: 'POST', body: { step, field } });
        render();
      });
    return [
      h('p', {}, h('a', { href: '#/teach' }, '← Teach an agent')),
      h('h1', {}, `${ep.task}`),
      h(
        'div',
        { class: 'chips' },
        chip(d.state, d.state === 'reviewed' ? 'ok' : d.state === 'proposed' ? 'warn' : ''),
        chip(d.source),
        chip(`${ep.steps.length} steps`),
        chip(`content ${short(ep.content_hash)}`),
      ),
      d.state === 'proposed'
        ? h(
            'p',
            { class: 'notice' },
            'A correction from an agent run. It joins nothing until you accept it for review.',
            ' ',
            h(
              'button',
              {
                onclick: run(
                  async () => (await LQ(`/episodes/${id}/accept`, { method: 'POST' }), render()),
                ),
              },
              'Accept for review',
            ),
          )
        : null,
      h('h2', {}, 'Labels'),
      h('div', { class: 'row' }, h('label', {}, h('span', {}, 'Goal'), goal)),
      h(
        'div',
        { class: 'row' },
        h('label', {}, h('span', {}, 'Outcome'), success),
        h('label', {}, h('span', {}, 'Risk'), risk),
        h('label', {}, h('span', {}, 'Failure reason'), reason),
      ),
      d.state === 'proposed'
        ? null
        : h(
            'div',
            { class: 'actions' },
            h('button', { onclick: save(false) }, 'Save'),
            h('button', { class: 'primary', onclick: save(true) }, 'Mark reviewed'),
            h(
              'button',
              {
                class: 'danger',
                onclick: run(
                  async () => (
                    confirm('Discard this episode?') &&
                      (await LQ(`/episodes/${id}/discard`, { method: 'POST' })),
                    render()
                  ),
                ),
              },
              'Discard',
            ),
          ),
      h('h2', {}, 'Timeline'),
      h(
        'p',
        { class: 'muted' },
        'What a model would see: app, URL pattern, focus and the visible controls. Typed text in secret fields was never kept. Redactions can be undone on this device.',
      ),
      h(
        'div',
        { class: 'scroll' },
        h(
          'table',
          {},
          h(
            'thead',
            {},
            h(
              'tr',
              {},
              ['#', 't', 'Screen', 'Action', 'Target', 'Text', ''].map((x) => h('th', {}, x)),
            ),
          ),
          h(
            'tbody',
            {},
            ep.steps.map((s, i) =>
              h(
                'tr',
                {},
                h('td', {}, String(i)),
                h('td', {}, `${(s.t_ms / 1000).toFixed(1)}s`),
                h(
                  'td',
                  {},
                  `${s.observation.app} ${s.observation.url_pattern ?? ''}${s.observation.loading ? ' (loading)' : ''}`,
                ),
                h('td', {}, s.action.type),
                h(
                  'td',
                  {},
                  s.action.target ? `${s.action.target.role} “${s.action.target.name}”` : '',
                ),
                h('td', {}, s.action.text_redacted ? '[not kept]' : (s.action.text ?? '')),
                h(
                  'td',
                  {},
                  s.action.text !== undefined
                    ? h('button', { onclick: redact(i, 'action.text') }, 'Redact text')
                    : null,
                  s.action.target && s.action.target.name !== '[redacted]'
                    ? h('button', { onclick: redact(i, 'action.target.name') }, 'Redact target')
                    : null,
                ),
              ),
            ),
          ),
        ),
      ),
      d.redactions.length
        ? [
            h('h2', {}, 'Redactions'),
            h(
              'ul',
              {},
              d.redactions.map((r) =>
                h(
                  'li',
                  {},
                  `step ${r.step} ${r.field} `,
                  h(
                    'button',
                    {
                      onclick: run(
                        async () => (
                          await LQ(`/episodes/${id}/unredact`, {
                            method: 'POST',
                            body: { redaction_id: r.redaction_id },
                          }),
                          render()
                        ),
                      ),
                    },
                    'Undo',
                  ),
                ),
              ),
            ),
          ]
        : null,
    ];
  })();
}

views.teach = async (arg) => {
  if (HOSTED)
    return [
      h('h1', {}, 'Teach an agent'),
      h('p', {}, 'Teaching runs only in the local app (w3bs training open).'),
    ];
  if (arg?.startsWith('ep_')) return teachEpisode(arg);
  const [s, episodes, datasets, jobs, models, asks, runs, auditRows] = await Promise.all([
    LQ('/status'),
    LQ('/episodes'),
    LQ('/datasets'),
    LQ('/jobs'),
    LQ('/models'),
    LQ('/asks'),
    LQ('/runs'),
    LQ('/audit'),
  ]);
  const perm = Object.fromEntries(s.permissions.map((p) => [p.name, p]));
  const post = (path, body) => LQ(path, { method: 'POST', body: body ?? {} });

  // Kill switch and permissions
  const killer = h(
    'div',
    { class: `card${s.kill_switch ? ' warn' : ''}` },
    h('h2', {}, 'Kill switch'),
    h(
      'p',
      {},
      s.kill_switch
        ? 'ON: no agent action runs. Pending approvals were cancelled.'
        : 'Off. Throwing it stops every agent action before the next one runs.',
    ),
    s.kill_switch
      ? h(
          'button',
          {
            onclick: run(
              async () => (
                confirm('Let agents act again in the modes you set?') &&
                  (await post('/kill-switch', { on: false })),
                render()
              ),
            ),
          },
          'Release',
        )
      : h(
          'button',
          {
            class: 'danger',
            onclick: run(async () => (await post('/kill-switch', { on: true }), render())),
          },
          'Stop all agent actions',
        ),
  );
  const permissions = h(
    'div',
    { class: 'card' },
    h('h2', {}, 'Permissions'),
    h(
      'p',
      { class: 'muted' },
      'Each is off until you turn it on, and each can be revoked at any time. Every change is in the audit log.',
    ),
    h(
      'table',
      {},
      h(
        'tbody',
        {},
        s.permissions.map((p) =>
          h(
            'tr',
            {},
            h('td', {}, h('strong', {}, p.name.replace('_', ' '))),
            h('td', {}, p.description),
            h(
              'td',
              {},
              p.on ? chip('on', 'ok') : chip('off'),
              p.changed_at ? h('span', { class: 'meta' }, ` ${when(p.changed_at)}`) : null,
            ),
            h(
              'td',
              {},
              h(
                'button',
                {
                  class: p.on ? 'danger' : '',
                  onclick: run(async () => {
                    if (!p.on && !confirm(`Turn on ${p.name.replace('_', ' ')}? ${p.description}.`))
                      return;
                    await post(`/permissions/${p.name}`, { on: !p.on });
                    render();
                  }),
                },
                p.on ? 'Revoke' : 'Turn on',
              ),
            ),
          ),
        ),
      ),
    ),
  );

  // Record
  const task = h(
    'select',
    { 'aria-label': 'Task' },
    TEACH_TASKS.map((t) => h('option', { value: t }, t)),
  );
  const app = h('input', { 'aria-label': 'App or window', value: 'mail', placeholder: 'mail' });
  task.addEventListener('change', () => (app.value = TEACH_APPS[task.value] ?? ''));
  const profile = h(
    'select',
    { 'aria-label': 'Redaction profile' },
    h('option', { value: 'strict' }, 'strict: keep no typed text'),
    h('option', { value: 'standard' }, 'standard: keep non-secret text'),
  );
  const shots = h('input', { type: 'checkbox', 'aria-label': 'Screenshot hashes' });
  const rec = s.recording;
  const record = h(
    'div',
    { class: 'card' },
    h('h2', {}, '1. Record'),
    rec.state === 'recording' || rec.state === 'paused'
      ? [
          h(
            'p',
            { class: rec.state === 'recording' ? 'notice' : '' },
            `${rec.state === 'recording' ? '● Recording' : '❚❚ Paused'}: ${rec.app}, task ${rec.task}, ${rec.steps} step(s) since ${when(rec.since)}.`,
          ),
          h(
            'div',
            { class: 'actions' },
            rec.state === 'recording'
              ? h(
                  'button',
                  {
                    onclick: run(
                      async () => (await post(`/captures/${rec.capture_id}/pause`), render()),
                    ),
                  },
                  'Pause',
                )
              : h(
                  'button',
                  {
                    onclick: run(
                      async () => (await post(`/captures/${rec.capture_id}/resume`), render()),
                    ),
                  },
                  'Resume',
                ),
            h(
              'button',
              {
                class: 'primary',
                onclick: run(
                  async () => (await post(`/captures/${rec.capture_id}/stop`), render()),
                ),
              },
              'Stop',
            ),
            h(
              'button',
              {
                class: 'danger',
                onclick: run(
                  async () => (
                    confirm('Throw this recording away?') &&
                      (await post(`/captures/${rec.capture_id}/stop`, { discard: true })),
                    render()
                  ),
                ),
              },
              'Discard',
            ),
          ),
        ]
      : [
          h(
            'p',
            { class: 'muted' },
            perm.recording.on
              ? 'One app or window, one task. Secret fields never carry text. Nothing is recorded until you press Start.'
              : 'Turn on the recording permission first.',
          ),
          h(
            'div',
            { class: 'row' },
            h('label', {}, h('span', {}, 'Task'), task),
            h('label', {}, h('span', {}, 'App or window'), app),
            h('label', {}, h('span', {}, 'Profile'), profile),
            h('label', {}, shots, ' screenshot hashes'),
          ),
          h(
            'div',
            { class: 'actions' },
            h(
              'button',
              {
                class: 'primary',
                disabled: !perm.recording.on || undefined,
                onclick: run(
                  async () => (
                    await post('/captures', {
                      task: task.value,
                      app: app.value,
                      profile: profile.value,
                      screenshots: shots.checked,
                    }),
                    render()
                  ),
                ),
              },
              'Start recording',
            ),
            h(
              'button',
              {
                disabled: !perm.recording.on || undefined,
                title:
                  'A scripted person performs the three tasks on the mock sites while recording',
                onclick: run(async () => {
                  const r = await LQ('/demo', { method: 'POST', body: { count: 20 } });
                  toast(`Recorded ${r.episodes.length} demonstrations on the mock sites`);
                  render();
                }),
              },
              'Record 20 on the mock sites',
            ),
          ),
        ],
  );

  // Review
  const byState = (st) => episodes.filter((e) => e.state === st);
  const epList = (list) =>
    list.length
      ? h(
          'ul',
          {},
          list
            .slice(0, 50)
            .map((e) =>
              h(
                'li',
                {},
                h('a', { href: `#/teach/${e.episode_id}` }, `${e.task} · ${e.steps} steps`),
                ' ',
                chip(e.state, e.state === 'reviewed' ? 'ok' : ''),
                e.labels?.success ? chip('success', 'ok') : null,
              ),
            ),
        )
      : h('p', { class: 'muted' }, 'None.');
  const review = h(
    'div',
    { class: 'card' },
    h('h2', {}, '2. Review'),
    h(
      'p',
      {},
      `${byState('review_required').length} to review · ${byState('reviewed').length} reviewed · ${byState('duplicate').length} duplicates · ${byState('proposed').length} proposed corrections`,
    ),
    h(
      'div',
      { class: 'actions' },
      h(
        'button',
        {
          onclick: run(
            async () =>
              toast(
                `${(await post('/episodes/dedupe')).duplicates.length} duplicate(s) set aside`,
              ) || render(),
          ),
        },
        'Find duplicates',
      ),
    ),
    h('h3', {}, 'To review'),
    epList(byState('review_required')),
    byState('proposed').length
      ? [h('h3', {}, 'Proposed corrections'), epList(byState('proposed'))]
      : null,
    h('h3', {}, 'Reviewed'),
    epList(byState('reviewed')),
  );

  // Dataset + train
  const dsSelect = h(
    'select',
    { 'aria-label': 'Dataset' },
    datasets.map((d) =>
      h(
        'option',
        { value: d.dataset_id },
        `${d.name} · ${d.episodes} episodes · ${short(d.dataset_hash)}`,
      ),
    ),
  );
  const where = h(
    'select',
    { 'aria-label': 'Where to train' },
    h('option', { value: 'local' }, 'Train local (this computer)'),
    h(
      'option',
      { value: 'c0mpute', disabled: !perm.network_upload.on || undefined },
      'Trusted c0mpute worker',
    ),
  );
  const worker = h('input', { 'aria-label': 'Worker peer id', placeholder: 'worker peer id' });
  const cap = h('input', {
    'aria-label': 'Cost cap USD',
    type: 'number',
    min: '0',
    step: '0.01',
    value: '1',
  });
  const previewBox = h('div', {});
  const train = h(
    'div',
    { class: 'card' },
    h('h2', {}, '3. Dataset and training'),
    h(
      'p',
      { class: 'muted' },
      perm.training_data.on
        ? 'A dataset holds every reviewed episode, split by recording session, after a secret scan.'
        : 'Turn on training-data use first.',
    ),
    h(
      'div',
      { class: 'actions' },
      h(
        'button',
        {
          disabled: !perm.training_data.on || undefined,
          onclick: run(async () => {
            const d = await post('/datasets', {});
            toast(
              `Dataset ${short(d.dataset_hash)}: ${d.counts.train}/${d.counts.validation}/${d.counts.test}`,
            );
            render();
          }),
        },
        'Build dataset from reviewed episodes',
      ),
    ),
    datasets.length
      ? [
          h(
            'div',
            { class: 'row' },
            h('label', {}, h('span', {}, 'Dataset'), dsSelect),
            h('label', {}, h('span', {}, 'Where'), where),
          ),
          h(
            'div',
            { class: 'row' },
            h('label', {}, h('span', {}, 'Worker (c0mpute only)'), worker),
            h('label', {}, h('span', {}, 'Cost cap $'), cap),
          ),
          h(
            'div',
            { class: 'actions' },
            h(
              'button',
              {
                class: 'primary',
                disabled: !perm.training_data.on || undefined,
                onclick: run(async () => {
                  if (where.value === 'local') {
                    await post('/jobs', { dataset_id: dsSelect.value, target: 'local' });
                    toast('Training CfC and the GRU baseline on this computer');
                    return render();
                  }
                  const p = await post('/jobs/preview', {
                    dataset_id: dsSelect.value,
                    worker: worker.value,
                    max_price_usd: Number(cap.value),
                  });
                  previewBox.replaceChildren(
                    h(
                      'div',
                      { class: 'card warn' },
                      h('h3', {}, 'Upload preview'),
                      kv([
                        ['Dataset hash', p.dataset_hash],
                        ['Episodes', String(p.episodes)],
                        ['Worker', p.worker],
                        ['Cost cap', `$${p.max_price_usd}`],
                        ['Retention', p.retention.dataset],
                        ['Privacy', p.privacy],
                      ]),
                      h(
                        'button',
                        {
                          class: 'primary',
                          onclick: run(async () => {
                            await post('/jobs', {
                              dataset_id: p.dataset_id,
                              target: 'c0mpute',
                              preview_id: p.preview_id,
                              confirm: true,
                            });
                            toast('Sent to the worker');
                            render();
                          }),
                        },
                        'I understand, send it',
                      ),
                    ),
                  );
                }),
              },
              'Train',
            ),
          ),
          previewBox,
        ]
      : null,
    jobs.length
      ? h(
          'table',
          {},
          h(
            'tbody',
            {},
            jobs
              .slice(0, 10)
              .map((j) =>
                h(
                  'tr',
                  {},
                  h('td', {}, j.job_id),
                  h(
                    'td',
                    {},
                    chip(
                      j.state,
                      j.state === 'succeeded' ? 'ok' : j.state === 'failed' ? 'warn' : '',
                    ),
                  ),
                  h('td', {}, j.target === 'local' ? 'local' : `c0mpute ${j.worker}`),
                  h('td', {}, j.recommendation ?? j.error?.message ?? ''),
                ),
              ),
          ),
        )
      : null,
  );

  // Models: evaluate, activate
  const modelsCard = h(
    'div',
    { class: 'card' },
    h('h2', {}, '4. Evaluate and activate'),
    h(
      'p',
      { class: 'muted' },
      'Evaluation replays each workflow on the mock sites (and a changed layout) through the guarded executor. A liquid family is recommended only when it beats the GRU baseline; activation needs a passing evaluation.',
    ),
    models.length
      ? h(
          'table',
          {},
          h(
            'thead',
            {},
            h(
              'tr',
              {},
              ['Model', 'Family', 'Source', 'Weights hash', 'Episode success', '', ''].map((x) =>
                h('th', {}, x),
              ),
            ),
          ),
          h(
            'tbody',
            {},
            models.map((m) =>
              h(
                'tr',
                {},
                h('td', {}, m.model_id),
                h('td', {}, m.family, m.selected ? chip('recommended', 'ok') : null),
                h('td', {}, m.source),
                h('td', { title: m.model_hash }, short(m.model_hash)),
                h('td', {}, pct(m.metrics.episode_success)),
                h(
                  'td',
                  {},
                  h(
                    'button',
                    {
                      onclick: run(async () => {
                        const r = await post('/evaluate', { model_id: m.model_id });
                        toast(
                          r.pass ? 'Passed on the mock sites' : `Not ready: ${r.gaps.join('; ')}`,
                          !r.pass,
                        );
                        render();
                      }),
                    },
                    'Evaluate',
                  ),
                  m.eval_pass ? chip('passed', 'ok') : null,
                ),
                h(
                  'td',
                  {},
                  h(
                    'button',
                    {
                      class: 'primary',
                      disabled: !m.eval_pass || undefined,
                      onclick: run(
                        async () => (await post(`/models/${m.model_id}/activate`, {}), render()),
                      ),
                    },
                    'Activate',
                  ),
                ),
              ),
            ),
          ),
        )
      : h('p', { class: 'muted' }, 'No models yet.'),
  );

  // Use: modes per task, runs, approvals
  const caps = await Promise.all(TEACH_TASKS.map((t) => LQ(`/tasks/${t}/capability`)));
  const use = h(
    'div',
    { class: 'card' },
    h('h2', {}, '5. Use'),
    h(
      'p',
      { class: 'muted' },
      'Off by default. Suggest is recommended once checks pass; acting needs the action-execution permission. Every send, post, publish or delete asks you first, in every mode. Runs only on the mock sites.',
    ),
    h(
      'table',
      {},
      h(
        'tbody',
        {},
        caps.map((c) => {
          const mode = h(
            'select',
            { 'aria-label': `Mode for ${c.task}` },
            Object.entries(MODE_NAMES).map(([k, v]) =>
              h('option', { value: k, selected: c.mode === k || undefined }, v),
            ),
          );
          const steps = h('input', {
            'aria-label': 'Max steps',
            type: 'number',
            min: '1',
            max: '100',
            value: '20',
          });
          return h(
            'tr',
            {},
            h(
              'td',
              {},
              h('strong', {}, c.task),
              h('br'),
              c.active_model
                ? h(
                    'span',
                    { class: 'meta' },
                    `${c.active_model.family} · ${c.active_model.source} · ${short(c.active_model.model_hash)}`,
                  )
                : h('span', { class: 'meta' }, 'no active model'),
            ),
            h(
              'td',
              {},
              c.checks_pass
                ? chip(`recommended: ${MODE_NAMES[c.recommended_mode]}`, 'ok')
                : h('span', { class: 'meta' }, c.reasons.join('; ')),
            ),
            h(
              'td',
              {},
              mode,
              ' ',
              steps,
              ' ',
              h(
                'button',
                {
                  onclick: run(
                    async () => (
                      await post(`/tasks/${c.task}/mode`, {
                        mode: mode.value,
                        bounds:
                          mode.value === 'bounded' ? { max_steps: Number(steps.value) } : undefined,
                      }),
                      render()
                    ),
                  ),
                },
                'Set',
              ),
            ),
            h(
              'td',
              {},
              h(
                'button',
                {
                  disabled: c.mode === 'off' || undefined,
                  onclick: run(
                    async () => (
                      await post('/runs', { task: c.task, seed: Date.now() % 1000 }),
                      render()
                    ),
                  ),
                },
                'Run on mock site',
              ),
            ),
          );
        }),
      ),
    ),
    h('h3', {}, 'Waiting for you'),
    asks.length
      ? h(
          'ul',
          {},
          asks.map((q) =>
            h(
              'li',
              {},
              q.kind === 'approval'
                ? [
                    `${q.external ? 'EXTERNAL WRITE: ' : ''}${q.action.type} ${q.action.target?.name ?? ''} (${q.task}) `,
                    h(
                      'button',
                      {
                        class: 'primary',
                        onclick: run(
                          async () => (
                            await post(`/asks/${q.ask_id}`, { approve: true }),
                            render()
                          ),
                        ),
                      },
                      'Approve',
                    ),
                    ' ',
                    h(
                      'button',
                      {
                        class: 'danger',
                        onclick: run(
                          async () => (
                            await post(`/asks/${q.ask_id}`, { approve: false }),
                            render()
                          ),
                        ),
                      },
                      'Deny',
                    ),
                  ]
                : [
                    `${q.kind === 'suggestion' ? 'Suggestion' : 'Take over'}: ${q.proposal ? `${q.proposal.type} ${q.proposal.target?.name ?? ''}` : `none (${q.abstain_reason ?? 'abstained'})`} (${q.task}) `,
                    q.proposal
                      ? h(
                          'button',
                          {
                            onclick: run(
                              async () => (
                                await post(`/asks/${q.ask_id}`, { accept: true }),
                                render()
                              ),
                            ),
                          },
                          'Do it',
                        )
                      : null,
                    ' ',
                    h(
                      'button',
                      {
                        onclick: run(
                          async () => (await post(`/asks/${q.ask_id}`, { stop: true }), render()),
                        ),
                      },
                      'Stop',
                    ),
                  ],
            ),
          ),
        )
      : h('p', { class: 'muted' }, 'Nothing.'),
    runs.length
      ? [
          h('h3', {}, 'Runs'),
          h(
            'table',
            {},
            h(
              'tbody',
              {},
              runs
                .slice(0, 10)
                .map((r) =>
                  h(
                    'tr',
                    {},
                    h('td', {}, r.task),
                    h('td', {}, MODE_NAMES[r.mode]),
                    h('td', {}, chip(r.state, r.report?.success ? 'ok' : '')),
                    h(
                      'td',
                      {},
                      r.report
                        ? `${r.report.executed.length} executed · ${r.report.approvals_requested} approvals · stopped by ${r.report.stopped_by ?? 'done'}`
                        : (r.error?.message ?? ''),
                    ),
                  ),
                ),
            ),
          ),
        ]
      : null,
  );

  const auditCard = h(
    'div',
    { class: 'card' },
    h('h2', {}, 'Audit log'),
    h(
      'div',
      { class: 'scroll' },
      h(
        'table',
        {},
        h(
          'tbody',
          {},
          auditRows
            .slice(0, 60)
            .map((r) =>
              h(
                'tr',
                {},
                h('td', {}, when(r.at)),
                h('td', {}, r.actor),
                h('td', {}, r.kind),
                h('td', {}, r.subject ?? ''),
              ),
            ),
        ),
      ),
    ),
  );

  return [
    h('h1', {}, 'Teach an agent'),
    h(
      'p',
      {},
      'Show W3BS a task a few times; it trains a small temporal policy (CfC liquid network, with a GRU baseline) and can then suggest or, with your approval, perform the next step. Mock sites only for now; nothing real is sent, posted or logged in to.',
    ),
    killer,
    permissions,
    record,
    review,
    train,
    modelsCard,
    use,
    auditCard,
  ];
};

// ------------------------------------------------------------------ router
async function render() {
  const [, view = 'overview', arg] = location.hash.match(/^#\/([a-z]+)\/?(.*)$/) || [];
  const fn = views[view] ?? views.overview;
  document.querySelectorAll('.tabs a').forEach((a) => {
    const target = a.getAttribute('href').slice(2);
    if (
      target === view ||
      (view === 'episode' && target === 'sessions') ||
      (view === 'dataset' && target === 'datasets') ||
      (view === 'teach' && target === 'teach')
    )
      a.setAttribute('aria-current', 'page');
    else a.removeAttribute('aria-current');
  });
  try {
    main.replaceChildren(...[await fn(decodeURIComponent(arg || ''))].flat(3).filter(Boolean));
  } catch (error) {
    main.replaceChildren(
      h('h1', {}, 'Something went wrong'),
      h('p', {}, `${error.code ?? ''} ${error.message}`),
    );
  }
  refreshStatus();
  refreshIndicator();
}

// Keyboard-first inbox: c/p/f/u act on the first card, j/k move focus.
document.addEventListener('keydown', (event) => {
  if (
    !location.hash.startsWith('#/inbox') ||
    event.target.closest('input, textarea, select') ||
    event.metaKey ||
    event.ctrlKey ||
    event.altKey
  )
    return;
  const cards = [...document.querySelectorAll('[data-episode]')];
  const current = cards.indexOf(document.activeElement?.closest('[data-episode]'));
  const card = cards[current >= 0 ? current : 0];
  if (!card) return;
  const buttons = { c: 'Complete', p: 'Partial', f: 'Failed', u: 'Unknown' };
  if (buttons[event.key])
    [...card.querySelectorAll('button')].find((b) => b.textContent === buttons[event.key])?.click();
  if (event.key === 'j' || event.key === 'k') {
    const next =
      cards[
        Math.max(
          0,
          Math.min(cards.length - 1, (current < 0 ? 0 : current) + (event.key === 'j' ? 1 : -1)),
        )
      ];
    next.setAttribute('tabindex', '-1');
    next.focus();
  }
});

window.addEventListener('hashchange', render);
new EventSource(`${API}/events`).addEventListener('change', () => {
  refreshStatus();
  refreshIndicator();
});
api('/status')
  .then((s) => {
    HOSTED = s.key_store === 'server-managed';
    if (HOSTED) document.querySelector('.tabs a[href="#/teach"]')?.remove();
  })
  .catch(() => {})
  .finally(render);
