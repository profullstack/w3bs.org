// w3bs.org account pages. Everything goes through the site's own /api.
const fromB64url = (value) => {
  const base64 = value.replace(/-/g, '+').replace(/_/g, '/');
  const binary = atob(base64 + '='.repeat((4 - (base64.length % 4)) % 4));
  return Uint8Array.from(binary, (char) => char.charCodeAt(0)).buffer;
};
const toB64url = (buffer) =>
  btoa(String.fromCharCode(...new Uint8Array(buffer)))
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/, '');

async function call(path, { method = 'POST', body } = {}) {
  const response = await fetch(path, {
    method,
    headers: body === undefined ? {} : { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
    credentials: 'same-origin',
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data.error?.message || `Request failed (${response.status}).`);
  return data;
}

const creationOptions = (options) => ({
  ...options,
  challenge: fromB64url(options.challenge),
  user: { ...options.user, id: fromB64url(options.user.id) },
  excludeCredentials: (options.excludeCredentials || []).map((item) => ({
    ...item,
    id: fromB64url(item.id),
  })),
});
const requestOptions = (options) => ({
  ...options,
  challenge: fromB64url(options.challenge),
  allowCredentials: (options.allowCredentials || []).map((item) => ({
    ...item,
    id: fromB64url(item.id),
  })),
});
const credentialJson = (credential) => {
  const response = credential.response;
  const json = {
    id: credential.id,
    rawId: toB64url(credential.rawId),
    type: credential.type,
    clientExtensionResults: credential.getClientExtensionResults(),
    authenticatorAttachment: credential.authenticatorAttachment ?? undefined,
    response: { clientDataJSON: toB64url(response.clientDataJSON) },
  };
  if (response.attestationObject) {
    json.response.attestationObject = toB64url(response.attestationObject);
    json.response.transports = response.getTransports?.() ?? [];
  } else {
    json.response.authenticatorData = toB64url(response.authenticatorData);
    json.response.signature = toB64url(response.signature);
    if (response.userHandle) json.response.userHandle = toB64url(response.userHandle);
  }
  return json;
};

const supportsPasskeys = () => typeof window.PublicKeyCredential === 'function';
async function passkeyLogin(next, status) {
  if (!supportsPasskeys())
    throw new Error('This browser does not support passkeys. Use your password or an email link.');
  const { challengeId, options } = await call('/api/auth/passkey/options');
  status && (status.textContent = 'Waiting for your passkey…');
  const credential = await navigator.credentials.get({ publicKey: requestOptions(options) });
  await call('/api/auth/passkey', { body: { challengeId, response: credentialJson(credential) } });
  location.assign(next);
}
async function addPasskey(name) {
  if (!supportsPasskeys()) throw new Error('This browser does not support passkeys.');
  const { challengeId, options } = await call('/api/account/passkeys/options');
  const credential = await navigator.credentials.create({ publicKey: creationOptions(options) });
  await call('/api/account/passkeys', {
    body: { challengeId, response: credentialJson(credential), name },
  });
}

const fields = (form) => Object.fromEntries(new FormData(form).entries());
const handlers = {
  async signup(form, values) {
    if (!values.password) delete values.password;
    const { message } = await call('/api/auth/signup', { body: values });
    form.replaceChildren(
      Object.assign(document.createElement('p'), { className: 'auth-sent', textContent: message }),
    );
  },
  async email(form, values) {
    const { message } = await call('/api/auth/email', { body: values });
    form.replaceChildren(
      Object.assign(document.createElement('p'), { className: 'auth-sent', textContent: message }),
    );
  },
  async login(form, values) {
    await call('/api/auth/login', { body: values });
    location.assign(form.dataset.next || '/account');
  },
  async link(_form, values) {
    const { created } = await call('/api/auth/link', { body: values });
    location.assign(created ? '/account?welcome' : '/account');
  },
  async 'add-passkey'(_form, values) {
    await addPasskey(values.name);
    location.reload();
  },
  async password(_form, values) {
    await call('/api/account/password', { method: 'PUT', body: values });
    location.reload();
  },
  async 'add-email'(form, values) {
    const { message } = await call('/api/account/email', { body: values });
    form.replaceChildren(
      Object.assign(document.createElement('p'), { className: 'auth-sent', textContent: message }),
    );
  },
  async app(_form, values) {
    const redirectUris = values.redirectUris.split(/\s+/).filter(Boolean);
    await call('/api/account/apps', { body: { name: values.name, redirectUris } });
    location.reload();
  },
};

for (const form of document.querySelectorAll('form[data-form]')) {
  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    const status = form.querySelector('.form-status');
    const button = form.querySelector('button[type="submit"]');
    if (status) status.textContent = '';
    if (button) button.disabled = true;
    try {
      await handlers[form.dataset.form](form, fields(form));
    } catch (error) {
      if (status)
        status.textContent = error.name === 'NotAllowedError' ? 'Cancelled.' : error.message;
    } finally {
      if (button) button.disabled = false;
    }
  });
}

