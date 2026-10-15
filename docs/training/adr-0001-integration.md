# ADR 0001: How W3BS Training joins the w3bs.org repository

Status: accepted for M1 (local vertical slice). Revisit at M2 (hosted app).
Date: 2026-10-15. Context: [PRD](prd.md), section 3.3.

## Recorded versions

| Component                                          | Version in this change                   |
| -------------------------------------------------- | ---------------------------------------- |
| Bun (root `packageManager`, CI `bun-version-file`) | 1.4.2                                    |
| Node.js (CLI also runs on it)                      | 24 (tested 24.21.0); needs `node:sqlite` |
| Express (existing host)                            | 5.1.0                                    |
| `@modelcontextprotocol/sdk`                        | 1.30.0                                   |
| `@profullstack/hqtui` (terminal review)            | 0.10.3, pinned exactly                   |
| zod (schema source of truth)                       | 4.3.6                                    |
| Rust desktop workspace                             | unchanged by this ADR (desktop 0.3.0)    |

No new lockfile. One dependency (`@profullstack/hqtui`) was added through the
root `bun.lock`.

## Decisions

### 1. Package layout: one `training/` tree, not new workspaces yet

The PRD suggests `apps/training-web`, `packages/training-*` and
`services/training-worker`. The repository has no workspaces today, and adding
them changes how `bun install`, the Dockerfile and CI resolve modules. That is
a reviewed change of its own. Until it is needed, the same boundaries live as
directories:

| PRD package                | Here                                                                                     |
| -------------------------- | ---------------------------------------------------------------------------------------- |
| `packages/training-schema` | `training/schema/` (zod source, generated JSON Schema, fixtures in `training/fixtures/`) |
| `packages/training-core`   | `training/core/` (service, store, ingest, privacy, zip, crypto, segmentation)            |
| `packages/training-cli`    | `training/bin/w3bs-training.mjs`                                                         |
| `packages/training-mcp`    | `training/mcp.mjs`                                                                       |
| `packages/training-client` | `training/http.mjs` (server contract; a client is not needed yet)                        |
| `apps/training-web`        | `training/ui/` (local app); hosted app is M2                                             |
| `services/training-worker` | not needed: M1 has no background processing                                              |

Each directory imports only downward (ui/mcp/bin -> http -> core -> schema), so
splitting them into workspaces later is a move, not a rewrite.

### 2. Language: ESM JavaScript, not TypeScript, for M1

The existing host is plain `.mjs` with zod at the boundaries. The PRD asks to
follow the host's runtime conventions and also names TypeScript packages. M1
follows the host: no build step and the same check script. The record contract
is zod (`training/schema/format.mjs`), which also generates the published JSON
Schema, so types are not lost. Converting to TypeScript is mechanical and can
come with the workspace split.

### 3. Web stack: deviation from Next 16 / Hono, flagged for M2

The PRD selects Next 16, React 19, Hono and Tailwind 4/shadcn for the new web
app. M1 ships no hosted app, so nothing here commits to or against that stack.
The local review app is a small dependency-free page served by Express, the
framework already in the host, because:

- it must run from the CLI with no build and no Node install beyond the CLI's
  own runtime;
- it is a security boundary (loopback, one-time launch link), and fewer moving
  parts make that boundary easier to review.

**Open question for M2:** run Next/Hono as a second process behind the
existing host (which then needs a supervisor and a session bridge) or mount the
`/api/v1/training` Express router from `training/http.mjs` directly in
`src/server.mjs` and build the hosted UI on the existing renderer. The router
already has the PRD's resource layout so either choice keeps the contract.

### 4. Routing and sessions (M2, decided now so M1 does not paint us in)

- `/training` (public page) and `/training/app` (workspace app) are routes of
  the existing host. `/api/v1/training` mounts `trainingRouter` from
  `training/http.mjs`.
- Identity is the existing W3BS account (magic link + passkey). The hosted
  router receives an `actorFor(req)` that maps the existing session to
  `{ kind: 'human', id }` and an API key/OAuth bearer to
  `{ kind: 'agent', id }`. No second identity database.
- Production metadata goes to the existing Postgres (same tables as
  `training/core/store.mjs`, `BIGSERIAL` where SQLite has `AUTOINCREMENT`).
  Artifacts go to private object storage, never public URLs.

### 5. Human versus agent

Policy is enforced in `training/core/service.mjs`. Each surface derives the
actor from its own channel:

| Surface                         | Actor                                                                                                             |
| ------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| MCP (`w3bs training mcp serve`) | always `agent`, named by `--consumer`                                                                             |
| CLI, human-only command         | `human` only after an interactive terminal shows a fresh code and the person types it back; refused without a TTY |
| CLI, other commands             | `human` on a TTY, otherwise `agent` (affects audit only)                                                          |
| Local app                       | `human`, after the one-time launch link created an HttpOnly session                                               |
| Hosted app (M2)                 | from the account session or bearer                                                                                |

This does not defend against malware or an agent that controls a real
terminal as the same user; see the [threat model](threat-model.md).

### 6. Recorder boundary

Recording belongs to the Rust broker (`desktop/`), exposed as
`w3bs desktop training ...`. The Training app never captures anything itself
and cannot turn capture on. The recorder hands sessions to Training as native
bundles (the same import path as any external data), so a recorder bug cannot
bypass import validation. That work is M0/M1 in the desktop workspace and is
not part of this change.

### 7. Runtime for distribution

`w3bs-training` runs under Bun or Node 24. The PRD requires that users not
install Node. Two options remain open for the release that registers
`w3bs-training` in the W3BS package catalog: a `bun build --compile` binary
(needs the UI assets embedded), or bundling inside the Electron Training
desktop app. Until then it runs from a checkout (`bun run training`).

## Licenses

The training code is MIT like the rest of the root project. The desktop
workspace remains Apache-2.0. Dataset rights are separate and travel in each
bundle's `rights.json`.
