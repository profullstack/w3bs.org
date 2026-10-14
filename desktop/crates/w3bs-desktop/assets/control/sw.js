// W3BS Desktop Control service worker: caches the app shell only, so the installed app opens
// (and explains itself) while the broker is down. It never caches or serves data: /control/api/*
// and /control/login always go to the network.
const CACHE = 'w3bs-control-shell-v1';
const SHELL = ['/control/', '/control/app.js', '/control/app.css', '/control/icon.svg', '/control/icon-192.png', '/control/icon-512.png', '/control/manifest.webmanifest'];

self.addEventListener('install', (e) => {
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(SHELL)).then(() => self.skipWaiting()));
});

self.addEventListener('activate', (e) => {
  e.waitUntil(caches.keys().then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k)))).then(() => self.clients.claim()));
});

self.addEventListener('fetch', (e) => {
  const req = e.request;
  const url = new URL(req.url);
  if (req.method !== 'GET' || url.origin !== self.location.origin || !SHELL.includes(url.pathname)) return;
  // Network first, so an upgraded broker's shell wins; the cache is only the offline fallback.
  e.respondWith(
    fetch(req)
      .then((res) => {
        if (res.ok) {
          const copy = res.clone();
          caches.open(CACHE).then((c) => c.put(url.pathname, copy));
        }
        return res;
      })
      .catch(() => caches.match(url.pathname)),
  );
});
