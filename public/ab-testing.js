// w3bs.org/ab-testing: create experiments and keys, change state, delete. The pages are
// server-rendered; every action calls /api/ab and reloads.
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

// "id weight {json}" per line; weight and parameters are optional.
export function parseVariants(source) {
  return source
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line, index) => {
      const match = line.match(/^(\S+)(?:\s+(\d+))?(?:\s+(\{.*\}))?$/);
      if (!match) throw new Error(`Line ${index + 1}: write it as  id weight {"key":"value"}`);
      let parameters;
      if (match[3]) {
        try {
          parameters = JSON.parse(match[3]);
        } catch {
          throw new Error(`Line ${index + 1}: the parameters are not valid JSON.`);
        }
      }
      return { id: match[1], weight: match[2] ? Number(match[2]) : 1, parameters };
    });
}

// Imported by the tests for parseVariants, where there is no document.
if (globalThis.document) {
  const status = (form, message, error = false) => {
    const node = form?.querySelector('.form-status') ?? document.getElementById('ab-status');
    if (!node) return;
    node.textContent = message;
    node.classList.toggle('error', error);
  };

  const create = document.querySelector('[data-form="ab-create"]');
  create?.addEventListener('submit', async (event) => {
    event.preventDefault();
    const data = new FormData(create);
    try {
      const days = Number(data.get('days') || 30);
      const made = await call('/api/ab/experiments', {
        body: {
          name: data.get('name'),
          slug: data.get('slug'),
          unit: data.get('unit'),
          eligibilityRule: data.get('eligibilityRule') || undefined,
          variants: parseVariants(String(data.get('variants') ?? '')),
          minimumEligiblePerVariant: Number(data.get('minimumEligiblePerVariant') || 100),
          days,
        },
      });
      location.href = `/ab-testing/${made.slug}`;
    } catch (error) {
      status(create, error.message, true);
    }
  });

  const keyForm = document.querySelector('[data-form="ab-key"]');
  keyForm?.addEventListener('submit', async (event) => {
    event.preventDefault();
    try {
      const made = await call('/api/ab/keys', {
        body: { name: new FormData(keyForm).get('name') || undefined },
      });
      document.getElementById('ab-key-value').textContent = made.key;
      keyForm.nextElementSibling.hidden = false;
      status(keyForm, 'Key made. It works now.');
    } catch (error) {
      status(keyForm, error.message, true);
    }
  });

  const data = document.getElementById('ab-data');
  const experiment = data ? JSON.parse(data.textContent) : null;

  document.addEventListener('click', async (event) => {
    const button = event.target.closest('[data-action]');
    if (!button) return;
    try {
      switch (button.dataset.action) {
        case 'copy-key':
          await navigator.clipboard.writeText(document.getElementById('ab-key-value').textContent);
          button.textContent = 'Copied';
          return;
        case 'remove-key':
          if (!confirm('Revoke this key? Servers using it stop working at once.')) return;
          await call(`/api/ab/keys/${encodeURIComponent(button.dataset.id)}`, { method: 'DELETE' });
          break;
        case 'state': {
          const state = button.dataset.state;
          if (
            state === 'closed' &&
            !confirm('Close this experiment? It takes no new participants and cannot be reopened.')
          )
            return;
          await call(`/api/ab/experiments/${experiment.slug}/state`, { body: { state } });
          break;
        }
        case 'delete':
          if (
            !confirm(
              'Delete this experiment, its assignments and every event? This cannot be undone.',
            )
          )
            return;
          await call(`/api/ab/experiments/${experiment.slug}`, { method: 'DELETE' });
          location.href = '/ab-testing';
          return;
        default:
          return;
      }
      location.reload();
    } catch (error) {
      status(null, error.message, true);
    }
  });
}
