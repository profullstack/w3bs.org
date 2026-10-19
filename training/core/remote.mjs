// Talking to a hosted W3BS Training workspace (w3bs.org/training) from the local CLI. It signs in
// with the session `w3bs login` saved (~/.config/w3bs/auth.json, one entry per origin) and
// refreshes it the same way the w3bs CLI does (OAuth 2.1 refresh grant, rotating tokens).
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, resolve } from 'node:path';
import { W3bsError } from '../../src/core.mjs';

const CLIENT_ID = 'w3bs-cli';
const SLACK_S = 60;

export function authPath(env = process.env) {
  const base = env.XDG_CONFIG_HOME || resolve(env.HOME || homedir(), '.config');
  return resolve(base, 'w3bs/auth.json');
}

const notSignedIn = (origin) =>
  new W3bsError(
    'NOT_SIGNED_IN',
    `Not signed in to ${origin}. Run: w3bs login${origin === 'https://w3bs.org' ? '' : ` --api ${origin}`}`,
    401,
  );

export async function accessToken(
  origin,
  { env = process.env, fetchImpl = fetch, now = () => Date.now() } = {},
) {
  const path = authPath(env);
  const all = existsSync(path) ? JSON.parse(readFileSync(path, 'utf8')) : {};
  const creds = all[origin];
  if (!creds?.access_token) throw notSignedIn(origin);
  if (creds.expires_at > now() / 1000 + SLACK_S) return creds.access_token;
  const meta = await (await fetchImpl(`${origin}/.well-known/oauth-authorization-server`)).json();
  if (new URL(meta.token_endpoint).origin !== origin)
    throw new W3bsError('AUTH_UNSUPPORTED', `token endpoint is not on ${origin}`, 502);
  const res = await fetchImpl(meta.token_endpoint, {
    method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({
      grant_type: 'refresh_token',
      refresh_token: creds.refresh_token,
      client_id: CLIENT_ID,
    }),
  });
  const v = await res.json().catch(() => ({}));
  if (!res.ok) {
    if (v.error === 'invalid_grant')
      throw new W3bsError('NOT_SIGNED_IN', 'Your session has ended. Run: w3bs login', 401);
    throw new W3bsError('OAUTH_ERROR', v.error_description || v.error || `HTTP ${res.status}`, 502);
  }
  all[origin] = {
    access_token: v.access_token,
    refresh_token: v.refresh_token,
    expires_at: Math.floor(now() / 1000) + (v.expires_in ?? 3600),
    username: creds.username,
  };
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  const tmp = `${path}.${process.pid}.tmp`;
  writeFileSync(tmp, JSON.stringify(all, null, 2), { mode: 0o600 });
  renameSync(tmp, path);
  return v.access_token;
}

// Upload a bundle to the hosted workspace and commit it. The server re-proposes every label, as
// any import does, so the person confirms there again.
export async function pushBundle(origin, bundle, { token, filename, fetchImpl = fetch }) {
  const api = `${origin}/api/v1/training`;
  const call = async (path, init) => {
    const res = await fetchImpl(`${api}${path}`, {
      ...init,
      headers: { authorization: `Bearer ${token}`, ...init.headers },
    });
    const v = await res.json().catch(() => ({}));
    if (!res.ok)
      throw Object.assign(
        new W3bsError(
          v.error?.code || 'PUSH_FAILED',
          v.error?.message || `HTTP ${res.status}`,
          res.status,
        ),
        { details: v.error?.details },
      );
    return v;
  };
  const preview = await call('/imports', {
    method: 'POST',
    body: bundle,
    headers: { 'content-type': 'application/octet-stream', 'x-filename': filename },
  });
  const committed = await call(`/imports/${preview.import_id}/complete`, {
    method: 'POST',
    headers: {},
  });
  return { ...committed, preview: preview.preview, review: `${origin}/training/app/#/inbox` };
}

async function callApi(origin, path, { token, method, body, fetchImpl = fetch }) {
  const res = await fetchImpl(`${origin}${path}`, {
    method,
    headers: {
      authorization: `Bearer ${token}`,
      ...(body === undefined ? {} : { 'content-type': 'application/json' }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const v = await res.json().catch(() => ({}));
  if (!res.ok)
    throw new W3bsError(
      v.error?.code || 'REQUEST_FAILED',
      v.error?.message || `HTTP ${res.status}`,
      res.status,
    );
  return v;
}

// Publish an OpenTask (made by service.publishTask after the person confirmed) to
// <origin>/tasks, optionally adding it to one of their workflows by title.
export const publishTask = (origin, task, { token, workflow, fetchImpl }) =>
  callApi(origin, '/api/v1/tasks', { token, method: 'POST', body: { task, workflow }, fetchImpl });

export const unpublishTask = (origin, id, { token, fetchImpl }) =>
  callApi(origin, `/api/v1/tasks/${encodeURIComponent(id)}`, {
    token,
    method: 'DELETE',
    fetchImpl,
  });
