# Build once. Meet every surface.

The reference stack implements an experimental W3BS profile with a signed registry, resolver, web inspector, CLI, HTTP API, MCP server and independently implemented native verifier. The server runs on Bun 1.4 (pinned by `packageManager` in package.json); the CLI and native client also run on Node.js 24 or later.

## Start locally

From the repository directory:

```sh
bun install
bun run start
```

Open `http://localhost:3000` in your browser and set `W3BS_API=http://localhost:3000` in your shell. Ten signed resources seed the persistent SQLite registry on first start. The private example signing key is not required to run or deploy it.

The standards, gateway and registry work at `/specs`, `/browse` and `/prompts`. The production subdomains are deployment targets; these instructions do not imply they are live.

## One resource, three interfaces

Install the `w3bs` CLI (one binary: packages, resources and more) with `curl -fsSL https://w3bs.org/install.sh | sh`. The Node reference client is still `node src/cli.mjs` (`w3bs-reference`).

```sh
W3BS_API=http://localhost:3000 w3bs resolve w3bs://prompt/w3bs/research@1
W3BS_API=http://localhost:3000 w3bs verify w3bs://prompt/w3bs/research@1
curl --get http://localhost:3000/api/resolve --data-urlencode 'uri=w3bs://prompt/w3bs/research@1'
```

Remote MCP clients connect to `/mcp` using Streamable HTTP. Local MCP clients can launch the stdio bridge:

```json
{
  "mcpServers": {
    "w3bs": {
      "command": "node",
      "args": ["/absolute/path/to/w3bs.org/src/mcp-stdio.mjs"],
      "env": { "W3BS_API": "http://localhost:3000" }
    }
  }
}
```

Both MCP transports expose `w3bs_search`, `w3bs_resolve`, `w3bs_inspect`, `w3bs_verify`, `w3bs_publish`, `w3bs_run`, `w3bs_revoke` and `w3bs_conformance`. Tool arguments match the corresponding HTTP operation body. `/api` publishes descriptions and JSON input schemas. `/schemas/manifest.json` publishes the manifest schema.

## Render a prompt

Create `inputs.json` containing `{ "topic": "Open protocols for sensor data" }`, then:

```sh
W3BS_API=http://localhost:3000 w3bs resource run w3bs://prompt/w3bs/research@1 --inputs inputs.json --consent
```

This runs a deterministic template renderer. The output is instructions, not an AI-generated research answer. It never calls a model or tool. Supply the rendered prompt to a separately authorized model runtime if needed. Manifests requesting tools or capabilities are rejected by this runtime.

Inputs also come from `--input NAME=VALUE` (repeatable) and from piped text, which fills the input named `input` or a prompt's only input. `<namespace>/<name>` is shorthand for `w3bs://prompt/<namespace>/<name>`.

## Run a prompt on a model

`--via` is the separately authorized runtime: the CLI renders, then pipes the instructions to a peer CLI you installed and prints its answer. W3BS holds no model keys and calls no provider itself.

| `--via`    | Runs                              | `--model`                                  | Install                                  |
| ---------- | --------------------------------- | ------------------------------------------ | ---------------------------------------- |
| `moshcode` | `moshcode oneshot <engine> -`     | the engine, required (`claude`, `codex`, …; see `moshcode oneshot --list`) | `w3bs install moshcode.sh/moshcode`      |
| `infernet` | `infernet chat [--model M]`       | optional model, e.g. `qwen2.5:7b`          | `w3bs install infernetprotocol.com/infernet` |
| `c0mpute`  | `c0mpute infernet chat [--model M]` | optional model                           | `w3bs install c0mpute.com/c0mpute`       |

```sh
pbpaste | w3bs resource run fabric/extract-wisdom --consent --via moshcode --model claude
w3bs resource run fabric/translate --input lang_code=fr --consent --via infernet < notes.md
```

`infernet chat` uses the Infernet network when its config sets a control plane, and its local engine otherwise. `--json` reports `modelInvoked: true` and the runner command alongside the rendered output and the answer.

## Fabric patterns

