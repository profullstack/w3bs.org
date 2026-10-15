# W3BS Social: the AT Protocol PDS and its web client

`w3bs.org/social` is the app's page (`src/social-routes.mjs`). The server itself is
Bluesky's reference PDS (`ghcr.io/bluesky-social/pds`) on its own hostname,
`social.w3bs.org`, with handles under `*.social.w3bs.org`. An atproto PDS serves
`/xrpc`, `/oauth` and `/.well-known/atproto-did` at the root of its host, so it
cannot be mounted under a path of w3bs.org.

`https://social.w3bs.org/` itself is the Bluesky web client (bluesky-social/social-app,
its `bskyweb` server), so the host is a full social network UI. nginx sends the PDS
paths (`/xrpc/`, `/oauth/`, `/@atproto/`, `/.well-known/`, `/account`) to the PDS and
everything else to the client; `*.social.w3bs.org` handle hosts go straight to the PDS.

## Web client

- dev2, `~/www/w3bs.org-social-web`, container `w3bsorgsocialweb-web-1`, `127.0.0.1:3343`,
  image `w3bs-social-web:latest`, built locally from social-app's own `Dockerfile` after
  `rebrand.py` restyles it in the w3bs.org design: default PDS `social.w3bs.org`, the
  paper/ink/orange palette, the orbit mark and "w3bs." wordmark, a paper welcome dialog
  with a Georgia headline, "W3BS Social" titles, and the icons + social card in `brand/`.
- Rebuild (Metro wants ~8 GB heap and takes several minutes). `rebrand.py` exits on any
  patch that no longer applies, so a moved upstream file stops the build instead of
  shipping half a rebrand:

  ```sh
  git clone --depth 1 https://github.com/bluesky-social/social-app.git && cd social-app
  python3 ~/src/profullstack/w3bs.org/ops/social/rebrand.py .
  docker build --build-arg EXPO_PUBLIC_ENV=production -t w3bs-social-web:latest .
  cd ~/www/w3bs.org-social-web && docker compose -p w3bsorgsocialweb -f docker-compose.app.yml --env-file deploy.env up -d
  ```

## Where the PDS runs

- dev2, `~/www/w3bs.org-social`, container `w3bsorg-social-pds-1`, `127.0.0.1:3342`.
  `docker-compose.app.yml` here is the copy in that directory; `deploy.env` holds
  `APP_PORT=3342`. Data (SQLite + blobs) in `volumes/pds`.
- Secrets: `app.env` there, backed up in vault `profullstack w3bs-org-social--prod`
  (`PDS_JWT_SECRET`, `PDS_ADMIN_PASSWORD`, `PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX`,
  SMTP via the w3bs.org Resend domain). Losing the rotation key loses control of every
  account's did:plc, so never regenerate it.
- nginx: `proxies` on the w3bs.org site in cli-tools `dev2/sites.d/w3bs.org.json`
  (`dev2-site vhost w3bs.org`). The w3bs.org certificate covers `social.w3bs.org` and
  `*.social.w3bs.org` (Porkbun DNS-01, `dev2-site cert w3bs.org`).
- DNS (Porkbun): `A social` and `A *.social` -> dev2.

## Invites

`PDS_INVITE_REQUIRED=true`. A signed-in W3BS account gets one single-use code from
`POST /api/social/invite`; the app mints it with the PDS admin API and stores it in
`social_invites`. The app needs `W3BS_SOCIAL_ADMIN_PASSWORD` (= `PDS_ADMIN_PASSWORD`);
without it the page says invites are not open yet.
`W3BS_SOCIAL_PUBLIC_INVITE` is a shared multi-use code shown to everyone on the page
(with its uses left, read from `admin.getInviteCodes`, cached 5 min); when it is used up
or disabled the page falls back to the one-per-account flow. `W3BS_SOCIAL_PDS` overrides the PDS
URL (default `https://social.w3bs.org`).

## Admin

```sh
AP=$(grep ^PDS_ADMIN_PASSWORD= ~/www/w3bs.org-social/app.env | cut -d= -f2)
# a code by hand
curl -u admin:$AP -X POST https://social.w3bs.org/xrpc/com.atproto.server.createInviteCode \
  -H 'content-type: application/json' -d '{"useCount":1}'
# take an account down
curl -u admin:$AP -X POST https://social.w3bs.org/xrpc/com.atproto.admin.deleteAccount \
  -H 'content-type: application/json' -d '{"did":"did:plc:..."}'
# relay status (the Bluesky network crawls us)
curl 'https://bsky.network/xrpc/com.atproto.sync.getHostStatus?hostname=social.w3bs.org'
```

Upgrade: bump the image tag in `docker-compose.app.yml`, then
`docker compose -p w3bsorg-social -f docker-compose.app.yml --env-file deploy.env up -d`.
