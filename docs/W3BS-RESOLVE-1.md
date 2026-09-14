# W3BS-RESOLVE-1: Discovery and resolution

Status: **Community Draft**, draft 1, 2026-09-14. Not a Recommendation.

## Bootstrap

An operator MAY serve `/.well-known/w3bs.json` over HTTPS with its registry, resolution, search, manifest, MCP, schema, trust and specification locations. Endpoints in the bootstrap document are informational until accepted by local policy. A client MUST NOT treat a discovered public key as self-authenticating.

The reference client uses an explicit operator-selected registry origin through `W3BS_API`. HTTPS is mandatory except for loopback development. Redirects are rejected to prevent accidental credential forwarding or changes of registry. The server does not fetch arbitrary URLs from manifests, follow dependencies, probe mirrors, query DNS records or use a DHT in draft 1.

## Resolution algorithm

1. Parse the URI using W3BS-URI-1. Reject malformed or ambiguous identifiers.
2. Find resources with the same type, namespace and name.
3. Select the requested exact version, the highest numeric version in a major, or the highest version when no selector is present.
4. Return the stored immutable manifest. Keep revoked versions visible; do not downgrade.
5. Validate the manifest and compute signature verification against locally pinned namespace keys.
6. Check key and artifact revocation. Return its state without converting it into instruction authority.

The successful JSON envelope is:

```json
{
  "requestedUri": "w3bs://prompt/w3bs/research@1",
  "canonicalUri": "w3bs://prompt/w3bs/research@1.0.0",
  "manifest": { "id": "w3bs://prompt/w3bs/research@1.0.0", "...": "full signed manifest" },
  "verification": { "valid": true, "authority": "DATA", "...": "full verification fields" }
}
```

The ellipses above are explanatory, not valid manifest fields.

## Surfaces and caching

HTTP exposes `GET /api/resolve?uri=...` and `POST /api/resolve` with `{ "uri": "..." }`. The web inspector embeds the same envelope for machine-readable inspection. CLI, MCP and native clients consume the same contract. A resolver must never redefine identity based on which surface requested it.

All reference resolution and verification responses are `Cache-Control: no-store`. The PWA caches static assets only. Offline clients MUST NOT label stale revocation state as current. Draft 1 trusts the configured HTTPS registry for current revocation state; offline signed status lists and decentralized revocation are not yet implemented.

Errors use `{ "error": { "code": "...", "message": "..." } }`. Malformed requests are 400, unauthorized access 401, policy failures 403, missing resources 404, immutable version conflicts 409 and disabled publishing 503. An invalid signature can remain inspectable but MUST prevent `run`.

## Mirror and transport roadmap

Future profiles must specify mirror integrity checks, cache freshness, namespace collision handling, signed catalog negotiation and DHT/P2P bootstrap. HTTP is the reference transport, not an architectural requirement for a W3BS identity.

Reference: https://www.rfc-editor.org/rfc/rfc8615
