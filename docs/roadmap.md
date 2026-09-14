# W3BS implementation and launch status

This file distinguishes implemented reference behavior from the PRD's full product scope. It is not an announcement that the production domains are live.

| PRD phase         | Reference implementation                                                                   | Remaining launch or ecosystem work                                                               |
| ----------------- | ------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------ |
| 0 — Foundation    | Brand, mission site, founding PRD, governance proposal, repository and issue templates     | Publish source and issue process; deploy domain; ratify governance and IP policy                 |
| 1 — Protocol      | URI, manifest and resolver drafts; signed artifacts; CLI and HTTP API; strict verification | Global namespace governance, richer resource payload profiles, decentralized resolution          |
| 2 — Visible Web   | Browser gateway, searchable prompt registry, ten signed examples, PWA assets               | Public DNS/TLS; self-service publisher accounts and richer registry UX                           |
| 3 — Agent Surface | Streamable HTTP and stdio MCP; discovery on every configured host                          | Portable delegated credentials and agent execution adapters                                      |
| 4 — Native Client | Experimental native URI handler and independent parser/verifier                            | Packaged installers, native UI, TronBrowser engine integration, external implementation evidence |
| 5 — Ecosystem     | Proposal templates and open conformance suite                                              | Outside contributors, working groups, three independent implementations, Candidate Standards     |

## Deliberate draft limitations

- `run` executes the `w3bs-template-v1` renderer. It produces instructions and never claims to have invoked an AI model, tool or device.
- The manifest validator recognizes all twelve resource types but currently specializes content to text instruction artifacts. Other payload and transport profiles need implementation.
- Complete operation parity across web, mobile and desktop UIs remains future work. The six MVP operations are shared by CLI, HTTP and MCP; browser and native inspection use the same resolution contract.
- The reference registry requires operator-pinned publisher keys and an administrative token to publish. It does not implement decentralized namespace ownership or public signup.
- Trust classes never self-promote into instruction authority. Resource revocation is implemented; delegation credentials and offline signed status lists are not.
- A public repository, production deployment, DNS/TLS and live verification are required before calling the public MVP launched. Local passing tests do not establish that launch.
- Multiple independently maintained implementations and ratified governance are required before Recommendation status. This repository does not supply those social outcomes.
