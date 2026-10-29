const send = async (path, body) => {
  const response = await fetch(path, {
    method: 'POST',
    headers: { accept: 'application/json', 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data?.error?.message || `Request failed (${response.status})`);
  return data;
};

for (const [id, path] of [
  ['net-register-tld', '/api/net/v1/tlds'],
  ['net-register-name', '/api/net/v1/names'],
]) {
  const form = document.getElementById(id);
  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    const button = form.querySelector('button[type="submit"]');
    const status = form.querySelector('[data-status]');
    const fields = new FormData(form);
    const body = id === 'net-register-tld'
      ? { tld: fields.get('tld'), open: fields.get('open') === '1' }
      : { name: fields.get('name'), target: fields.get('target') || null };
    button.disabled = true;
    status.classList.remove('net-error');
    try {
      const result = await send(path, body);
      status.textContent = `${result.name || `.${result.tld}`} is in the moderation queue.`;
      form.reset();
    } catch (error) {
      status.textContent = error.message;
      status.classList.add('net-error');
    } finally {
      button.disabled = false;
    }
  });
}
