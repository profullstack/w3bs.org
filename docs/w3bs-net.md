# W3BS Net

W3BS Net is an independent namespace registry hosted by W3BS. Its PostgreSQL/SQLite tables are separate from Moshpit and its local records have priority during name lookup. Namespace and name requests remain private until an operator approves them. There are no sales, pre-buys, paid reservations or auction states.

The initial open W3BS namespaces are `.agent` and `.whatever`. W3BS reviews every new name request in both namespaces; Moshpit records are backfill and do not reserve or sell W3BS names. The launch seeds `riotcoder.agent` and `cha0spunk.agent` as active operator-created names without assigning ownership. Only the matching W3BS username can request a claim; each claim goes back through moderation.

## Public API

- `GET /api/net/v1/registry?q=...` returns local W3BS names first, then opted-in federation announcements, then the Moshpit backfill. The response labels each record with its source.
- `GET /api/net/v1/resolve/:name` resolves an exact name with the same local-first precedence.
- `GET /api/net/v1/events?since=0&limit=100` reads the append-only public event log. Keep the returned `sequence` as a cursor and request later events with `since`.
- `GET /api/net/v1/instances` and `GET /.well-known/w3bs-net.json` publish this instance, discovered instance announcements and opted-in peers.
- `GET /.well-known/w3bs-net.json` describes the instance and its endpoints.
- `POST /api/net/v1/federation/events` receives public announcements from an opted-in W3BS peer. It requires that peer's inbound bearer token. Imported peer announcements are attributed to their source and never become W3BS-owned registrations.

Local user requests use `POST /api/net/v1/tlds` and `POST /api/net/v1/names`. A signed-in W3BS browser session or a bearer minted for the `w3bs-cli` OAuth client by `w3bs login` is required. A successful request returns `202` and stays private until review.

## Operators and federation

Set `W3BS_NET_OPERATOR_USERNAMES` to a comma-separated list of W3BS usernames. Operators sign in to `/net/operator` with the browser session or use a CLI bearer from the existing W3BS OAuth 2.1 device authorization flow. The operator API controls moderation, peer opt-in, peer removal and backfill.

To connect two W3BS instances, each operator adds the other instance's HTTPS origin and the other instance's inbound token. The console creates a local inbound token once; share it with the remote operator and have them save it as their outbound token. Instances announce themselves and their public event streams. Each instance pulls from its direct peers, records the cursor, and relays new public events to its other opted-in peers. Delivery retries with bounded exponential backoff.

Peer delivery is authenticated with exchanged bearer tokens. Event IDs are SHA-256 integrity hashes, not cryptographic origin signatures, so operators should only connect peers they trust to relay accurate origin attribution.

The production daemon also imports public Moshpit allocation history and names in bounded pages. Moshpit records are stored with `https://pit.moshcode.sh` provenance and do not grant W3BS ownership. The importer reads Moshpit's public APIs; it does not attempt to claim names on behalf of users or mutate the Moshpit registry.

This event feed is a W3BS Net protocol surface inspired by relay and repository federation models. It is not a Nostr NIP-01 relay or an AT Protocol PDS/firehose implementation.

## Domain routing

An active name like `riotcoder.agent` resolves through `riotcoder.agent.w3bs.org`. Configure `W3BS_NET_HOSTED_TLDS` and a DNS wildcard, edge proxy host and TLS certificate for every suffix served here (the launch includes `*.whatever.w3bs.org`, `*.agent.w3bs.org` and `*.hacker.w3bs.org` for Moshpit backfill). If a name has a public HTTPS target, W3BS redirects to it. Otherwise the host serves the W3BS name record. Imported Moshpit names redirect to their source gateway when no local W3BS registration overrides them.

## Instance DNS records

W3BS is an independent Moshpit instance: W3BS names, owner ids and DNS writes stay in its database. Primary Moshpit records are read-only discovery backfill, not a grant to edit W3BS names. `/net/records` follows the primary Moshpit records workflow: filter names owned by the signed-in local account, publish AAAA/CNAME/MX/TXT with TTL and MX priority, inspect public values and remove individual records. Pending or suspended names/namespaces cannot publish. CNAME is exclusive with other types. The public read API is `/api/net/v1/dns/records?name=label.namespace`; authenticated mutations are `POST /api/net/v1/names/:name/records` and `DELETE /api/net/v1/names/:name/records/:id`. The existing W3BS session or OAuth bearer authenticates writes.

A configured Moshpit resolver maps the W3BS realm `label.namespace.w3bs.org` to this instance, querying its resolve and records APIs. It does not treat bare `label.namespace` on the primary network as the same ownership. Active instance records are authoritative within that explicitly configured realm, including over public parking answers; inactive local names cannot be revived through primary-network fallback. A name with no custom address records goes to the W3BS gateway so its existing HTTPS destination can redirect there. Custom AAAA or CNAME records override the gateway address; TXT/MX coexist with its default address. DNS changes publish immediately in the instance; cached resolver/browser answers still follow TTL.

DNS answers and HTTPS certificates are separate rollout gates. Standard browser access to every new `*.namespace.w3bs.org` requires public DNS and a publicly trusted certificate for that suffix. A generic `*.w3bs.org` certificate does not cover two labels. The primary Moshpit CA intentionally refuses public TLDs and must not be changed to mint arbitrary `.org` certificates to work around missing registrar credentials. Resolver routing is not evidence that public DNS or HTTPS is ready. Preserve that distinction when reporting a new name live.

### URL redirects

At `/net/records`, each active name you own has a URL redirect form. Enter a complete HTTPS URL (including a path), choose **301 permanent** or **302 temporary**, then save. Existing registration destinations appear here as 302 redirects. Saving/removing gives inline feedback without refreshing the page. A 301 can be remembered by browsers; use 302 while testing.

Authenticated owner API:

```http
PUT /api/net/v1/names/blue.example/redirect
Content-Type: application/json

{"target":"https://example.com/page","status":301}
```

`DELETE` at that same endpoint removes the destination. Only the local owner of an active name in an active namespace can change it. Redirect changes update the resolver record and publish a federation activation event. Redirects and AAAA/CNAME records are mutually exclusive, since those records route visitors away from the gateway. TXT/MX records can coexist. Redirects require working gateway DNS and HTTPS; setting a URL does not provision a public certificate or change registrar DNS.
