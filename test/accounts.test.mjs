import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash, generateKeyPairSync, randomBytes, sign } from 'node:crypto';
import { startServer } from '../src/server.mjs';
import { openStore } from '../src/store.mjs';
import { redirectAllowed } from '../src/accounts.mjs';

const ORIGIN = 'http://localhost:3000';
const b64url = (bytes) => Buffer.from(bytes).toString('base64url');
const sha = (value) => createHash('sha256').update(value).digest();

async function boot() {
  const mail = [];
  const instance = await startServer({
    store: await openStore({ path: ':memory:' }),
    port: 0,
    publicOrigin: ORIGIN,
    mailer: { send: async (message) => (mail.push(message), { sent: true }) },
    formGuard: { minAgeMs: 0, rateLimit: false },
  });
  const base = `http://localhost:${instance.server.address().port}`;
  const jar = new Map();
  const http = async (path, { body, form, headers = {}, cookies = true, ...rest } = {}) => {
    const response = await fetch(`${base}${path}`, {
      redirect: 'manual',
      ...rest,
      method: rest.method ?? (body !== undefined || form !== undefined ? 'POST' : 'GET'),
      headers: {
        ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
        ...(form !== undefined ? { 'Content-Type': 'application/x-www-form-urlencoded' } : {}),
        ...(cookies && jar.size
          ? { Cookie: [...jar].map(([k, v]) => `${k}=${v}`).join('; ') }
          : {}),
        ...headers,
      },
      body:
        body !== undefined
          ? JSON.stringify(body)
          : form !== undefined
            ? new URLSearchParams(form)
            : undefined,
    });
    for (const cookie of response.headers.getSetCookie()) {
      const [pair] = cookie.split(';');
      const [name, value] = pair.split('=');
      if (value) jar.set(name, value);
      else jar.delete(name);
    }
    return response;
  };
  // Hidden form-guard fields, as the page renders them.
  const guardFields = async (path) => {
    const html = await (await http(path, { cookies: false })).text();
    const form = html.slice(
      html.indexOf('data-form="' + (path === '/signup' ? 'signup' : 'email') + '"'),
    );
    return Object.fromEntries(
      [...form.matchAll(/<input type="hidden" name="([^"]+)" value="([^"]*)"/g)].map((m) => [
        m[1],
        m[2],
      ]),
    );
  };
  const linkFrom = (message) =>
    new URL(/https?:\/\/\S+\/auth\/link\?token=\S+/.exec(message.text)[0]).searchParams.get(
      'token',
    );
  async function signUp(username, email, password) {
    const res = await http('/api/auth/signup', {
      body: { username, email, password, ...(await guardFields('/signup')) },
    });
    assert.equal(res.status, 200);
    const res2 = await http('/api/auth/link', { body: { token: linkFrom(mail.at(-1)) } });
    assert.equal(res2.status, 200, await res2.clone().text());
    return (await res2.json()).user;
  }
  return {
    instance,
    http,
    jar,
    mail,
    guardFields,
    linkFrom,
    signUp,
    close: () => instance.close(),
  };
}

test('signup creates the account only when the emailed link is used, and never says who exists', async () => {
  const t = await boot();
  try {
    const fields = await t.guardFields('/signup');
    const res = await t.http('/api/auth/signup', {
      body: {
        username: 'ada',
        email: 'Ada@Example.com',
        password: 'correct horse battery',
        ...fields,
      },
    });
    assert.equal(res.status, 200);
    const message = (await res.json()).message;
    assert.equal(t.mail.length, 1);
    assert.equal(t.mail[0].to, 'ada@example.com');
    assert.match(t.mail[0].subject, /Confirm/);
    assert.equal((await t.http('/api/me')).status, 401);
    const token = t.linkFrom(t.mail[0]);
    // Opening the link (a mail scanner) does not use it.
    const page = await t.http(`/auth/link?token=${token}`);
    assert.match(await page.text(), /Confirm and create account/);
    const used = await t.http('/api/auth/link', { body: { token } });
    assert.equal(used.status, 200);
    assert.deepEqual((await used.json()).created, true);
    const me = await (await t.http('/api/me')).json();
    assert.equal(me.username, 'ada');
    assert.equal(me.email, 'ada@example.com');
    assert.equal(
      (await t.http('/api/auth/link', { body: { token } })).status,
      400,
      'links work once',
    );

    // The same address again: same answer, but the mail is a sign-in link.
    t.jar.clear();
    const again = await t.http('/api/auth/signup', {
      body: { username: 'someone', email: 'ada@example.com', ...(await t.guardFields('/signup')) },
    });
    assert.equal((await again.json()).message, message);
    assert.match(t.mail.at(-1).subject, /sign-in link/);
    // A taken username is public, so that one is reported.
    const taken = await t.http('/api/auth/signup', {
      body: { username: 'ada', email: 'other@example.com', ...(await t.guardFields('/signup')) },
    });
    assert.equal(taken.status, 409);
    // Posting straight at the endpoint without the rendered form is dropped silently.
    const before = t.mail.length;
    const bot = await t.http('/api/auth/signup', {
      body: { username: 'botty', email: 'bot@example.com' },
    });
    assert.equal(bot.status, 200);
    assert.equal(t.mail.length, before);
    // Reserved and malformed usernames.
    for (const username of ['admin', 'a', 'Bad Name', 'x--y'])
      assert.notEqual(
        (
          await t.http('/api/auth/signup', {
            body: { username, email: 'n@example.com', ...(await t.guardFields('/signup')) },
          })
        ).status,
        200,
      );
  } finally {
    await t.close();
  }
});

