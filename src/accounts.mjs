// w3bs.org accounts: username + password and passkeys are the ways in, an emailed
// link is the fallback (and how an address is proven), and w3bs.org is an OAuth 2.1
// authorization server (authorization code + PKCE S256, rotating refresh tokens) so
// `w3bs login` and third-party apps can sign people in.
import { createHash, randomBytes, timingSafeEqual } from 'node:crypto';
import {
  generateAuthenticationOptions,
  generateRegistrationOptions,
  verifyAuthenticationResponse,
  verifyRegistrationResponse,
} from '@simplewebauthn/server';
import { W3bsError } from './core.mjs';

const tables = (serial) => `
  CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, username TEXT NOT NULL UNIQUE, email TEXT NOT NULL UNIQUE, password_hash TEXT, created_at TEXT NOT NULL);
  CREATE TABLE IF NOT EXISTS passkeys (id TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, public_key TEXT NOT NULL, counter INTEGER NOT NULL, transports TEXT NOT NULL, name TEXT NOT NULL, created_at TEXT NOT NULL, last_used_at TEXT);
  CREATE INDEX IF NOT EXISTS passkeys_user ON passkeys(user_id);
  CREATE TABLE IF NOT EXISTS sessions (id_hash TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, created_at TEXT NOT NULL, expires_at TEXT NOT NULL);
  CREATE TABLE IF NOT EXISTS auth_challenges (id TEXT PRIMARY KEY, challenge TEXT NOT NULL, user_id TEXT, purpose TEXT NOT NULL, expires_at TEXT NOT NULL);
  CREATE TABLE IF NOT EXISTS email_links (token_hash TEXT PRIMARY KEY, email TEXT NOT NULL, purpose TEXT NOT NULL, data TEXT NOT NULL, created_at TEXT NOT NULL, expires_at TEXT NOT NULL, used_at TEXT);
  CREATE INDEX IF NOT EXISTS email_links_email ON email_links(email, created_at);
  CREATE TABLE IF NOT EXISTS login_attempts (sequence ${serial}, user_id TEXT NOT NULL, ok INTEGER NOT NULL, at TEXT NOT NULL);
  CREATE INDEX IF NOT EXISTS login_attempts_user ON login_attempts(user_id, sequence);
  CREATE TABLE IF NOT EXISTS oauth_clients (id TEXT PRIMARY KEY, name TEXT NOT NULL, redirect_uris TEXT NOT NULL, owner_id TEXT REFERENCES users(id) ON DELETE CASCADE, created_at TEXT NOT NULL);
  CREATE TABLE IF NOT EXISTS oauth_codes (code_hash TEXT PRIMARY KEY, client_id TEXT NOT NULL, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, redirect_uri TEXT NOT NULL, challenge TEXT NOT NULL, scope TEXT NOT NULL, family TEXT NOT NULL, expires_at TEXT NOT NULL, used INTEGER NOT NULL);
  CREATE TABLE IF NOT EXISTS oauth_tokens (token_hash TEXT PRIMARY KEY, kind TEXT NOT NULL, family TEXT NOT NULL, client_id TEXT NOT NULL, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, scope TEXT NOT NULL, created_at TEXT NOT NULL, expires_at TEXT NOT NULL, used INTEGER NOT NULL, revoked INTEGER NOT NULL);
  CREATE INDEX IF NOT EXISTS oauth_tokens_family ON oauth_tokens(family);
  CREATE INDEX IF NOT EXISTS oauth_tokens_user ON oauth_tokens(user_id);`;
export const ACCOUNT_SCHEMA = {
  sqlite: `PRAGMA foreign_keys=ON;${tables('INTEGER PRIMARY KEY AUTOINCREMENT')}`,
  postgres: tables('BIGSERIAL PRIMARY KEY'),
};

