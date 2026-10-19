// w3bs.org/tasks and w3bs.org/workflows: published OpenTasks (logicsrc.com/docs/opentask)
// and OpenWorkflows (logicsrc.com/docs/openworkflow) made of them.
//
// A task comes from W3BS Training (`w3bs training publish`, or Publish in the app): a reviewed,
// successful demonstration, already turned into an OpenTask on the person's machine with window
// titles removed unless they kept them. The server validates it, gives it a public id, writes
// its moshscript from the task itself (never from uploaded code) and stores the exact bytes it
// serves, so the digests a workflow carries keep matching. A workflow is an ordered list of
// published tasks, served with each entry by URL and sha256.
import { createHash, randomBytes } from 'node:crypto';
import { validate } from '@logicsrc/validators';
import { W3bsError } from './core.mjs';
import { toMoshscript } from '../training/core/moshscript.mjs';
import { taskSlug } from '../training/core/opentask.mjs';

const tables = () => `
  CREATE TABLE IF NOT EXISTS pub_tasks (id TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, episode TEXT NOT NULL, title TEXT NOT NULL, body TEXT NOT NULL, script TEXT NOT NULL, sha256 TEXT NOT NULL, published_at TEXT NOT NULL, updated_at TEXT NOT NULL, UNIQUE (user_id, episode));
  CREATE INDEX IF NOT EXISTS pub_tasks_user ON pub_tasks(user_id);
  CREATE TABLE IF NOT EXISTS pub_workflows (id TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, title TEXT NOT NULL, description TEXT NOT NULL, run_order TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
  CREATE INDEX IF NOT EXISTS pub_workflows_user ON pub_workflows(user_id);
  CREATE TABLE IF NOT EXISTS pub_workflow_tasks (workflow_id TEXT NOT NULL REFERENCES pub_workflows(id) ON DELETE CASCADE, task_id TEXT NOT NULL REFERENCES pub_tasks(id) ON DELETE CASCADE, position INTEGER NOT NULL, PRIMARY KEY (workflow_id, task_id));`;
export const TASKS_SCHEMA = { sqlite: tables(), postgres: tables() };

// The limits the pages state; change them here and the pages follow.
export const TASK_LIMITS = { tasks: 500, workflows: 100, perWorkflow: 200, list: 500 };

const ID = /^[a-z0-9][a-z0-9-]{0,63}$/;
const sha256 = (value) => createHash('sha256').update(value).digest('hex');
const nowIso = () => new Date().toISOString();
const fail = (code, message, status = 400) => new W3bsError(code, message, status);
const text = (value, max) => (typeof value === 'string' ? value.trim().slice(0, max) : '');
const suffix = () =>
  [...randomBytes(6)].map((b) => 'abcdefghijklmnopqrstuvwxyz0123456789'[b % 36]).join('');
const publicId = (title) => `${taskSlug(title).slice(0, 56).replace(/-$/, '')}-${suffix()}`;

function mustValidate(kind, doc) {
  const result = validate(kind, doc);
  if (!result.ok)
    throw fail(
      'INVALID_OPENTASK',
      `Not a valid ${kind}: ${result.errors
        .slice(0, 3)
        .map((e) => `${e.instancePath || '/'} ${e.message}`)
        .join('; ')}`,
    );
}

