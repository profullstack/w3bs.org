// The newest W3BS OS installer ISO, for w3bs.org/os. .github/workflows/os-iso.yml publishes each
// tested build as a release os-v<version> and then overwrites latest.json in the fixed release
// `os-latest`; that is read here (release downloads are not API-rate-limited, the API is).
// Cached; an outage keeps the last answer. Without a checksum nothing is offered.
const LATEST =
  process.env.W3BS_OS_LATEST_URL ||
  'https://github.com/profullstack/w3bs.org/releases/download/os-latest/latest.json';
const TTL = 5 * 60 * 1000;

export const osReleaseSource = { fetchImpl: (...args) => fetch(...args) };
let cache = { at: 0, value: null };

const human = (bytes) =>
  bytes >= 1e9 ? `${(bytes / 1e9).toFixed(2)} GB` : `${Math.round(bytes / 1e6)} MB`;
const https = (url) =>
  typeof url === 'string' &&
  url.startsWith('https://github.com/profullstack/w3bs.org/releases/download/');

async function load() {
  const r = await osReleaseSource.fetchImpl(LATEST, { signal: AbortSignal.timeout(8000) });
  if (r.status === 404) return null;
  if (!r.ok) throw new Error(`latest.json: HTTP ${r.status}`);
  const x = await r.json();
  if (
    !/^\d+\.\d+\.\d+[0-9A-Za-z.+-]*$/.test(x.version ?? '') ||
    !/^[0-9a-f]{64}$/.test(x.sha256 ?? '') ||
    !/^w3bs-os-[0-9A-Za-z._+-]+\.iso$/.test(x.name ?? '') ||
    !https(x.url) ||
    !https(x.sumsUrl) ||
    !Number.isFinite(x.size)
  )
    return null;
  return {
    version: x.version,
    name: x.name,
    url: x.url,
    sumsUrl: x.sumsUrl,
    sha256: x.sha256,
    size: human(x.size),
    preview: Boolean(x.preview),
  };
}

export async function latestOsIso() {
  if (Date.now() - cache.at < TTL) return cache.value;
  try {
    cache = { at: Date.now(), value: await load() };
  } catch {
    cache = { at: Date.now() - TTL + 60_000, value: cache.value }; // retry in a minute
  }
  return cache.value;
}

export function resetOsReleaseCache() {
  cache = { at: 0, value: null };
}
