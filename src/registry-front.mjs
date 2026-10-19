// `docker pull w3bs.org/os:server`: a read-only OCI registry front for the W3BS OS images.
// The images are built and signed in CI and pushed to GHCR, whose org policy keeps packages
// private; CI also copies every manifest and blob into the public GitHub release `os-images`
// of this repository (asset names `sha256-<hex>`, plus `tags.json` mapping tag -> digest).
// Here we answer the registry API: manifests are served from those bytes (their digest is
// re-checked before serving), blobs redirect to the release asset, and the Docker client
// verifies every digest it downloads.
import { createHash } from 'node:crypto';

export const REGISTRY_REPOS = ['os', 'w3bs-os'];
const RELEASE =
  process.env.W3BS_OS_IMAGES_RELEASE ||
  'https://github.com/profullstack/w3bs.org/releases/download/os-images';
const DIGEST = /^sha256:[0-9a-f]{64}$/;
const TAG = /^[A-Za-z0-9_][A-Za-z0-9._-]{0,127}$/;
const MANIFEST_TYPES = [
  'application/vnd.oci.image.index.v1+json',
  'application/vnd.oci.image.manifest.v1+json',
  'application/vnd.docker.distribution.manifest.list.v2+json',
  'application/vnd.docker.distribution.manifest.v2+json',
];
const TAGS_TTL = 60_000;

const asset = (digest) => `${RELEASE}/${digest.replace(':', '-')}`;
const sha256 = (bytes) => `sha256:${createHash('sha256').update(bytes).digest('hex')}`;

export function mountRegistryFront(app, { fetchImpl = fetch } = {}) {
  const manifests = new Map(); // digest -> { bytes, type }, immutable
  let tags = { at: 0, map: {} };

  const fail = (res, status, code, message) =>
    res
      .status(status)
      .set('Docker-Distribution-API-Version', 'registry/2.0')
      .json({ errors: [{ code, message }] });

  async function tagMap() {
    if (Date.now() - tags.at < TAGS_TTL) return tags.map;
    const r = await fetchImpl(`${RELEASE}/tags.json`, { signal: AbortSignal.timeout(10_000) });
    if (!r.ok) throw new Error(`tags.json: HTTP ${r.status}`);
    tags = { at: Date.now(), map: await r.json() };
    return tags.map;
  }

  async function manifest(digest) {
    if (manifests.has(digest)) return manifests.get(digest);
    const r = await fetchImpl(asset(digest), { signal: AbortSignal.timeout(20_000) });
    if (!r.ok) return null;
    const bytes = Buffer.from(await r.arrayBuffer());
    if (sha256(bytes) !== digest) throw new Error(`manifest ${digest} does not match its digest`);
    const type = JSON.parse(bytes.toString('utf8')).mediaType;
    if (!MANIFEST_TYPES.includes(type)) throw new Error(`manifest ${digest} has type ${type}`);
    const entry = { bytes, type };
    manifests.set(digest, entry);
    return entry;
  }

  const base = (res) => res.set('Docker-Distribution-API-Version', 'registry/2.0');
  app.get('/v2/', (_req, res) => base(res).json({}));

  app.all('/v2/:repo/manifests/:ref', async (req, res) => {
    if (!['GET', 'HEAD'].includes(req.method))
      return fail(res, 405, 'UNSUPPORTED', 'This registry is read-only.');
    if (!REGISTRY_REPOS.includes(req.params.repo))
      return fail(res, 404, 'NAME_UNKNOWN', 'Only w3bs.org/os is served here.');
    const ref = req.params.ref;
    try {
      const digest = DIGEST.test(ref) ? ref : TAG.test(ref) ? (await tagMap())[ref] : null;
      const found = digest && DIGEST.test(digest) ? await manifest(digest) : null;
      if (!found) return fail(res, 404, 'MANIFEST_UNKNOWN', `No manifest ${ref}.`);
      base(res)
        .set('Content-Type', found.type)
        .set('Docker-Content-Digest', digest)
        .set('Content-Length', String(found.bytes.length))
        .set(
          'Cache-Control',
          DIGEST.test(ref) ? 'public, max-age=31536000, immutable' : 'no-cache',
        );
      return req.method === 'HEAD' ? res.end() : res.end(found.bytes);
    } catch (error) {
      return fail(res, 502, 'UNAVAILABLE', `The image store is unavailable: ${error.message}`);
    }
  });

  app.all('/v2/:repo/blobs/:digest', (req, res) => {
    if (!['GET', 'HEAD'].includes(req.method))
      return fail(res, 405, 'UNSUPPORTED', 'This registry is read-only.');
    if (!REGISTRY_REPOS.includes(req.params.repo) || !DIGEST.test(req.params.digest))
      return fail(res, 404, 'BLOB_UNKNOWN', 'No such blob.');
    base(res).redirect(307, asset(req.params.digest));
  });

  app.all(/^\/v2\/.+/, (_req, res) =>
    fail(res, 404, 'NAME_UNKNOWN', 'Only w3bs.org/os is served here.'),
  );
}
