// w3bs.org/tasks and /workflows: unpublish a task, create or delete a workflow. The pages are
// server-rendered; every action calls /api/v1 and then moves on.
async function call(path, { body, method = body === undefined ? 'GET' : 'POST' } = {}) {
  const response = await fetch(path, {
    method,
    headers: body === undefined ? {} : { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
    credentials: 'same-origin',
  });
  const result = await response.json().catch(() => ({}));
  if (!response.ok)
    throw new Error(result.error?.message || `Request failed (${response.status}).`);
  return result;
}

const status = (node, message, error = false) => {
  if (!node) return;
  node.textContent = message;
  node.classList.toggle('error', error);
};

// Ticked order is the workflow's order.
const picked = [];
document.querySelectorAll('[data-form="workflow-create"] input[name="tasks"]').forEach((box) =>
  box.addEventListener('change', () => {
    const at = picked.indexOf(box.value);
    if (box.checked && at === -1) picked.push(box.value);
    if (!box.checked && at !== -1) picked.splice(at, 1);
  }),
);

const create = document.querySelector('[data-form="workflow-create"]');
create?.addEventListener('submit', async (event) => {
  event.preventDefault();
  const data = new FormData(create);
  try {
    const made = await call('/api/v1/workflows', {
      body: {
        title: data.get('title'),
        description: data.get('description') || undefined,
        order: data.get('order'),
        tasks: picked,
      },
    });
    location.href = `/workflows/${made.id}`;
  } catch (error) {
    status(create.querySelector('.form-status'), error.message, true);
  }
});

document.addEventListener('click', async (event) => {
  const button = event.target.closest('[data-action]');
  if (!button) return;
  const node = document.getElementById('tasks-status');
  const { action, id } = button.dataset;
  const [question, path, next] =
    action === 'unpublish'
      ? ['Unpublish this task? It leaves every workflow it is in.', `/api/v1/tasks/${id}`, '/tasks']
      : [
          'Delete this workflow? Its tasks stay published.',
          `/api/v1/workflows/${id}`,
          '/workflows',
        ];
  if (!confirm(question)) return;
  try {
    await call(path, { method: 'DELETE' });
    location.href = next;
  } catch (error) {
    status(node, error.message, true);
  }
});
