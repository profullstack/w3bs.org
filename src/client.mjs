import { readFileSync } from 'node:fs';
import { W3bsError, verifyManifest, parseUri } from './core.mjs';

export function apiOrigin(value = process.env.W3BS_API || 'https://w3bs.org') {
  const url = new URL(value);
  if (url.username || url.password || url.search || url.hash || url.pathname !== '/')
    throw new W3bsError(
      'INVALID_API',
      'API must be an origin without credentials, paths, query or fragment.',
    );
  if (
    url.protocol !== 'https:' &&
    !(url.protocol === 'http:' && ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname))
  )
    throw new W3bsError('HTTPS_REQUIRED', 'Use HTTPS except for loopback development.');
  return url.origin;
}
export async function request(
  operation,
  args,
  { origin, token = process.env.W3BS_PUBLISH_TOKEN, trustStore } = {},
) {
  const base = apiOrigin(origin);
  const response = await fetch(`${base}/api/${operation}`, {
    method: 'POST',
    redirect: 'error',
    signal: AbortSignal.timeout(15000),
    headers: {
      'Content-Type': 'application/json',
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    },
    body: JSON.stringify(args),
  });
  const chunks = [];
  let size = 0;
  for await (const chunk of response.body) {
    size += chunk.length;
    if (size > 2_000_000)
      throw new W3bsError('RESPONSE_TOO_LARGE', 'Registry response exceeded 2 MB.');
    chunks.push(chunk);
  }
  const result = JSON.parse(Buffer.concat(chunks).toString('utf8'));
  if (!response.ok)
    throw new W3bsError(
      result.error?.code || 'REQUEST_FAILED',
      result.error?.message || `HTTP ${response.status}`,
      response.status,
    );
  if (result.manifest) {
    const trust =
      trustStore ||
      JSON.parse(
        readFileSync(
          process.env.W3BS_TRUST_FILE || new URL('../fixtures/trust.json', import.meta.url),
          'utf8',
        ),
      );
    const local = verifyManifest(result.manifest, trust, result.verification.revoked);
    if (
      result.canonicalUri !== result.manifest.id ||
      JSON.stringify(local) !== JSON.stringify(result.verification)
    )
      throw new W3bsError(
        'SURFACE_MISMATCH',
        'Local manifest verification disagrees with the registry.',
        502,
      );
    const requested = parseUri(args.uri || args.manifest.id),
      actual = parseUri(result.canonicalUri);
    if (
      requested.base !== actual.base ||
      (requested.version &&
        (requested.version.includes('.')
          ? actual.version !== requested.version
          : actual.version.split('.')[0] !== requested.version))
    )
      throw new W3bsError(
        'SURFACE_MISMATCH',
        'Registry returned a different resource than requested.',
        502,
      );
  }
  return result;
}