export function createTasks(db, { origin = 'https://w3bs.org' } = {}) {
  const summary = (row) => {
    const task = JSON.parse(row.body);
    return {
      id: row.id,
      title: task.title,
      goal: task.goal,
      apps: task.apps.map((a) => a.name),
      steps: task.steps.length,
      status: task.status,
      publisher: `@${row.username}`,
      published_at: row.published_at,
      updated_at: row.updated_at,
      url: `${origin}/tasks/${row.id}`,
      json: `${origin}/tasks/${row.id}.json`,
      mosh: `${origin}/tasks/${row.id}.mosh`,
    };
  };
  const taskRows = (where = '', params = [], limit = TASK_LIMITS.list) =>
    db.all(
      `SELECT t.*, u.username FROM pub_tasks t JOIN users u ON u.id = t.user_id ${where} ORDER BY t.updated_at DESC LIMIT ${Number(limit)}`,
      params,
    );
  const ownWorkflow = async (userId, id) => {
    const row = await db.get('SELECT * FROM pub_workflows WHERE id = ?', [id]);
    if (!row || row.user_id !== userId) throw fail('NOT_FOUND', 'No such workflow of yours.', 404);
    return row;
  };
  const checkTaskIds = async (ids) => {
    if (!Array.isArray(ids) || !ids.length)
      throw fail('INVALID_WORKFLOW', 'A workflow needs at least one published task.');
    if (ids.length > TASK_LIMITS.perWorkflow)
      throw fail('INVALID_WORKFLOW', `At most ${TASK_LIMITS.perWorkflow} tasks per workflow.`);
    const unique = [...new Set(ids)];
    for (const id of unique)
      if (
        typeof id !== 'string' ||
        !ID.test(id) ||
        !(await db.get('SELECT id FROM pub_tasks WHERE id = ?', [id]))
      )
        throw fail('UNKNOWN_TASK', `No published task ${String(id).slice(0, 80)}.`, 404);
    return unique;
  };
  const setTasks = async (tx, workflowId, ids) => {
    await tx.run('DELETE FROM pub_workflow_tasks WHERE workflow_id = ?', [workflowId]);
    for (const [i, id] of ids.entries())
      await tx.run(
        'INSERT INTO pub_workflow_tasks (workflow_id, task_id, position) VALUES (?, ?, ?)',
        [workflowId, id, i],
      );
  };
  const workflowFields = (input) => {
    const title = text(input.title, 200);
    if (!title) throw fail('INVALID_WORKFLOW', 'A workflow needs a title.');
    const order = input.order ?? 'sequence';
    if (!['sequence', 'any'].includes(order))
      throw fail('INVALID_WORKFLOW', 'order is sequence or any.');
    return { title, description: text(input.description, 4000), order };
  };

  const api = {
    // Publish (or republish) the OpenTask made from one episode.
    async publish(userId, input = {}) {
      const task = structuredClone(input.task);
      mustValidate('opentask', task);
      if (task.source.kind !== 'demonstration' || !task.source.episode)
        throw fail(
          'INVALID_OPENTASK',
          'w3bs.org/tasks takes W3BS Training demonstrations: source.episode is required.',
        );
      if (!task.source.reviewed || task.source.outcome !== 'success')
        throw fail('INVALID_OPENTASK', 'Only a reviewed, successful demonstration is published.');
      const existing = await db.get(
        'SELECT id, published_at FROM pub_tasks WHERE user_id = ? AND episode = ?',
        [userId, task.source.episode],
      );
      if (!existing) {
        const n = Number(
          (await db.get('SELECT COUNT(*) AS n FROM pub_tasks WHERE user_id = ?', [userId])).n,
        );
        if (n >= TASK_LIMITS.tasks)
          throw fail('LIMIT', `At most ${TASK_LIMITS.tasks} published tasks per account.`, 409);
      }
      const id = existing?.id ?? publicId(task.title);
      const now = nowIso();
      // The status is what this site has seen, and it has run nothing yet.
      const bare = { ...task, id, status: 'untested' };
      delete bare.runners;
      const filename = `${id}.mosh`;
      const script = toMoshscript(bare, {
        exportedAt: now,
        version: String(task.source.recorder ?? '').replace(/^w3bs-training /, '') || 'unknown',
        filename,
      });
      // OpenTask runners are https only, so a plain-http development server lists none.
      const doc = origin.startsWith('https://')
        ? {
            ...bare,
            runners: [
              {
                kind: 'moshscript',
                url: `${origin}/tasks/${filename}`,
                sha256: sha256(script),
                command: `moshcode run ${filename}`,
              },
            ],
          }
        : bare;
      mustValidate('opentask', doc);
      const body = `${JSON.stringify(doc, null, 2)}\n`;
      await db.transaction(async (tx) => {
        if (existing)
          await tx.run(
            'UPDATE pub_tasks SET title = ?, body = ?, script = ?, sha256 = ?, updated_at = ? WHERE id = ?',
            [doc.title, body, script, sha256(body), now, id],
          );
        else
          await tx.run(
            'INSERT INTO pub_tasks (id, user_id, episode, title, body, script, sha256, published_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)',
            [id, userId, task.source.episode, doc.title, body, script, sha256(body), now, now],
          );
      });
      let workflow;
      const name = text(input.workflow, 200);
      if (name) {
        const found = (
          await db.all('SELECT id, title FROM pub_workflows WHERE user_id = ?', [userId])
        ).find((w) => w.title.toLowerCase() === name.toLowerCase());
        if (found) {
          const ids = (
            await db.all(
              'SELECT task_id FROM pub_workflow_tasks WHERE workflow_id = ? ORDER BY position',
              [found.id],
            )
          ).map((r) => r.task_id);
          workflow = await api.updateWorkflow(userId, found.id, {
            tasks: ids.includes(id) ? ids : [...ids, id],
          });
        } else workflow = await api.createWorkflow(userId, { title: name, tasks: [id] });
      }
      return {
        id,
        url: `${origin}/tasks/${id}`,
        json: `${origin}/tasks/${id}.json`,
        mosh: `${origin}/tasks/${filename}`,
        republished: Boolean(existing),
        ...(workflow ? { workflow: { id: workflow.id, url: workflow.url } } : {}),
      };
    },
    async unpublish(userId, id) {
      const row = await db.get('SELECT user_id FROM pub_tasks WHERE id = ?', [id]);
      if (!row || row.user_id !== userId) throw fail('NOT_FOUND', 'No such task of yours.', 404);
      const emptied = [];
      await db.transaction(async (tx) => {
        const holders = await tx.all(
          'SELECT workflow_id FROM pub_workflow_tasks WHERE task_id = ?',
          [id],
        );
        await tx.run('DELETE FROM pub_workflow_tasks WHERE task_id = ?', [id]);
        await tx.run('DELETE FROM pub_tasks WHERE id = ?', [id]);
        // A workflow needs at least one task; one left empty goes too.
        for (const { workflow_id } of holders)
          if (
            !(await tx.get('SELECT task_id FROM pub_workflow_tasks WHERE workflow_id = ?', [
              workflow_id,
            ]))
          ) {
            await tx.run('DELETE FROM pub_workflows WHERE id = ?', [workflow_id]);
            emptied.push(workflow_id);
          }
      });
      return { id, unpublished: true, workflows_removed: emptied };
    },
    async list({ userId } = {}) {
      return (await taskRows(userId ? 'WHERE t.user_id = ?' : '', userId ? [userId] : [])).map(
        summary,
      );
    },
    async get(id) {
      if (!ID.test(id)) return null;
      const [row] = await taskRows('WHERE t.id = ?', [id], 1);
      if (!row) return null;
      const workflows = await db.all(
        'SELECT w.id, w.title FROM pub_workflow_tasks x JOIN pub_workflows w ON w.id = x.workflow_id WHERE x.task_id = ? ORDER BY w.title',
        [id],
      );
      return {
        ...summary(row),
        user_id: row.user_id,
        body: row.body,
        script: row.script,
        sha256: row.sha256,
        task: JSON.parse(row.body),
        workflows,
      };
    },

    async createWorkflow(userId, input = {}) {
      const fields = workflowFields(input);
      const ids = await checkTaskIds(input.tasks);
      const n = Number(
        (await db.get('SELECT COUNT(*) AS n FROM pub_workflows WHERE user_id = ?', [userId])).n,
      );
      if (n >= TASK_LIMITS.workflows)
        throw fail('LIMIT', `At most ${TASK_LIMITS.workflows} workflows per account.`, 409);
      const id = publicId(fields.title);
      const now = nowIso();
      await db.transaction(async (tx) => {
        await tx.run(
          'INSERT INTO pub_workflows (id, user_id, title, description, run_order, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)',
          [id, userId, fields.title, fields.description, fields.order, now, now],
        );
        await setTasks(tx, id, ids);
      });
      return {
        id,
        url: `${origin}/workflows/${id}`,
        json: `${origin}/workflows/${id}.json`,
        tasks: ids.length,
      };
    },
    async updateWorkflow(userId, id, input = {}) {
      const row = await ownWorkflow(userId, id);
      const fields = workflowFields({
        title: input.title ?? row.title,
        description: input.description ?? row.description,
        order: input.order ?? row.run_order,
      });
      const ids = input.tasks === undefined ? null : await checkTaskIds(input.tasks);
      await db.transaction(async (tx) => {
        await tx.run(
          'UPDATE pub_workflows SET title = ?, description = ?, run_order = ?, updated_at = ? WHERE id = ?',
          [fields.title, fields.description, fields.order, nowIso(), id],
        );
        if (ids) await setTasks(tx, id, ids);
      });
      return { id, url: `${origin}/workflows/${id}`, json: `${origin}/workflows/${id}.json` };
    },
    async deleteWorkflow(userId, id) {
      await ownWorkflow(userId, id);
      await db.transaction(async (tx) => {
        await tx.run('DELETE FROM pub_workflow_tasks WHERE workflow_id = ?', [id]);
        await tx.run('DELETE FROM pub_workflows WHERE id = ?', [id]);
      });
      return { id, deleted: true };
    },
    async workflows({ userId } = {}) {
      const rows = await db.all(
        `SELECT w.*, u.username, (SELECT COUNT(*) FROM pub_workflow_tasks x WHERE x.workflow_id = w.id) AS n FROM pub_workflows w JOIN users u ON u.id = w.user_id ${userId ? 'WHERE w.user_id = ?' : ''} ORDER BY w.updated_at DESC LIMIT ${TASK_LIMITS.list}`,
        userId ? [userId] : [],
      );
      return rows.map((w) => ({
        id: w.id,
        title: w.title,
        description: w.description,
        order: w.run_order,
        tasks: Number(w.n),
        publisher: `@${w.username}`,
        updated_at: w.updated_at,
        url: `${origin}/workflows/${w.id}`,
        json: `${origin}/workflows/${w.id}.json`,
      }));
    },
    // The workflow and its OpenWorkflow document, every entry by URL and digest.
    async workflow(id) {
      if (!ID.test(id)) return null;
      const w = await db.get(
        'SELECT w.*, u.username FROM pub_workflows w JOIN users u ON u.id = w.user_id WHERE w.id = ?',
        [id],
      );
      if (!w) return null;
      const rows = await db.all(
        'SELECT t.id, t.title, t.sha256, t.body FROM pub_workflow_tasks x JOIN pub_tasks t ON t.id = x.task_id WHERE x.workflow_id = ? ORDER BY x.position',
        [id],
      );
      const doc = {
        type: 'logicsrc.openworkflow',
        version: '0.1',
        id: w.id,
        title: w.title,
        ...(w.description ? { description: w.description } : {}),
        publisher: { name: `@${w.username}`, url: `${origin}/workflows` },
        updated: w.updated_at,
        order: w.run_order,
        tasks: rows.map((t) => ({
          url: `${origin}/tasks/${t.id}.json`,
          sha256: t.sha256,
          id: t.id,
          title: t.title,
        })),
      };
      return {
        id: w.id,
        user_id: w.user_id,
        title: w.title,
        description: w.description,
        order: w.run_order,
        publisher: `@${w.username}`,
        updated_at: w.updated_at,
        tasks: rows.map((t) => ({ id: t.id, title: t.title, task: JSON.parse(t.body) })),
        doc,
      };
    },
    async index() {
      const all = await api.workflows();
      return {
        type: 'logicsrc.openworkflow-index',
        version: '0.1',
        publisher: origin,
        updated: all[0]?.updated_at ?? nowIso(),
        workflows: all
          .filter((w) => w.tasks > 0)
          .map((w) => ({
            url: w.json,
            id: w.id,
            title: w.title,
            tasks: w.tasks,
            updated: w.updated_at,
          })),
      };
    },
  };
  return api;
}
