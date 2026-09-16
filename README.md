# W3BS

**The Open Web for Humans, Agents & Devices.**

W3BS is a founding open standards initiative. This repository contains community drafts and an executable reference stack based on the supplied version 2.0 PRD, preserved in `docs/founding-prd.md`.

## Run

Node.js 24+:

```sh
npm ci
npm start
```

Open http://localhost:3000. The mission, specifications, prompt registry and resolver are available at `/mission`, `/specs`, `/prompts` and `/browse`. The same deployment can serve the four W3BS domains. Nothing in this README claims production DNS or deployment is complete.

```sh
W3BS_API=http://localhost:3000 node src/cli.mjs resolve w3bs://prompt/w3bs/research@1
W3BS_API=http://localhost:3000 node native/client.mjs w3bs://prompt/w3bs/research@1 --json
```

## Included

- Three implemented protocol drafts and five initial proposals, governance and contribution process.
- Strict URI and manifest validation, RFC 8785 canonicalization, Ed25519 signatures and namespace-bound pinned public keys.
- Persistent registry with ten signed prompt examples, immutable publication and resource revocation. PostgreSQL in production (`DATABASE_URL`), SQLite for local development and tests.
- Shared search, resolve, inspect, verify, publish, run, revoke and conformance operations over CLI, HTTP and MCP, including stdio and Streamable HTTP.
- Responsive web inspector, prompt search, manifest downloads, explicit template-rendering consent and a PWA offline state.
- An experimental native URI handler with an independent parser and signature verifier.
- Conformance, negative security tests and desktop/mobile browser tests.

`run` renders a prompt template; it does not generate an AI answer or execute tools. Other resource classes, delegated execution, mobile-native apps, decentralized lookup and complete operation parity on every UI surface remain future profiles. The native handler is not a Chromium engine integration. Read `docs/roadmap.md` for the exact scope and remaining PRD work.

## Validate

```sh
npm run check
npm test
npm run conformance
npx playwright install chromium
npm run test:browser
```

Conformance compares the actual API, CLI process, HTML inspection envelope, HTTP MCP, stdio MCP and independent native client. The browser suite checks the page loaded in Chromium, mobile overflow, forms and offline behavior. A passing reference suite is not a W3BS Recommendation or evidence of independent organizational adoption.

## Configuration and operation

See `.env.example`, `docs/developers.md` and `docs/deployment.md`. Production requires a PostgreSQL `DATABASE_URL`, an explicit public origin and host allowlist. Administrative writes are disabled without a publisher token. Example private signing material stays in ignored `.local/keys` and is not required by the server.

The Docker image runs one Node service as a non-root user against the self-hosted PostgreSQL service in the same Railway project. Source, governance proposals, issue templates and conformance fixtures are intended for public release.

## Layout

| Directory  | Contents                                                                                |
| ---------- | --------------------------------------------------------------------------------------- |
| `src`      | Core protocol, registry, operation dispatch, HTTP, CLI, MCP and server-rendered website |
| `native`   | Independent experimental verifier and Linux URI registration                            |
| `public`   | Local CSS, SVG mark, browser behavior and PWA assets                                    |
| `fixtures` | Signed prompts, public trust policy and recorded conformance evidence                   |
| `docs`     | Founding PRD, drafts, governance, developer guide and launch gates                      |
| `test`     | Protocol, integration and browser checks                                                |

## License

MIT for code, schemas, examples and specification drafts. See `LICENSE` and `CONTRIBUTING.md`. A standards patent policy remains to be ratified; no blanket patent clearance is claimed.