export const CLI_CLIENT_ID = 'w3bs-cli';
const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
export const TTL = {
  session: 30 * DAY,
  challenge: 5 * MINUTE,
  emailLink: 20 * MINUTE,
  code: 10 * MINUTE,
  access: HOUR,
  refresh: 30 * DAY,
};
const LOCKOUT = { failures: 10, window: 15 * MINUTE };
const LINKS_PER_HOUR = 5;
export const SCOPES = ['profile'];
const RESERVED = new Set(
  'admin administrator root system support help security abuse postmaster webmaster hostmaster noreply no-reply mail www api app apps oauth auth login logout signup account accounts settings w3bs w3c registry repo packages specs browse prompt prompts desktop mcp docs status official staff team null undefined'.split(
    ' ',
  ),
);

const token = (bytes = 32) => randomBytes(bytes).toString('base64url');
export const sha256 = (value) => createHash('sha256').update(value).digest('hex');
const b64url = (bytes) => Buffer.from(bytes).toString('base64url');
const later = (ms) => new Date(Date.now() + ms).toISOString();
const nowIso = () => new Date().toISOString();
const fail = (code, message, status = 400) => new W3bsError(code, message, status);

export class OAuthError extends Error {
  constructor(error, description, status = 400) {
    super(description);
    this.error = error;
    this.status = status;
  }
}

export function normalizeUsername(value) {
  const username = String(value ?? '')
    .trim()
    .toLowerCase();
  if (!/^[a-z0-9](?:[a-z0-9-]{1,30})[a-z0-9]$/.test(username) || username.includes('--'))
    throw fail(
      'INVALID_USERNAME',
      'Usernames are 3 to 32 lowercase letters, digits and single hyphens, starting and ending with a letter or digit.',
    );
  if (RESERVED.has(username)) throw fail('USERNAME_TAKEN', 'That username is not available.', 409);
  return username;
}
export function normalizeEmail(value) {
  const email = String(value ?? '')
    .trim()
    .toLowerCase();
  if (email.length > 254 || !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email))
    throw fail('INVALID_EMAIL', 'Enter a valid email address.');
  return email;
}
function checkPassword(value) {
  if (typeof value !== 'string' || value.length < 10 || value.length > 256)
    throw fail('INVALID_PASSWORD', 'Passwords are 10 to 256 characters.');
  return value;
}
const hashPassword = (password) => Bun.password.hash(password, { algorithm: 'argon2id' });
let decoy;
const decoyHash = async () => (decoy ??= await hashPassword(token()));

// Loopback redirects (RFC 8252 section 7.3): a registered http://127.0.0.1/path or
// http://[::1]/path matches any port, because a native app picks a free one at run time.
const LOOPBACK = new Set(['127.0.0.1', '[::1]']);
export function redirectAllowed(registered, requested) {
  let want;
  try {
    want = new URL(requested);
  } catch {
    return false;
  }
  if (want.hash) return false;
  return registered.some((uri) => {
    const have = new URL(uri);
    if (have.protocol === 'http:' && LOOPBACK.has(have.hostname))
      return (
        want.protocol === 'http:' &&
        want.hostname === have.hostname &&
        want.pathname === have.pathname &&
        want.search === have.search
      );
    return uri === requested;
  });
}
export function checkRedirectUri(value) {
  let url;
  try {
    url = new URL(value);
  } catch {
    throw fail('INVALID_REDIRECT_URI', `Not a URL: ${value}`);
  }
  if (url.hash) throw fail('INVALID_REDIRECT_URI', 'Redirect URIs cannot have a fragment.');
  const loopback = url.protocol === 'http:' && LOOPBACK.has(url.hostname);
  if (url.protocol !== 'https:' && !loopback)
    throw fail(
      'INVALID_REDIRECT_URI',
      'Redirect URIs use https, or http://127.0.0.1 / http://[::1] for native apps.',
    );
  if (url.username || url.password) throw fail('INVALID_REDIRECT_URI', 'No credentials in URIs.');
  return loopback ? `${url.protocol}//${url.hostname}${url.pathname}${url.search}` : url.href;
}

