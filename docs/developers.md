# Build once. Meet every surface.

The reference stack implements an experimental W3BS profile with a signed registry, resolver, web inspector, CLI, HTTP API, MCP server and independently implemented native verifier. Node.js 24 or later is required.

## Start locally

From the repository directory:

```sh
npm ci
npm start
```

Open http://localhost:3000 and set `W3BS_API=http://localhost:3000` in your shell. Ten signed resources seed the persistent SQLite registry on first start. The private example signing key is not required to run or deploy it.

The standards, gateway and registry work at `/specs`, `/browse` and `/prompts`. The production subdomains are deployment targets; these instructions do not imply they are live.

## One resource, three interfaces

```sh
W3BS_API=http://localhost:3000 node src/cli.mjs resolve w3bs://prompt/w3bs/research@1
W3BS_API=http://localhost:3000 node src/cli.mjs verify w3bs://prompt/w3bs/research@1
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
W3BS_API=http://localhost:3000 node src/cli.mjs run w3bs://prompt/w3bs/research@1 --inputs inputs.json --consent
```

This runs a deterministic template renderer. The output is instructions, not an AI-generated research answer. It never calls a model or tool. Supply the rendered prompt to a separately authorized model runtime if needed. Manifests requesting tools or capabilities are rejected by this runtime.

## Publish your first artifact

1. Generate an Ed25519 publisher key with `node src/cli.mjs keygen --out .local/my-key --namespace example`.
2. Have the registry operator add the `public.json` entry to its pinned trust store. The namespace must be approved; do not send your private key.
3. Set the same public trust policy for your client through `W3BS_TRUST_FILE`. A registry response alone does not establish client trust.
4. Fork an example with the command below, edit its instructions and metadata, then sign it locally.
5. Have the operator enable publishing with a random `W3BS_PUBLISH_TOKEN` of at least 32 characters. Publish using that token through an authorized CLI or MCP environment, or HTTP Authorization header.

```sh
node src/cli.mjs fork w3bs://prompt/w3bs/research@1 --namespace example --name research --key-id example-1 --out .local/draft.json
node src/cli.mjs sign .local/draft.json --key .local/my-key/private.pem --out .local/signed.json
node src/cli.mjs publish .local/signed.json
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

<span id="conformance"></span>

## Conformance

```sh
npm test
npm run conformance
npx playwright install chromium
npm run test:browser
```

The suite starts an isolated registry and checks real CLI and native subprocesses, HTTP, both MCP transports and the web inspector. It includes signature tampering, namespace mismatch, immutable versions, persistent revocation and consent boundaries. Browser tests exercise navigation, prompt search, resource rendering, mobile layouts and offline behavior.

Recorded results are available at `/api/conformance-report` when bundled. Read the report's timestamp and revision metadata; it is build evidence, not a live availability guarantee or a Recommendation badge.

## Deployment

See `docs/deployment.md`. The container runs one replica with persistent SQLite storage. Set the public origin and allowlisted hosts explicitly, configure administrative publishing in a secret manager, attach all four custom domains and verify discovery and the acceptance resource on each. TLS, DNS, an externally accessible source repository and a live smoke check are release gates.

MCP implementation reference: https://ts.sdk.modelcontextprotocol.io/server
