# W3BS reference release review

Prepared 2026-09-14 from the supplied founding PRD v2.0.

## Reviewable result

The local repository is `/home/anthony/src/profullstack/w3bs.org`. A Docker preview is bound to the local machine at http://localhost:3493. It uses a temporary container database and has administrative publishing disabled. Restarting this temporary preview may reset its data.

The reviewed implementation includes the mission site, specification library, prompt registry, browser inspector, ten signed resources, CLI, HTTP API, both MCP transports and an experimental native URI client. Shared resolution preserves identity, exact version, permissions, provenance, content and verification state. The only execution runtime renders templates with explicit consent; it does not invoke models or tools.

## Evidence

- All 14 protocol, negative trust-boundary, CLI and integration tests pass; CLI publication is exercised using a fresh temporary publisher key.
- All 8 desktop and mobile Chromium checks pass, covering navigation, search, resolution, template rendering and an offline state that never asserts fresh verification.
- `fixtures/conformance-report.json` records the acceptance resource, digest, tested surfaces, timestamp and source fingerprint.
- The Docker build runs as an unprivileged user and excludes local signing keys and Git metadata.
- The root-owned-volume startup path was also tested: initialization succeeds, the HTTP process drops to uid/gid 1000, and the Railway health-check hostname receives a successful response.
- The container returns the correct root surface and discovery document for each of the four configured W3BS Host headers. These local routing checks are not production DNS/TLS checks.
- The production dependency audit reported no known vulnerabilities at the review time.

Screenshots are in the ignored `artifacts` directory. Browser dependencies already available on this workstation were supplied using `LD_LIBRARY_PATH=/home/anthony/.local/share/chrome-deps/usr/lib/x86_64-linux-gnu`; CI installs Chromium dependencies normally.

## Proposed public release

Publish this source as a public `profullstack/w3bs.org` repository with issues enabled. Deploy one Railway service using the Dockerfile, a persistent `/data` volume and the documented environment settings. Connect `w3bs.org`, `specs.w3bs.org`, `browse.w3bs.org` and `prompt.w3bs.org`, replacing their parking records with the provider's exact DNS targets. Run the live acceptance checks in `docs/deployment.md` before announcing availability.

Public repository creation, a new hosted service and production DNS changes have not been performed in this preparation. The PRD establishes the implementation requirements; the live release remains the next action for explicit approval.

## Remaining product scope

The initial reference profile is intentionally labeled experimental. Full delegated execution, decentralized discovery, native mobile/desktop operation parity, a Chromium engine implementation, outside implementations and ratified standards governance remain tracked in `docs/roadmap.md`.