export function createAccounts(db, { origin }) {
  const rpID = new URL(origin).hostname;
  const expectedOrigin = new URL(origin).origin;
  const sweep = async () => {
    const now = nowIso();
    await db.run('DELETE FROM auth_challenges WHERE expires_at < ?', [now]);
    await db.run('DELETE FROM sessions WHERE expires_at < ?', [now]);
    await db.run('DELETE FROM oauth_codes WHERE expires_at < ?', [now]);
    await db.run('DELETE FROM oauth_tokens WHERE expires_at < ?', [now]);
    await db.run('DELETE FROM email_links WHERE expires_at < ?', [later(-DAY)]);
  };

  async function seedClients() {
    await db.run(
      'INSERT INTO oauth_clients(id, name, redirect_uris, owner_id, created_at) VALUES (?, ?, ?, NULL, ?) ON CONFLICT (id) DO UPDATE SET name = excluded.name, redirect_uris = excluded.redirect_uris',
      [
        CLI_CLIENT_ID,
        'w3bs CLI',
        JSON.stringify(['http://127.0.0.1/callback', 'http://[::1]/callback']),
        nowIso(),
      ],
    );
  }

  const publicUser = (row) =>
    row && {
      id: row.id,
      username: row.username,
      email: row.email,
      hasPassword: Boolean(row.password_hash),
      createdAt: row.created_at,
    };
  const userById = async (id) => db.get('SELECT * FROM users WHERE id = ?', [id]);
  const userByLogin = async (login) => {
    const value = String(login ?? '')
      .trim()
      .toLowerCase();
    if (!value || value.length > 254) return undefined;
    return db.get(`SELECT * FROM users WHERE ${value.includes('@') ? 'email' : 'username'} = ?`, [
      value,
    ]);
  };

  // Sessions. The cookie holds a random token; the table holds only its hash.
  async function createSession(userId) {
    const id = token();
    await db.run('INSERT INTO sessions VALUES (?, ?, ?, ?)', [
      sha256(id),
      userId,
      nowIso(),
      later(TTL.session),
    ]);
    if (Math.random() < 0.05) await sweep();
    return id;
  }
  async function sessionUser(id) {
    if (!id || id.length > 100) return undefined;
    const row = await db.get(
      'SELECT users.* FROM sessions JOIN users ON users.id = sessions.user_id WHERE sessions.id_hash = ? AND sessions.expires_at > ?',
      [sha256(id), nowIso()],
    );
    return publicUser(row);
  }
  const endSession = (id) => id && db.run('DELETE FROM sessions WHERE id_hash = ?', [sha256(id)]);

  // Email links: proof of an address. Signup links create the account; sign-in links
  // are the fallback way in. Neither ever says whether an address has an account.
  async function issueLink(email, purpose, data = {}) {
    const recent = await db.get(
      'SELECT COUNT(*) AS n FROM email_links WHERE email = ? AND created_at > ?',
      [email, later(-HOUR)],
    );
    if (Number(recent.n) >= LINKS_PER_HOUR) return undefined;
    const value = token();
    await db.run('INSERT INTO email_links VALUES (?, ?, ?, ?, ?, ?, NULL)', [
      sha256(value),
      email,
      purpose,
      JSON.stringify(data),
      nowIso(),
      later(TTL.emailLink),
    ]);
    return value;
  }
  async function startSignup({ username, email, password }) {
    username = normalizeUsername(username);
    email = normalizeEmail(email);
    const passwordHash =
      password === undefined || password === ''
        ? null
        : await hashPassword(checkPassword(password));
    if (await db.get('SELECT id FROM users WHERE username = ?', [username]))
      throw fail('USERNAME_TAKEN', 'That username is taken.', 409);
    const existing = await db.get('SELECT username FROM users WHERE email = ?', [email]);
    // An address that already has an account gets a sign-in link instead, so the form
    // answers the same either way.
    if (existing)
      return {
        email,
        purpose: 'login',
        link: await issueLink(email, 'login'),
        username: existing.username,
      };
    return {
      email,
      purpose: 'signup',
      link: await issueLink(email, 'signup', { username, passwordHash }),
      username,
    };
  }
  async function startEmailLogin(value) {
    const email = normalizeEmail(value);
    const user = await db.get('SELECT username FROM users WHERE email = ?', [email]);
    if (!user) return { email };
    return {
      email,
      purpose: 'login',
      link: await issueLink(email, 'login'),
      username: user.username,
    };
  }
  async function peekLink(value) {
    if (typeof value !== 'string' || value.length > 100) return undefined;
    const row = await db.get(
      'SELECT purpose, email FROM email_links WHERE token_hash = ? AND used_at IS NULL AND expires_at > ?',
      [sha256(value), nowIso()],
    );
    return row && { purpose: row.purpose, email: row.email };
  }
  async function consumeLink(value) {
    if (typeof value !== 'string' || value.length > 100)
      throw fail('INVALID_LINK', 'This link is invalid or has expired.');
    return db.transaction(async (tx) => {
      const row = await tx.get(
        'SELECT * FROM email_links WHERE token_hash = ? AND used_at IS NULL AND expires_at > ?',
        [sha256(value), nowIso()],
      );
      if (!row)
        throw fail('INVALID_LINK', 'This link is invalid, used or has expired. Request a new one.');
      await tx.run('UPDATE email_links SET used_at = ? WHERE token_hash = ?', [
        nowIso(),
        row.token_hash,
      ]);
      let user = await tx.get('SELECT * FROM users WHERE email = ?', [row.email]);
      if (row.purpose === 'signup' && !user) {
        const { username, passwordHash } = JSON.parse(row.data);
        if (await tx.get('SELECT id FROM users WHERE username = ?', [username]))
          throw fail(
            'USERNAME_TAKEN',
            `The username ${username} was taken meanwhile. Sign up again.`,
            409,
          );
        user = {
          id: token(16),
          username,
          email: row.email,
          password_hash: passwordHash,
          created_at: nowIso(),
        };
        await tx.run('INSERT INTO users VALUES (?, ?, ?, ?, ?)', [
          user.id,
          user.username,
          user.email,
          user.password_hash,
          user.created_at,
        ]);
        return { user: publicUser(user), created: true };
      }
      if (!user) throw fail('INVALID_LINK', 'This link is invalid or has expired.');
      // Signing in by email is a success, so it clears any password lockout.
      await tx.run('INSERT INTO login_attempts(user_id, ok, at) VALUES (?, 1, ?)', [
        user.id,
        nowIso(),
      ]);
      return { user: publicUser(user), created: false };
    });
  }

  // Passwords. Every failure reads the same and costs the same, unknown logins verify
  // against a decoy hash, and failures since the last success lock the password (never
  // the account: the emailed link still works).
  async function passwordLogin(login, password) {
    const wrong = () => fail('INVALID_LOGIN', 'That username or password is not right.', 401);
    if (typeof password !== 'string' || password.length > 256) throw wrong();
    const user = await userByLogin(login);
    if (!user?.password_hash) {
      await Bun.password.verify(password, await decoyHash());
      throw wrong();
    }
    const since = later(-LOCKOUT.window);
    const last = await db.get(
      'SELECT MAX(sequence) AS s FROM login_attempts WHERE user_id = ? AND ok = 1',
      [user.id],
    );
    const failures = await db.get(
      'SELECT COUNT(*) AS n FROM login_attempts WHERE user_id = ? AND ok = 0 AND at > ? AND sequence > ?',
      [user.id, since, Number(last?.s ?? 0)],
    );
    if (Number(failures.n) >= LOCKOUT.failures)
      throw fail(
        'LOCKED',
        'Too many wrong passwords. Sign in with an email link instead, or wait 15 minutes.',
        429,
      );
    const ok = await Bun.password.verify(password, user.password_hash);
    await db.run('INSERT INTO login_attempts(user_id, ok, at) VALUES (?, ?, ?)', [
      user.id,
      ok ? 1 : 0,
      nowIso(),
    ]);
    if (!ok) throw wrong();
    return publicUser(user);
  }
  async function setPassword(userId, password) {
    const hash = password === null ? null : await hashPassword(checkPassword(password));
    if (hash === null && !(await db.get('SELECT id FROM passkeys WHERE user_id = ?', [userId])))
      throw fail('LAST_CREDENTIAL', 'Add a passkey before removing your password.', 409);
    await db.run('UPDATE users SET password_hash = ? WHERE id = ?', [hash, userId]);
  }

  // Passkeys (WebAuthn). Challenges live server-side for five minutes and are single use.
  async function saveChallenge(challenge, purpose, userId = null) {
    const id = token(16);
    await db.run('INSERT INTO auth_challenges VALUES (?, ?, ?, ?, ?)', [
      id,
      challenge,
      userId,
      purpose,
      later(TTL.challenge),
    ]);
    return id;
  }
  async function takeChallenge(id, purpose) {
    if (typeof id !== 'string' || id.length > 64) throw fail('INVALID_CHALLENGE', 'Start again.');
    return db.transaction(async (tx) => {
      const row = await tx.get(
        'SELECT * FROM auth_challenges WHERE id = ? AND purpose = ? AND expires_at > ?',
        [id, purpose, nowIso()],
      );
      await tx.run('DELETE FROM auth_challenges WHERE id = ?', [id]);
      if (!row) throw fail('INVALID_CHALLENGE', 'That passkey request expired. Try again.');
      return row;
    });
  }
  async function passkeyRegistrationOptions(user) {
    const existing = await db.all('SELECT id, transports FROM passkeys WHERE user_id = ?', [
      user.id,
    ]);
    const options = await generateRegistrationOptions({
      rpName: 'W3BS',
      rpID,
      userName: user.username,
      userDisplayName: user.username,
      userID: Buffer.from(user.id),
      attestationType: 'none',
      excludeCredentials: existing.map((row) => ({
        id: row.id,
        transports: JSON.parse(row.transports),
      })),
      authenticatorSelection: { residentKey: 'required', userVerification: 'preferred' },
    });
    return { challengeId: await saveChallenge(options.challenge, 'register', user.id), options };
  }
  async function passkeyRegister(user, { challengeId, response, name }) {
    const challenge = await takeChallenge(challengeId, 'register');
    if (challenge.user_id !== user.id) throw fail('INVALID_CHALLENGE', 'Start again.');
    let result;
    try {
      result = await verifyRegistrationResponse({
        response,
        expectedChallenge: challenge.challenge,
        expectedOrigin,
        expectedRPID: rpID,
        requireUserVerification: false,
      });
    } catch (error) {
      throw fail('PASSKEY_REJECTED', `Passkey not accepted: ${error.message}`);
    }
    if (!result.verified) throw fail('PASSKEY_REJECTED', 'Passkey not accepted.');
    const { credential } = result.registrationInfo;
    const label =
      String(name ?? '')
        .trim()
        .slice(0, 60) || 'Passkey';
    await db.run('INSERT INTO passkeys VALUES (?, ?, ?, ?, ?, ?, ?, NULL)', [
      credential.id,
      user.id,
      b64url(credential.publicKey),
      credential.counter,
      JSON.stringify(credential.transports ?? []),
      label,
      nowIso(),
    ]);
    return { id: credential.id, name: label };
  }
  async function passkeyLoginOptions() {
    const options = await generateAuthenticationOptions({
      rpID,
      allowCredentials: [],
      userVerification: 'preferred',
    });
    return { challengeId: await saveChallenge(options.challenge, 'login'), options };
  }
  async function passkeyLogin({ challengeId, response }) {
    const challenge = await takeChallenge(challengeId, 'login');
    const rejected = () => fail('PASSKEY_REJECTED', 'That passkey is not registered here.', 401);
    if (typeof response?.id !== 'string') throw rejected();
    const row = await db.get('SELECT * FROM passkeys WHERE id = ?', [response.id]);
    if (!row) throw rejected();
    let result;
    try {
      result = await verifyAuthenticationResponse({
        response,
        expectedChallenge: challenge.challenge,
        expectedOrigin,
        expectedRPID: rpID,
        credential: {
          id: row.id,
          publicKey: Buffer.from(row.public_key, 'base64url'),
          counter: Number(row.counter),
          transports: JSON.parse(row.transports),
        },
        requireUserVerification: false,
      });
    } catch {
      throw rejected();
    }
    if (!result.verified) throw rejected();
    await db.run('UPDATE passkeys SET counter = ?, last_used_at = ? WHERE id = ?', [
      result.authenticationInfo.newCounter,
      nowIso(),
      row.id,
    ]);
    await db.run('INSERT INTO login_attempts(user_id, ok, at) VALUES (?, 1, ?)', [
      row.user_id,
      nowIso(),
    ]);
    return publicUser(await userById(row.user_id));
  }
  const listPasskeys = async (userId) =>
    (
      await db.all(
        'SELECT id, name, created_at, last_used_at FROM passkeys WHERE user_id = ? ORDER BY created_at',
        [userId],
      )
    ).map((row) => ({
      id: row.id,
      name: row.name,
      createdAt: row.created_at,
      lastUsedAt: row.last_used_at,
    }));
  async function deletePasskey(userId, id) {
    const user = await userById(userId);
    const count = await db.get('SELECT COUNT(*) AS n FROM passkeys WHERE user_id = ?', [userId]);
    if (!user.password_hash && Number(count.n) <= 1)
      throw fail(
        'LAST_CREDENTIAL',
        'Set a password or add another passkey before removing this one.',
        409,
      );
    await db.run('DELETE FROM passkeys WHERE id = ? AND user_id = ?', [id, userId]);
  }

  // OAuth 2.1 clients. All clients are public (PKCE, no secret); w3bs-cli is seeded.
  async function client(id) {
    if (typeof id !== 'string' || id.length > 64) return undefined;
    const row = await db.get('SELECT * FROM oauth_clients WHERE id = ?', [id]);
    return (
      row && {
        id: row.id,
        name: row.name,
        redirectUris: JSON.parse(row.redirect_uris),
        ownerId: row.owner_id,
        createdAt: row.created_at,
      }
    );
  }
  async function createClient(ownerId, { name, redirectUris }) {
    const label = String(name ?? '').trim();
    if (label.length < 2 || label.length > 60)
      throw fail('INVALID_NAME', 'App names are 2 to 60 characters.');
    if (!Array.isArray(redirectUris) || !redirectUris.length || redirectUris.length > 10)
      throw fail('INVALID_REDIRECT_URI', 'Give 1 to 10 redirect URIs.');
    const uris = redirectUris.map(checkRedirectUri);
    const count = await db.get('SELECT COUNT(*) AS n FROM oauth_clients WHERE owner_id = ?', [
      ownerId,
    ]);
    if (Number(count.n) >= 20)
      throw fail('TOO_MANY_APPS', 'Accounts can register up to 20 apps.', 409);
    const id = `app_${token(12)}`;
    await db.run('INSERT INTO oauth_clients VALUES (?, ?, ?, ?, ?)', [
      id,
      label,
      JSON.stringify(uris),
      ownerId,
      nowIso(),
    ]);
    return client(id);
  }
  const listClients = async (ownerId) =>
    Promise.all(
      (
        await db.all('SELECT id FROM oauth_clients WHERE owner_id = ? ORDER BY created_at', [
          ownerId,
        ])
      ).map((row) => client(row.id)),
    );
  async function deleteClient(ownerId, id) {
    await db.transaction(async (tx) => {
      const row = await tx.get('SELECT id FROM oauth_clients WHERE id = ? AND owner_id = ?', [
        id,
        ownerId,
      ]);
      if (!row) throw fail('NOT_FOUND', 'No such app.', 404);
      await tx.run('DELETE FROM oauth_tokens WHERE client_id = ?', [id]);
      await tx.run('DELETE FROM oauth_codes WHERE client_id = ?', [id]);
      await tx.run('DELETE FROM oauth_clients WHERE id = ?', [id]);
    });
  }

  // Validates an authorization request. Errors about the client or redirect URI must
  // never redirect (the URI is not trusted yet); everything else may.
  async function authorizationRequest(query) {
    const app = await client(query.client_id);
    if (!app) throw new OAuthError('invalid_client', 'Unknown client_id.');
    const redirectUri = String(query.redirect_uri ?? '');
    if (!redirectAllowed(app.redirectUris, redirectUri))
      throw new OAuthError('invalid_request', 'redirect_uri is not registered for this client.');
    const request = {
      client: app,
      redirectUri,
      state: query.state === undefined ? undefined : String(query.state),
    };
    const refuse = (error, description) =>
      Object.assign(new OAuthError(error, description), { redirect: request });
    if (query.response_type !== 'code')
      throw refuse('unsupported_response_type', 'Only response_type=code is supported.');
    if (query.code_challenge_method !== 'S256')
      throw refuse('invalid_request', 'PKCE is required: code_challenge_method=S256.');
    if (!/^[A-Za-z0-9_-]{43,128}$/.test(String(query.code_challenge ?? '')))
      throw refuse(
        'invalid_request',
        'code_challenge must be a base64url SHA-256 of the verifier.',
      );
    const scope = String(query.scope ?? 'profile')
      .split(' ')
      .filter(Boolean);
    if (!scope.length || scope.some((item) => !SCOPES.includes(item)))
      throw refuse('invalid_scope', `Supported scopes: ${SCOPES.join(' ')}.`);
    if (request.state && request.state.length > 500)
      throw refuse('invalid_request', 'state is too long.');
    return { ...request, challenge: query.code_challenge, scope: [...new Set(scope)].join(' ') };
  }
  async function issueCode(userId, request) {
    const code = token();
    await db.run('INSERT INTO oauth_codes VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0)', [
      sha256(code),
      request.client.id,
      userId,
      request.redirectUri,
      request.challenge,
      request.scope,
      token(16),
      later(TTL.code),
    ]);
    return code;
  }
  async function issueTokens(tx, { family, clientId, userId, scope }) {
    const access = token();
    const refresh = token();
    const created = nowIso();
    await tx.run('INSERT INTO oauth_tokens VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, 0)', [
      sha256(access),
      'access',
      family,
      clientId,
      userId,
      scope,
      created,
      later(TTL.access),
    ]);
    await tx.run('INSERT INTO oauth_tokens VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, 0)', [
      sha256(refresh),
      'refresh',
      family,
      clientId,
      userId,
      scope,
      created,
      later(TTL.refresh),
    ]);
    return {
      access_token: access,
      token_type: 'Bearer',
      expires_in: TTL.access / 1000,
      refresh_token: refresh,
      scope,
    };
  }
  const revokeFamily = (tx, family) =>
    tx.run('UPDATE oauth_tokens SET revoked = 1 WHERE family = ?', [family]);
  async function exchange(body) {
    const clientId = String(body.client_id ?? '');
    if (!(await client(clientId)))
      throw new OAuthError('invalid_client', 'Unknown client_id.', 401);
    const outcome = await db.transaction(async (tx) => {
      if (body.grant_type === 'authorization_code') {
        const row = await tx.get('SELECT * FROM oauth_codes WHERE code_hash = ?', [
          sha256(String(body.code ?? '')),
        ]);
        if (!row || row.client_id !== clientId || row.expires_at < nowIso())
          return {
            error: new OAuthError('invalid_grant', 'The authorization code is invalid or expired.'),
          };
        if (Number(row.used)) {
          // A replayed code means it leaked: kill whatever it already produced.
          await revokeFamily(tx, row.family);
          return {
            error: new OAuthError('invalid_grant', 'The authorization code was already used.'),
          };
        }
        await tx.run('UPDATE oauth_codes SET used = 1 WHERE code_hash = ?', [row.code_hash]);
        if (String(body.redirect_uri ?? '') !== row.redirect_uri)
          return {
            error: new OAuthError(
              'invalid_grant',
              'redirect_uri does not match the authorization request.',
            ),
          };
        const verifier = String(body.code_verifier ?? '');
        if (!/^[A-Za-z0-9._~-]{43,128}$/.test(verifier))
          return {
            error: new OAuthError('invalid_grant', 'code_verifier is missing or malformed.'),
          };
        const expected = Buffer.from(createHash('sha256').update(verifier).digest('base64url'));
        const given = Buffer.from(row.challenge);
        if (expected.length !== given.length || !timingSafeEqual(expected, given))
          return {
            error: new OAuthError('invalid_grant', 'code_verifier does not match code_challenge.'),
          };
        return {
          tokens: await issueTokens(tx, {
            family: row.family,
            clientId,
            userId: row.user_id,
            scope: row.scope,
          }),
        };
      }
      if (body.grant_type === 'refresh_token') {
        const row = await tx.get(
          "SELECT * FROM oauth_tokens WHERE token_hash = ? AND kind = 'refresh'",
          [sha256(String(body.refresh_token ?? ''))],
        );
        if (!row || row.client_id !== clientId || row.expires_at < nowIso() || Number(row.revoked))
          return {
            error: new OAuthError(
              'invalid_grant',
              'The refresh token is invalid, expired or revoked.',
            ),
          };
        if (Number(row.used)) {
          // Refresh tokens rotate; presenting a spent one means two parties hold it.
          await revokeFamily(tx, row.family);
          return {
            error: new OAuthError(
              'invalid_grant',
              'The refresh token was already used; this sign-in is revoked.',
            ),
          };
        }
        await tx.run('UPDATE oauth_tokens SET used = 1 WHERE token_hash = ?', [row.token_hash]);
        await tx.run("UPDATE oauth_tokens SET revoked = 1 WHERE family = ? AND kind = 'access'", [
          row.family,
        ]);
        return {
          tokens: await issueTokens(tx, {
            family: row.family,
            clientId,
            userId: row.user_id,
            scope: row.scope,
          }),
        };
      }
      return {
        error: new OAuthError('unsupported_grant_type', 'Use authorization_code or refresh_token.'),
      };
    });
    if (outcome.error) throw outcome.error;
    return outcome.tokens;
  }
  async function revokeToken(value, clientId) {
    const row = await db.get(
      'SELECT family, kind, client_id FROM oauth_tokens WHERE token_hash = ?',
      [sha256(String(value ?? ''))],
    );
    if (!row || (clientId && row.client_id !== clientId)) return;
    if (row.kind === 'refresh') await revokeFamily(db, row.family);
    else
      await db.run('UPDATE oauth_tokens SET revoked = 1 WHERE token_hash = ?', [
        sha256(String(value)),
      ]);
  }
  async function bearerUser(value) {
    if (typeof value !== 'string' || value.length > 100) return undefined;
    const row = await db.get(
      "SELECT users.*, oauth_tokens.scope AS scope, oauth_tokens.client_id AS client_id FROM oauth_tokens JOIN users ON users.id = oauth_tokens.user_id WHERE oauth_tokens.token_hash = ? AND oauth_tokens.kind = 'access' AND oauth_tokens.revoked = 0 AND oauth_tokens.expires_at > ?",
      [sha256(value), nowIso()],
    );
    return row && { ...publicUser(row), scope: row.scope, clientId: row.client_id };
  }
  // Apps a user has signed in to: one row per live refresh family.
  async function grants(userId) {
    const rows = await db.all(
      "SELECT oauth_tokens.family, oauth_tokens.client_id, oauth_tokens.scope, oauth_tokens.created_at, oauth_clients.name FROM oauth_tokens JOIN oauth_clients ON oauth_clients.id = oauth_tokens.client_id WHERE oauth_tokens.user_id = ? AND oauth_tokens.kind = 'refresh' AND oauth_tokens.used = 0 AND oauth_tokens.revoked = 0 AND oauth_tokens.expires_at > ? ORDER BY oauth_tokens.created_at DESC",
      [userId, nowIso()],
    );
    return rows.map((row) => ({
      id: row.family,
      clientId: row.client_id,
      app: row.name,
      scope: row.scope,
      lastRefreshedAt: row.created_at,
    }));
  }
  const revokeGrant = (userId, family) =>
    db.run('UPDATE oauth_tokens SET revoked = 1 WHERE family = ? AND user_id = ?', [
      family,
      userId,
    ]);

  return {
    seedClients,
    createSession,
    sessionUser,
    endSession,
    startSignup,
    startEmailLogin,
    peekLink,
    consumeLink,
    passwordLogin,
    setPassword,
    passkeyRegistrationOptions,
    passkeyRegister,
    passkeyLoginOptions,
    passkeyLogin,
    listPasskeys,
    deletePasskey,
    client,
    createClient,
    listClients,
    deleteClient,
    authorizationRequest,
    issueCode,
    exchange,
    revokeToken,
    bearerUser,
    grants,
    revokeGrant,
  };
}