test('password login: identical failures, lockout to the email link, email link clears it', async () => {
  const t = await boot();
  try {
    await t.signUp('grace', 'grace@example.com', 'a long enough password');
    t.jar.clear();
    const unknown = await t.http('/api/auth/login', {
      body: { login: 'nobody', password: 'whatever whatever' },
    });
    const wrong = await t.http('/api/auth/login', {
      body: { login: 'grace', password: 'not the password' },
    });
    assert.equal(unknown.status, 401);
    assert.equal(wrong.status, 401);
    assert.deepEqual(await unknown.json(), await wrong.json());
    const ok = await t.http('/api/auth/login', {
      body: { login: 'GRACE@example.com', password: 'a long enough password' },
    });
    assert.equal(ok.status, 200);
    assert.equal((await (await t.http('/api/me')).json()).username, 'grace');
    t.jar.clear();
    for (let i = 0; i < 10; i++)
      await t.http('/api/auth/login', { body: { login: 'grace', password: 'wrong wrong wrong' } });
    const locked = await t.http('/api/auth/login', {
      body: { login: 'grace', password: 'a long enough password' },
    });
    assert.equal(locked.status, 429);
    assert.match((await locked.json()).error.message, /email link/);
    // Email link fallback; unknown addresses get the same answer and no mail.
    const sent = t.mail.length;
    const none = await t.http('/api/auth/email', {
      body: { email: 'missing@example.com', ...(await t.guardFields('/login')) },
    });
    assert.equal(none.status, 200);
    assert.equal(t.mail.length, sent);
    await t.http('/api/auth/email', {
      body: { email: 'grace@example.com', ...(await t.guardFields('/login')) },
    });
    assert.equal(t.mail.length, sent + 1);
    assert.equal(
      (await t.http('/api/auth/link', { body: { token: t.linkFrom(t.mail.at(-1)) } })).status,
      200,
    );
    t.jar.clear();
    assert.equal(
      (
        await t.http('/api/auth/login', {
          body: { login: 'grace', password: 'a long enough password' },
        })
      ).status,
      200,
    );
    // Logout ends the session server-side.
    await t.http('/api/auth/logout', { method: 'POST' });
    assert.equal((await t.http('/api/me')).status, 401);
  } finally {
    await t.close();
  }
});

