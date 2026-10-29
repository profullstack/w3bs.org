# W3BS Net

W3BS Net is an independent namespace registry hosted by W3BS. Its PostgreSQL/SQLite tables are separate from Moshpit and its local records have priority during name lookup. Namespace and name requests remain private until an operator approves them. There are no sales, pre-buys, paid reservations or auction states.

The initial open W3BS namespaces are `.agent` and `.whatever`. W3BS reviews every new name request in both namespaces; Moshpit records are backfill and do not reserve or sell W3BS names. The launch seeds `riotcoder.agent` and `cha0spunk.agent` as active operator-created names without assigning ownership. Only the matching W3BS username can request a claim; each claim goes back through moderation.

## Public API

- `GET /api/net/v1/registry?q=...` returns local W3BS names first, then opted-in federation announcements, then the Moshpit backfill. The response labels each record with its source.
- `GET /api/net/v1/resolve/:name` resolves an exact name with the same local-first precedence.
- `GET /api/net/v1/events?since=0&limit=100` reads the append-only public event log. Keep the returned `sequence` as a cursor and request later events with `since`.
- `GET /api/net/v1/instances` and `GET /.well-known/w3bs-net.json` publish this instance and its opted-in peers.
- `GET /.well-known/w3bs-net.json` describes the instance and its endpoints.
- `POST /api/net/v1/federation/events` receives public announcements from an opted-in W3BS peer. It requires that peer's inbound bearer token. Imported peer announcements are attributed to their source and never become W3BS-owned registrations.

Local user requests use `POST /api/net/v1/tlds` and `POST /api/net/v1/names`. A signed-in W3BS browser session or a bearer minted for the `w3bs-cli` OAuth client by `w3bs login` is required. A successful request returns `202` and stays private until review.

## Operators and federation

Set `W3BS_NET_OPERATOR_USERNAMES` to a comma-separated list of W3BS usernames. Operators sign in to `/net/operator` with the browser session or use a CLI bearer from the existing W3BS OAuth 2.1 device authorization flow. The operator API controls moderation, peer opt-in, peer removal and backfill.

To connect two W3BS instances, each operator adds the other instance's HTTPS origin and the other instance's inbound token. The console creates a local inbound token once; share it with the remote operator and have them save it as their outbound token. Each instance sends public registration events to its opted-in peers and pulls their public event log from its stored cursor. Delivery retries with bounded exponential backoff.

The production daemon also imports public Moshpit allocation history and names in bounded pages. Moshpit records are stored with `https://pit.moshcode.sh` provenance and do not grant W3BS ownership. The importer reads Moshpit's public APIs; it does not attempt to claim names on behalf of users or mutate the Moshpit registry.

This event feed is a W3BS Net protocol surface inspired by relay and repository federation models. It is not a Nostr NIP-01 relay or an AT Protocol PDS/firehose implementation.

## Domain routing

An active name like `riotcoder.agent` resolves through `riotcoder.agent.w3bs.org`. Configure `W3BS_NET_HOSTED_TLDS` and a DNS wildcard, edge proxy host and TLS certificate for every suffix served here (the launch includes `*.whatever.w3bs.org` and `*.agent.w3bs.org`). If a name has a public HTTPS target, W3BS redirects to it. Otherwise the host serves the W3BS name record. Imported Moshpit names redirect to their source gateway when no local W3BS registration overrides them.
