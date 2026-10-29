const escape = (value) => String(value ?? '').replace(/[&<>"']/g, (c) => ({
  '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
}[c]));

const request = async (path, options = {}) => {
  const response = await fetch(path, {
    ...options,
    headers: { accept: 'application/json', ...(options.body ? { 'content-type': 'application/json' } : {}), ...options.headers },
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data?.error?.message || data?.error || `Request failed (${response.status})`);
  return data;
};

const setStatus = (form, message, isError = false) => {
  const status = form.querySelector('[data-status]');
  if (!status) return;
  status.textContent = message;
  status.classList.toggle('net-error', isError);
};

async function refresh() {
  const data = await request('/api/net/v1/operator/overview');
  const queue = document.querySelector('#net-queue');
  const items = [
    ...data.queue.namespaces.map((item) => ({ kind: 'namespace', subject: `.${item.tld}`, owner: item.owner, date: item.createdAt, details: JSON.stringify(item.policy) })),
    ...data.queue.names.map((item) => ({ kind: 'name', subject: item.name, owner: item.owner, date: item.createdAt, details: item.target || 'No HTTPS target supplied.' })),
  ];
  queue.innerHTML = items.length ? items.map((item) => `<article><strong>${escape(item.subject)}</strong><p class="net-row-meta">${escape(item.kind)} · account ${escape(item.owner)} · ${escape(item.date)}</p><p>${escape(item.details)}</p><div class="net-actions"><button class="button primary" data-moderate="approve" data-kind="${escape(item.kind)}" data-subject="${escape(item.subject)}">Approve</button><button class="button" data-moderate="reject" data-kind="${escape(item.kind)}" data-subject="${escape(item.subject)}">Reject</button></div></article>`).join('') : '<p class="muted">No registrations are waiting for review.</p>';

  const managed = document.querySelector('#net-managed');
  const active = [
    ...data.managed.namespaces.map((item) => ({ kind: 'namespace', subject: `.${item.tld}`, status: item.status, owner: item.owner, edge: data.hostedTlds.includes(item.tld) ? 'DNS/TLS ready' : 'DNS/TLS setup required' })),
    ...data.managed.names.map((item) => ({ kind: 'name', subject: item.name, status: item.status, owner: item.owner })),
  ];
  managed.innerHTML = active.length ? active.map((item) => `<article><strong>${escape(item.subject)}</strong><p class="net-row-meta">${escape(item.kind)} · ${escape(item.status)} · account ${escape(item.owner || 'operator seeded')}${item.edge ? ` · ${escape(item.edge)}` : ''}</p><div class="net-actions"><button class="button" data-moderate="${item.status === 'suspended' ? 'restore' : 'suspend'}" data-kind="${escape(item.kind)}" data-subject="${escape(item.subject)}">${item.status === 'suspended' ? 'Restore' : 'Suspend'}</button></div></article>`).join('') : '<p class="muted">No local registrations are active yet.</p>';

  const peerList = document.querySelector('#net-peers');
  const peers = data.peers.map((item) => `<article class="net-row"><div><strong>${escape(item.endpoint)}</strong><p class="net-row-meta">${escape(item.kind)} · ${escape(item.status)} · sent cursor ${escape(item.cursor)} · received cursor ${escape(item.inboundCursor)} · last seen ${escape(item.lastSeenAt || 'never')}</p>${item.lastError ? `<p class="net-row-meta">Last error: ${escape(item.lastError)}</p>` : ''}</div><button class="button" data-remove-peer="${escape(item.id)}">Remove</button></article>`).join('');
  peerList.innerHTML = `${peers || '<p class="muted">No remote W3BS peers are opted in yet.</p>'}<article class="net-row"><div><strong>https://pit.moshcode.sh</strong><p class="net-row-meta">Moshpit · public read-only backfill source</p></div></article>`;

  const backfill = document.querySelector('#net-backfill');
  backfill.textContent = data.backfill.length
    ? data.backfill.map((item) => `${item.source} · ${item.kind}: ${item.count} · updated ${item.importedAt}`).join(' | ')
    : 'Backfill has not started yet.';
}

document.addEventListener('click', async (event) => {
  const moderate = event.target.closest('[data-moderate]');
  if (moderate) {
    moderate.disabled = true;
    try {
      await request('/api/net/v1/operator/moderate', {
        method: 'POST',
        body: JSON.stringify({ kind: moderate.dataset.kind, subject: moderate.dataset.subject, action: moderate.dataset.moderate }),
      });
      await refresh();
    } catch (error) {
      alert(error.message);
      moderate.disabled = false;
    }
    return;
  }

  const removePeer = event.target.closest('[data-remove-peer]');
  if (removePeer) {
    removePeer.disabled = true;
    try {
      await request(`/api/net/v1/operator/peers/${encodeURIComponent(removePeer.dataset.removePeer)}`, { method: 'DELETE' });
      await refresh();
    } catch (error) {
      alert(error.message);
      removePeer.disabled = false;
    }
  }
});

for (const [id, endpoint] of [
  ['net-tld-form', '/api/net/v1/tlds'],
  ['net-name-form', '/api/net/v1/names'],
]) {
  const form = document.getElementById(id);
  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    const button = form.querySelector('button[type="submit"]');
    button.disabled = true;
    const fields = new FormData(form);
    const payload = id === 'net-tld-form'
      ? { tld: fields.get('tld'), open: fields.get('open') === '1' }
      : { name: fields.get('name'), target: fields.get('target') || null };
    try {
      const result = await request(endpoint, { method: 'POST', body: JSON.stringify(payload) });
      setStatus(form, `${result.name || `.${result.tld}`} submitted for moderation.`);
      form.reset();
      await refresh();
    } catch (error) {
      setStatus(form, error.message, true);
    } finally {
      button.disabled = false;
    }
  });
}

const peerForm = document.getElementById('net-peer-form');
peerForm.addEventListener('submit', async (event) => {
  event.preventDefault();
  const button = peerForm.querySelector('button[type="submit"]');
  button.disabled = true;
  const fields = new FormData(peerForm);
  try {
    const result = await request('/api/net/v1/operator/peers', {
      method: 'POST',
      body: JSON.stringify({ endpoint: fields.get('endpoint'), outboundToken: fields.get('outboundToken') }),
    });
    const secret = document.getElementById('net-peer-secret');
    secret.hidden = false;
    secret.innerHTML = `<p>Send this one-time inbound token to ${escape(result.endpoint)} and have them configure it as their outbound token:</p><code class="net-code">${escape(result.inboundToken)}</code>`;
    setStatus(peerForm, 'Peer opted in; existing W3BS announcements are being backfilled.');
    peerForm.reset();
    await refresh();
  } catch (error) {
    setStatus(peerForm, error.message, true);
  } finally {
    button.disabled = false;
  }
});

document.getElementById('net-backfill-now').addEventListener('click', async (event) => {
  const button = event.currentTarget;
  button.disabled = true;
  try {
    await request('/api/net/v1/operator/backfill', { method: 'POST', body: '{}' });
    document.getElementById('net-backfill').textContent = 'Moshpit backfill is running. It will continue in bounded chunks.';
  } catch (error) {
    document.getElementById('net-backfill').textContent = error.message;
  } finally {
    button.disabled = false;
  }
});

refresh().catch((error) => {
  document.querySelector('#net-queue').textContent = error.message;
  document.querySelector('#net-peers').textContent = error.message;
  document.querySelector('#net-backfill').textContent = error.message;
});
setInterval(() => refresh().catch(() => {}), 15_000);