// A software authenticator: ES256, attestation "none", as a platform passkey would.
function cbor(value) {
  const head = (major, n) =>
    n < 24
      ? Buffer.from([(major << 5) | n])
      : n < 256
        ? Buffer.from([(major << 5) | 24, n])
        : Buffer.from([(major << 5) | 25, n >> 8, n & 255]);
  if (typeof value === 'number') return value >= 0 ? head(0, value) : head(1, -1 - value);
  if (typeof value === 'string')
    return Buffer.concat([head(3, Buffer.byteLength(value)), Buffer.from(value)]);
  if (Buffer.isBuffer(value)) return Buffer.concat([head(2, value.length), value]);
  const entries = value instanceof Map ? [...value] : Object.entries(value);
  return Buffer.concat([
    head(5, entries.length),
    ...entries.flatMap(([k, v]) => [cbor(k), cbor(v)]),
  ]);
}
function authenticator() {
  const { privateKey, publicKey } = generateKeyPairSync('ec', { namedCurve: 'P-256' });
  const jwk = publicKey.export({ format: 'jwk' });
  const id = randomBytes(16);
  let counter = 0;
  const rpHash = sha(new URL(ORIGIN).hostname);
  const u32 = (n) => Buffer.from([n >>> 24, (n >>> 16) & 255, (n >>> 8) & 255, n & 255]);
  const clientData = (type, challenge) =>
    Buffer.from(JSON.stringify({ type, challenge, origin: ORIGIN, crossOrigin: false }));
  return {
    create(options) {
      const cose = cbor(
        new Map([
          [1, 2],
          [3, -7],
          [-1, 1],
          [-2, Buffer.from(jwk.x, 'base64url')],
          [-3, Buffer.from(jwk.y, 'base64url')],
        ]),
      );
      const authData = Buffer.concat([
        rpHash,
        Buffer.from([0x45]),
        u32(counter),
        Buffer.alloc(16),
        Buffer.from([0, id.length]),
        id,
        cose,
      ]);
      return {
        id: b64url(id),
        rawId: b64url(id),
        type: 'public-key',
        clientExtensionResults: {},
        response: {
          clientDataJSON: b64url(clientData('webauthn.create', options.challenge)),
          attestationObject: b64url(cbor({ fmt: 'none', attStmt: {}, authData })),
          transports: ['internal'],
        },
      };
    },
    get(options) {
      counter += 1;
      const data = clientData('webauthn.get', options.challenge);
      const authData = Buffer.concat([rpHash, Buffer.from([0x05]), u32(counter)]);
      return {
        id: b64url(id),
        rawId: b64url(id),
        type: 'public-key',
        clientExtensionResults: {},
        response: {
          clientDataJSON: b64url(data),
          authenticatorData: b64url(authData),
          signature: b64url(sign('sha256', Buffer.concat([authData, sha(data)]), privateKey)),
        },
      };
    },
  };
}

test('passkeys: register while signed in, then sign in with no username', async () => {
  const t = await boot();
  try {
    await t.signUp('linus', 'linus@example.com'); // no password: passkey-first account
    const key = authenticator();
    const { challengeId, options } = await (
      await t.http('/api/account/passkeys/options', { method: 'POST' })
    ).json();
    assert.equal(options.rp.id, 'localhost');
    assert.equal(options.authenticatorSelection.residentKey, 'required');
    const added = await t.http('/api/account/passkeys', {
      body: { challengeId, response: key.create(options), name: 'Laptop' },
    });
    assert.equal(added.status, 200, await added.clone().text());
    const [saved] = await (await t.http('/api/account/passkeys')).json();
    assert.equal(saved.name, 'Laptop');
    // The only credential cannot be removed.
    assert.equal(
      (await t.http(`/api/account/passkeys/${saved.id}`, { method: 'DELETE' })).status,
      409,
    );
    t.jar.clear();
    const login = await (await t.http('/api/auth/passkey/options', { method: 'POST' })).json();
    assert.deepEqual(login.options.allowCredentials, []);
    const ok = await t.http('/api/auth/passkey', {
      body: { challengeId: login.challengeId, response: key.get(login.options) },
    });
    assert.equal(ok.status, 200, await ok.clone().text());
    assert.equal((await (await t.http('/api/me')).json()).username, 'linus');
    // A challenge is single use.
    t.jar.clear();
    const replay = await t.http('/api/auth/passkey', {
      body: { challengeId: login.challengeId, response: key.get(login.options) },
    });
    assert.equal(replay.status, 400);
    // A key nobody registered is refused.
    const stranger = authenticator();
    const fresh = await (await t.http('/api/auth/passkey/options', { method: 'POST' })).json();
    assert.equal(
      (
        await t.http('/api/auth/passkey', {
          body: { challengeId: fresh.challengeId, response: stranger.get(fresh.options) },
        })
      ).status,
      401,
    );
  } finally {
    await t.close();
  }
});

