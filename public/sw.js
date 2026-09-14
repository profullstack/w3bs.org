// Deliberately cache only static assets. Resolution, manifests and revocation
// state always require the network; offline UI never claims current validity.
const CACHE = 'w3bs-assets-v1';
const ASSETS = [
  '/assets/style.css',
  '/assets/tokens.css',
  '/assets/app.js',
  '/assets/mark.svg',
  '/assets/offline.html',
];
self.addEventListener('install', (event) =>
  event.waitUntil(caches.open(CACHE).then((cache) => cache.addAll(ASSETS))),
);
self.addEventListener('activate', (event) =>
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(
          keys
            .filter((key) => key.startsWith('w3bs-assets-') && key !== CACHE)
            .map((key) => caches.delete(key)),
        ),
      ),
  ),
);
self.addEventListener('fetch', (event) => {
  const url = new URL(event.request.url);
  if (event.request.method !== 'GET' || url.origin !== self.location.origin) return;
  if (ASSETS.includes(url.pathname))
    event.respondWith(fetch(event.request).catch(() => caches.match(event.request)));
  else if (event.request.mode === 'navigate')
    event.respondWith(fetch(event.request).catch(() => caches.match('/assets/offline.html')));
});