The registry carries 261 of [Fabric](https://github.com/danielmiessler/Fabric)'s MIT-licensed patterns as `w3bs://prompt/fabric/<name>@1.0.0` (Fabric's `extract_wisdom` is `fabric/extract-wisdom`). `bun scripts/import-fabric.mjs <Fabric checkout>` regenerates them into `fixtures/resources/fabric/`, signed with the `w3bs-fabric-1` key; the signature attests the conversion, and `NOTICE` there carries Fabric's license. Patterns the template runtime cannot carry unchanged are skipped rather than rewritten: three exceed the 50,000-character instruction limit and one is literal nuclei `{{var}}` syntax.

## Publish your first artifact

1. Generate an Ed25519 publisher key with `w3bs resource keygen --out .local/my-key --namespace example`.
2. Have the registry operator add the `public.json` entry to its pinned trust store. The namespace must be approved; do not send your private key.
3. Set the same public trust policy for your client through `W3BS_TRUST_FILE`. A registry response alone does not establish client trust.
4. Fork an example with the command below, edit its instructions and metadata, then sign it locally.
5. Have the operator enable publishing with a random `W3BS_PUBLISH_TOKEN` of at least 32 characters. Publish using that token through an authorized CLI or MCP environment, or HTTP Authorization header.

```sh
w3bs resource fork w3bs://prompt/w3bs/research@1 --namespace example --name research --key-id example-1 --out .local/draft.json
w3bs resource sign .local/draft.json --key .local/my-key/private.pem --out .local/signed.json
w3bs resource publish .local/signed.json
```

The CLI's fork command initially proposes the reference registry's manifest URL. Change `locations.canonical` to your actual registry if publishing elsewhere, and update publisher display metadata before signing. A new artifact version is immutable once accepted; use `version URI --version 1.1.0 --out draft.json` to prepare an unsigned revision.

The bearer token has registry-wide administrative write authority. It is not a per-publisher OAuth credential. The reference release supports operator-managed publication, not self-service accounts. Namespace signatures are required in addition to the token.

## Verify, save and revoke

The CLI and native client pin the bundled example public key by default. Use `W3BS_TRUST_FILE` for another independently approved trust policy. A verified artifact can be saved using `install URI --out resource.json`; this does not execute code or modify your system's protocol handlers.

`revoke URI --reason 'Reason for withdrawal'` requires the same write authorization and a full version. The artifact stays inspectable, but verification fails and `run` refuses it. Aliases keep the newest revoked version visible instead of silently falling back.

<span id="native"></span>

## Native client

The experimental native client has its own URI parser, JSON canonicalization and signature verification code, without importing the reference resolver or CLI implementation:

```sh
W3BS_API=http://localhost:3000 node native/client.mjs w3bs://prompt/w3bs/research@1 --json
node native/install-linux.mjs --install
```

The second command explicitly registers `w3bs://` for your Linux user. Opening such a URI independently verifies its manifest, then opens the HTTPS inspection view in your default browser, which may be TronBrowser. It is a protocol handler with a browser-based inspection surface, not a Chromium engine modification or an independently maintained external implementation. macOS supports manual `--open`; automatic registration and Windows support remain future work.

<span id="accounts"></span>

## Accounts and Sign in with W3BS

A W3BS account signs you in to w3bs.org, the `w3bs` CLI and apps that offer "Sign in with W3BS". Create one at `/signup`: pick a username, confirm your email from the link we send, then add a passkey (or keep the password you chose). Passkeys and username/password are the ways in; an emailed link is the fallback for a lost password or a device without your passkey.

```sh
w3bs login     # opens the browser, signs this computer in
w3bs whoami    # alice (alice@example.com) on https://w3bs.org
w3bs logout    # revokes the sign-in and forgets it
```

Over SSH, or on a machine without a display, `w3bs login` shows a code instead of opening a browser: open `https://w3bs.org/device` on any device, sign in, and enter it (`w3bs login --device` forces this). The CLI keeps its tokens in `~/.config/w3bs/auth.json` (mode 0600). Every signed-in app, the CLI included, is listed on `/account`, where you can sign it out.

**For app developers.** w3bs.org is an OAuth 2.1 authorization server. Register an app on `/account` (a name and its redirect URIs) to get a `client_id`. Apps are public clients: no client secret, so the same flow works for web, mobile and native apps.

- Metadata (RFC 8414): `/.well-known/oauth-authorization-server`
- Authorization code with PKCE, `code_challenge_method=S256` only. `state` is echoed back, and `iss` is added (RFC 9207).
- Redirect URIs match exactly. Use `https`, or `http://127.0.0.1/<path>` / `http://[::1]/<path>` for native apps, which then match any port (RFC 8252).
- `POST /oauth/token` (form or JSON): `authorization_code` with `code`, `redirect_uri`, `client_id`, `code_verifier`; `refresh_token` with `refresh_token`, `client_id`. Access tokens last an hour. Refresh tokens last 30 days and rotate on every use; presenting a spent one revokes the whole sign-in.
- Device authorization grant (RFC 8628) for devices without a browser: `POST /oauth/device_authorization` with `client_id`, show the `user_code` and `verification_uri` (`/device`), then poll `/oauth/token` with `grant_type=urn:ietf:params:oauth:grant-type:device_code`, `device_code`, `client_id` every `interval` seconds.
- `POST /oauth/revoke` (RFC 7009) with `token` and `client_id`.
- `GET /api/me` with `Authorization: Bearer <access token>` returns `sub` (stable, never reuse email as the key), `username`, `email` and `email_verified`.
- Scope: `profile`.

First-party sites that share a secret with w3bs.org can use the silent `prompt=none` bridge from [@profullstack/bridges](https://www.npmjs.com/package/@profullstack/bridges) at `/api/v1/bridge/authorize` and `/api/v1/bridge/token`; clients are configured by the operator in `W3BS_BRIDGE_CLIENTS`.

<span id="conformance"></span>

## Conformance

```sh
bun run test
bun run conformance
bunx playwright install chromium
bun run test:browser
```

The suite starts an isolated registry and checks real CLI and native subprocesses, HTTP, both MCP transports and the web inspector. It includes signature tampering, namespace mismatch, immutable versions, persistent revocation and consent boundaries. Browser tests exercise navigation, prompt search, resource rendering, mobile layouts and offline behavior.

Recorded results are available at `/api/conformance-report` when bundled. Read the report's timestamp and revision metadata; it is build evidence, not a live availability guarantee or a Recommendation badge.

## Deployment

See `docs/deployment.md`. The container runs one replica with persistent SQLite storage. Set the public origin and allowlisted hosts explicitly, configure administrative publishing in a secret manager, attach all four custom domains and verify discovery and the acceptance resource on each. TLS, DNS, an externally accessible source repository and a live smoke check are release gates.

MCP implementation reference: https://ts.sdk.modelcontextprotocol.io/server