test('OAuth 2.1: PKCE code flow for the w3bs CLI, rotating refresh tokens, reuse detection, revocation', async () => {
  const t = await boot();
  try {
    const meta = await (await t.http('/.well-known/oauth-authorization-server')).json();
    assert.equal(meta.issuer, ORIGIN);
    assert.deepEqual(meta.code_challenge_methods_supported, ['S256']);
    assert.deepEqual(meta.grant_types_supported, [
      'authorization_code',
      'refresh_token',
      'urn:ietf:params:oauth:grant-type:device_code',
    ]);
    const verifier = b64url(randomBytes(32));
    const challenge = b64url(sha(verifier));
    const redirect = 'http://127.0.0.1:49152/callback';
    const params = new URLSearchParams({
      client_id: 'w3bs-cli',
      redirect_uri: redirect,
      response_type: 'code',
      code_challenge: challenge,
      code_challenge_method: 'S256',
      scope: 'profile',
      state: 'xyz',
    });
    // Signed out: off to the login page and back.
    const out = await t.http(`/oauth/authorize?${params}`);
    assert.equal(out.status, 302);
    assert.match(out.headers.get('location'), /^\/login\?next=%2Foauth%2Fauthorize/);
    await t.signUp('barbara', 'barbara@example.com', 'barbara password 1');
    const consent = await t.http(`/oauth/authorize?${params}`);
    assert.equal(consent.status, 200);
    assert.match(
      consent.headers.get('content-security-policy'),
      /form-action 'self' http:\/\/127\.0\.0\.1:49152/,
    );
    assert.match(await consent.text(), /w3bs CLI/);
    // Denied.
    const denied = await t.http('/oauth/authorize', {
      form: { ...Object.fromEntries(params), decision: 'deny' },
    });
    assert.equal(
      new URL(denied.headers.get('location')).searchParams.get('error'),
      'access_denied',
    );
    // Allowed.
    const allowed = await t.http('/oauth/authorize', {
      form: { ...Object.fromEntries(params), decision: 'allow' },
    });
    assert.equal(allowed.status, 303);
    const back = new URL(allowed.headers.get('location'));
    assert.equal(back.origin + back.pathname, redirect);
    assert.equal(back.searchParams.get('state'), 'xyz');
    assert.equal(back.searchParams.get('iss'), ORIGIN);
    const code = back.searchParams.get('code');
    const exchange = (form) =>
      t.http('/oauth/token', {
        form,
        cookies: false,
        headers: { Origin: 'https://some-app.example' },
      });
    const badVerifier = await exchange({
      grant_type: 'authorization_code',
      code,
      redirect_uri: redirect,
      client_id: 'w3bs-cli',
      code_verifier: b64url(randomBytes(32)),
    });
    assert.equal(badVerifier.status, 400);
    assert.equal((await badVerifier.json()).error, 'invalid_grant');
    // The failed attempt spent the code; start again and do it right.
    const allowed2 = await t.http('/oauth/authorize', {
      form: { ...Object.fromEntries(params), decision: 'allow' },
    });
    const code2 = new URL(allowed2.headers.get('location')).searchParams.get('code');
    const good = await exchange({
      grant_type: 'authorization_code',
      code: code2,
      redirect_uri: redirect,
      client_id: 'w3bs-cli',
      code_verifier: verifier,
    });
    assert.equal(good.status, 200);
    assert.equal(good.headers.get('access-control-allow-origin'), '*');
    const tokens = await good.json();
    assert.equal(tokens.token_type, 'Bearer');
    assert.equal(tokens.scope, 'profile');
    const me = (access) =>
      t.http('/api/me', { cookies: false, headers: { Authorization: `Bearer ${access}` } });
    assert.equal((await (await me(tokens.access_token)).json()).username, 'barbara');
    // Replaying the code revokes what it produced.
    const replay = await exchange({
      grant_type: 'authorization_code',
      code: code2,
      redirect_uri: redirect,
      client_id: 'w3bs-cli',
      code_verifier: verifier,
    });
    assert.equal(replay.status, 400);
    assert.equal((await me(tokens.access_token)).status, 401);
    // A fresh grant to test refresh rotation.
    const allowed3 = await t.http('/oauth/authorize', {
      form: { ...Object.fromEntries(params), decision: 'allow' },
    });
    const code3 = new URL(allowed3.headers.get('location')).searchParams.get('code');
    const first = await (
      await exchange({
        grant_type: 'authorization_code',
        code: code3,
        redirect_uri: redirect,
        client_id: 'w3bs-cli',
        code_verifier: verifier,
      })
    ).json();
    const rotated = await (
      await exchange({
        grant_type: 'refresh_token',
        refresh_token: first.refresh_token,
        client_id: 'w3bs-cli',
      })
    ).json();
    assert.notEqual(rotated.refresh_token, first.refresh_token);
    assert.equal(
      (await me(first.access_token)).status,
      401,
      'refreshing retires the old access token',
    );
    assert.equal((await me(rotated.access_token)).status, 200);
    const grants = await (await t.http('/api/account/grants')).json();
    assert.equal(grants.length, 1);
    assert.equal(grants[0].app, 'w3bs CLI');
    // Presenting the spent refresh token kills the whole family.
    const reuse = await exchange({
      grant_type: 'refresh_token',
      refresh_token: first.refresh_token,
      client_id: 'w3bs-cli',
    });
    assert.equal(reuse.status, 400);
    assert.equal((await me(rotated.access_token)).status, 401);
    assert.equal(
      (
        await exchange({
          grant_type: 'refresh_token',
          refresh_token: rotated.refresh_token,
          client_id: 'w3bs-cli',
        })
      ).status,
      400,
    );
    // Revocation (RFC 7009) of a refresh token ends the sign-in.
    const allowed4 = await t.http('/oauth/authorize', {
      form: { ...Object.fromEntries(params), decision: 'allow' },
    });
    const code4 = new URL(allowed4.headers.get('location')).searchParams.get('code');
    const last = await (
      await exchange({
        grant_type: 'authorization_code',
        code: code4,
        redirect_uri: redirect,
        client_id: 'w3bs-cli',
        code_verifier: verifier,
      })
    ).json();
    assert.equal(
      (
        await t.http('/oauth/revoke', {
          form: { token: last.refresh_token, client_id: 'w3bs-cli' },
          cookies: false,
        })
      ).status,
      200,
    );
    assert.equal((await me(last.access_token)).status, 401);
  } finally {
    await t.close();
  }
});