const confirmAndCall = async (button, question, path, options) => {
  if (question && !confirm(question)) return;
  button.disabled = true;
  try {
    await call(path, options);
    location.reload();
  } catch (error) {
    button.disabled = false;
    alert(error.message);
  }
};
document.addEventListener('click', async (event) => {
  const button = event.target.closest('button[data-action]');
  if (!button) return;
  const id = encodeURIComponent(button.dataset.id || '');
  switch (button.dataset.action) {
    case 'passkey-login': {
      const status = document.querySelector('form[data-form="login"] .form-status');
      try {
        await passkeyLogin(button.dataset.next || '/account', status);
      } catch (error) {
        if (status)
          status.textContent = error.name === 'NotAllowedError' ? 'Cancelled.' : error.message;
      }
      break;
    }
    case 'delete-passkey':
      return confirmAndCall(button, 'Remove this passkey?', `/api/account/passkeys/${id}`, {
        method: 'DELETE',
      });
    case 'remove-password':
      return confirmAndCall(
        button,
        'Remove your password? You will sign in with passkeys or email links.',
        '/api/account/password',
        { method: 'PUT', body: { password: null } },
      );
    case 'revoke-grant':
      return confirmAndCall(button, 'Sign this app out?', `/api/account/grants/${id}`, {
        method: 'DELETE',
      });
    case 'delete-app':
      return confirmAndCall(
        button,
        'Delete this app? Everyone signed in through it is signed out.',
        `/api/account/apps/${id}`,
        { method: 'DELETE' },
      );
    case 'coinpay-connect':
      button.disabled = true;
      try {
        const { url } = await call('/api/account/coinpay');
        location.assign(url);
      } catch (error) {
        button.disabled = false;
        alert(error.message);
      }
      break;
    case 'coinpay-disconnect':
      return confirmAndCall(
        button,
        'Disconnect CoinPay from this account?',
        '/api/account/coinpay',
        {
          method: 'DELETE',
        },
      );
    case 'logout':
      await call('/api/auth/logout');
      location.assign('/login');
  }
});

// Offer saved passkeys in the username field's autofill where the browser supports it.
const loginField = document.querySelector('#login');
if (
  loginField &&
  supportsPasskeys() &&
  (await PublicKeyCredential.isConditionalMediationAvailable?.())
) {
  try {
    const { challengeId, options } = await call('/api/auth/passkey/options');
    const credential = await navigator.credentials.get({
      publicKey: requestOptions(options),
      mediation: 'conditional',
    });
    await call('/api/auth/passkey', {
      body: { challengeId, response: credentialJson(credential) },
    });
    location.assign(loginField.form.dataset.next || '/account');
  } catch {
    // Dismissed or unsupported: the buttons still work.
  }
}
