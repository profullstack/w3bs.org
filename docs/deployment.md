# Deploying the reference stack

The service is deployment-ready source. Production launch requires publishing the repository and deploying the reviewed build, then connecting the four domains. No deployed status is implied by this document.

## Configuration

| Setting              | Production value                                                                                                         |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| `HOST`               | `0.0.0.0`                                                                                                                |
| `PORT`               | Supplied by the host                                                                                                     |
| `PUBLIC_ORIGIN`      | `https://w3bs.org`                                                                                                       |
| `ALLOWED_HOSTS`      | `w3bs.org,www.w3bs.org,specs.w3bs.org,browse.w3bs.org,prompt.w3bs.org` plus the exact generated preview hostname if used |
| `DATABASE_URL`       | PostgreSQL connection string. On Railway, `${{Postgres.DATABASE_URL}}` from the self-hosted Postgres service            |
| `W3BS_DATA_DIR`      | Only used without `DATABASE_URL`: SQLite directory for local development                                                 |
| `W3BS_PUBLISH_TOKEN` | Random administrative secret, at least 32 characters; absent means read-only                                             |
| `W3BS_TRUST_FILE`    | Optional path to a mounted operator-managed trust file; otherwise bundled example public keys                            |

Never deploy `.local/keys` or upload publisher private keys. The container needs public keys and pre-signed examples only. Store any administrative token in the provider's secret manager. Back up the PostgreSQL database and trust configuration. The schema is created on first start; the ten bundled examples are seeded once and never re-inserted.

## Railway

The production project is `w3bs` in the Profullstack workspace: a self-hosted PostgreSQL service (Railway's `postgres-ssl` image, one replica, its own volume) and the `w3bs` web service built from this repository's `Dockerfile` on every push to `main`. `railway.json` sets the health check to `/healthz`, which reports `"storage": "postgres"` when the database is in use.

The health endpoint accepts Railway's `healthcheck.railway.app` hostname; that exception does not permit access to other routes. No application volume is needed. If a database URL carries `sslmode=require` (Railway's public proxy uses a self-signed certificate), the connection uses TLS without certificate verification; the internal URL has no `sslmode` and runs in plain TCP inside the private network. Provider references: https://docs.railway.com/deployments/healthchecks and https://docs.railway.com/guides/postgresql

Add `w3bs.org`, `specs.w3bs.org`, `browse.w3bs.org` and `prompt.w3bs.org` as custom service domains. Read each exact DNS target and verification record from the hosting provider; do not guess them. Update the registrar records, replacing the parking records only for those requested website names. Wait for domain validation and certificate issuance.

The service is initially safe to run read-only. Publishing requires explicit administrative configuration; a successful read-only deployment is not evidence that publisher onboarding was tested against production credentials.

## Live acceptance

1. Confirm HTTPS 200 for all four roots and that each presents its intended surface.
2. Confirm discovery on all four `/.well-known/w3bs.json` endpoints.
3. Resolve `w3bs://prompt/w3bs/research@1` on each host. Compare full canonical identity, version, manifest, permissions, provenance and verification state.
4. Run the CLI and experimental native client with `W3BS_API=https://w3bs.org`.
5. Connect an MCP client to `https://w3bs.org/mcp` and compare `w3bs_resolve`.
6. Confirm the source repository and public issue process are accessible.
7. Record the deployed revision, observed timestamp and results. Do not call the public MVP launched until these gates pass.

## Current operational limits

The server bounds request and response sizes and verifies Host and Origin headers. Platform-level request limiting should be configured for an internet-facing service. No billing, arbitrary tool execution, third-party messages, remote model calls or device control is enabled by this stack.