test('OAuth 2.1: bad requests never redirect to unregistered URIs; PKCE and scopes are enforced', async () => {
  const t = await boot();
  try {
    await t.signUp('katherine', 'katherine@example.com', 'katherine password');
    const base = {
      client_id: 'w3bs-cli',
      response_type: 'code',
      code_challenge: b64url(sha('v'.repeat(43))),
      code_challenge_method: 'S256',
    };
    const evil = await t.http(
      `/oauth/authorize?${new URLSearchParams({ ...base, redirect_uri: 'https://evil.example/cb' })}`,
    );
    assert.equal(evil.status, 400);
    assert.equal(evil.headers.get('location'), null);
    const unknown = await t.http(
      `/oauth/authorize?${new URLSearchParams({ ...base, client_id: 'nope', redirect_uri: 'http://127.0.0.1:1/callback' })}`,
    );
    assert.equal(unknown.status, 400);
    const plain = await t.http(
      `/oauth/authorize?${new URLSearchParams({ ...base, code_challenge_method: 'plain', redirect_uri: 'http://127.0.0.1:1/callback', state: 's' })}`,
    );
    assert.equal(plain.status, 302);
    const sent = new URL(plain.headers.get('location'));
    assert.equal(sent.searchParams.get('error'), 'invalid_request');
    assert.equal(sent.searchParams.get('state'), 's');
    const scope = await t.http(
      `/oauth/authorize?${new URLSearchParams({ ...base, scope: 'admin', redirect_uri: 'http://127.0.0.1:1/callback' })}`,
    );
    assert.equal(new URL(scope.headers.get('location')).searchParams.get('error'), 'invalid_scope');
    const grant = await t.http('/oauth/token', {
      form: { grant_type: 'password', client_id: 'w3bs-cli' },
      cookies: false,
    });
    assert.equal((await grant.json()).error, 'unsupported_grant_type');

    // Developers register public clients with https or loopback redirect URIs.
    assert.equal(
      (
        await t.http('/api/account/apps', {
          body: { name: 'Bad', redirectUris: ['http://example.com/cb'] },
        })
      ).status,
      400,
    );
    const app = await (
      await t.http('/api/account/apps', {
        body: { name: 'My App', redirectUris: ['https://app.example/cb'] },
      })
    ).json();
    assert.match(app.id, /^app_/);
    const consent = await t.http(
      `/oauth/authorize?${new URLSearchParams({ ...base, client_id: app.id, redirect_uri: 'https://app.example/cb' })}`,
    );
    assert.equal(consent.status, 200);
    assert.match(await consent.text(), /My App/);
    assert.equal(
      (
        await t.http(
          `/oauth/authorize?${new URLSearchParams({ ...base, client_id: app.id, redirect_uri: 'https://app.example/cb2' })}`,
        )
      ).status,
      400,
    );
  } finally {
    await t.close();
  }
});

test('device flow (RFC 8628): w3bs login over SSH shows a code, approved at /device', async () => {
  const t = await boot();
  try {
    const meta = await (await t.http('/.well-known/oauth-authorization-server')).json();
    assert.equal(meta.device_authorization_endpoint, `${ORIGIN}/oauth/device_authorization`);
    assert.ok(meta.grant_types_supported.includes('urn:ietf:params:oauth:grant-type:device_code'));
    const start = await t.http('/oauth/device_authorization', {
      form: { client_id: 'w3bs-cli', scope: 'profile' },
      cookies: false,
      headers: { Origin: 'https://elsewhere.example' },
    });
    assert.equal(start.status, 200);
    const device = await start.json();
    assert.match(device.user_code, /^[BCDFGHJKLMNPQRSTVWXZ]{4}-[BCDFGHJKLMNPQRSTVWXZ]{4}$/);
    assert.equal(device.verification_uri, `${ORIGIN}/device`);
    assert.equal(device.verification_uri_complete, `${ORIGIN}/device?code=${device.user_code}`);
    const poll = () =>
      t.http('/oauth/token', {
        form: {
          grant_type: 'urn:ietf:params:oauth:grant-type:device_code',
          device_code: device.device_code,
          client_id: 'w3bs-cli',
        },
        cookies: false,
      });
    assert.equal((await (await poll()).json()).error, 'authorization_pending');
    assert.equal((await (await poll()).json()).error, 'slow_down');
    // Signed out, the page sends you to log in and back.
    const out = await t.http(`/device?code=${device.user_code}`);
    assert.equal(out.status, 302);
    assert.match(out.headers.get('location'), /^\/login\?next=%2Fdevice/);
    await t.signUp('margaret', 'margaret@example.com', 'margaret password');
    assert.equal((await t.http('/device?code=WXYZ-BCDF')).status, 400);
    // Codes are forgiving about case and the dash.
    const lower = device.user_code.toLowerCase().replace('-', '');
    const consent = await t.http(`/device?code=${lower}`);
    assert.equal(consent.status, 200);
    assert.match(await consent.text(), new RegExp(device.user_code));
    const approved = await t.http('/device', {
      form: { code: device.user_code, decision: 'allow' },
    });
    assert.match(await approved.text(), /Signed <em>in/);
    await new Promise((resolve) => setTimeout(resolve, 2100));
    const done = await poll();
    assert.equal(done.status, 200);
    const tokens = await done.json();
    const me = await t.http('/api/me', {
      cookies: false,
      headers: { Authorization: `Bearer ${tokens.access_token}` },
    });
    assert.equal((await me.json()).username, 'margaret');
    await new Promise((resolve) => setTimeout(resolve, 2100));
    assert.equal((await (await poll()).json()).error, 'expired_token', 'a device code works once');
    // Denied requests say so.
    const second = await (
      await t.http('/oauth/device_authorization', {
        form: { client_id: 'w3bs-cli' },
        cookies: false,
      })
    ).json();
    await t.http('/device', { form: { code: second.user_code, decision: 'deny' } });
    const deniedPoll = await t.http('/oauth/token', {
      form: {
        grant_type: 'urn:ietf:params:oauth:grant-type:device_code',
        device_code: second.device_code,
        client_id: 'w3bs-cli',
      },
      cookies: false,
    });
    assert.equal((await deniedPoll.json()).error, 'access_denied');
    assert.equal(
      (await t.http('/oauth/device_authorization', { form: { client_id: 'nope' }, cookies: false }))
        .status,
      401,
    );
  } finally {
    await t.close();
  }
});

test('loopback redirect URIs match any port, nothing else is loose', () => {
  const registered = ['http://127.0.0.1/callback', 'https://app.example/cb'];
  assert.ok(redirectAllowed(registered, 'http://127.0.0.1:5000/callback'));
  assert.ok(redirectAllowed(registered, 'https://app.example/cb'));
  assert.ok(!redirectAllowed(registered, 'http://127.0.0.1:5000/other'));
  assert.ok(!redirectAllowed(registered, 'http://localhost:5000/callback'));
  assert.ok(!redirectAllowed(registered, 'https://app.example/cb?x=1'));
  assert.ok(!redirectAllowed(registered, 'https://app.example:444/cb'));
  assert.ok(!redirectAllowed(registered, 'https://app.example/cb#frag'));
});
