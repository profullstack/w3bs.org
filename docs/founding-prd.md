# W3BS

**Product Requirements Document**

**The Open Web for Humans, Agents & Devices**

_Open standards for the Agentic Era_

W3BS.org is an open standards initiative for a full-surface Web where humans, AI agents, software, machines, and devices can discover, communicate, delegate, transact, verify, and collaborate through interoperable protocols.

| Document            | Value                                     |
| ------------------- | ----------------------------------------- |
| Version             | 2.0                                       |
| Domain              | w3bs.org                                  |
| Status              | Founding PRD                              |
| Primary positioning | “W3C for the Agentic Era”                 |
| Tagline             | The Open Web for Humans, Agents & Devices |

# 1. Executive Summary

The Web is moving from a primarily human-operated network of pages and applications into a mixed environment where humans, AI agents, software services, machines, and devices all act as first-class participants. W3BS exists to help keep that transition open, interoperable, inspectable, and portable.

W3BS should function as a neutral standards body and incubation home for the Agentic Web. It should not replace W3C, IETF, HTTP, MCP, A2A, OAuth, DIDs, WebRTC, DHT/P2P, blockchains, or existing transport and identity systems. Its role is to define the missing conventions, profiles, manifests, discovery mechanisms, trust boundaries, conformance rules, and reference implementations that make those systems work together.

The initial product is not one app. It is a standards stack plus reference surfaces: w3bs:// URIs, manifests, resolution, prompt/behavior artifacts, a CLI, API, MCP server, web/PWA interfaces, browse.w3bs.org, prompt.w3bs.org, and at least one native browser implementation such as TronBrowser.

# 2. Product Thesis

The URI is canonical. The UX surface is optional. A W3BS resource should retain the same identity, semantics, permissions, provenance, and behavior whether accessed from a shell, an API, an MCP-capable agent, a mobile PWA, a desktop app, a conventional browser gateway, or a native W3BS-aware browser.

# 3. Design Principles

- Open by default — specifications, schemas, test suites, registries, and reference code are public.
- Human-friendly — humans can understand, approve, revoke, inspect, and override important actions.
- Agent-native — machine actors can discover and use capabilities without scraping a human UI.
- Device-friendly — constrained devices and heterogeneous transports are valid participants.
- Full-surface — CLI, API, MCP, web/PWA, desktop, mobile, and native clients expose the same core semantics.
- Vendor-neutral — no model, cloud, blockchain, browser, or commercial product receives architectural privilege.
- Protocol-composable — reuse proven standards before inventing replacements.
- Decentralization-friendly — centralized hosting and DHT/P2P distribution can coexist.
- Verifiable — identity, provenance, permissions, signatures, and audit trails are explicit.
- Implementation-tested — mature Recommendations require interoperable implementations and conformance tests.

# 4. First-Class Actors

| Actor            | Description                                                                        | Examples                                       |
| ---------------- | ---------------------------------------------------------------------------------- | ---------------------------------------------- |
| Human            | A person who owns goals, authority, identity, consent, and revocation.             | individual user, operator, creator             |
| Agent            | Autonomous or semi-autonomous software acting under policy or delegation.          | research agent, coding agent, purchasing agent |
| Organization     | A legal or social entity with shared authority and policies.                       | company, DAO, nonprofit, team                  |
| Service          | A network capability exposed programmatically.                                     | search, storage, payments, inference           |
| Machine / Device | Physical or embedded participant that can sense, actuate, compute, or communicate. | robot, sensor, vehicle, phone, appliance       |
| Swarm            | A coordinated set of agents or devices acting as one logical actor.                | agent fleet, distributed compute pool          |

# 5. Canonical W3BS Protocol

W3BS should define w3bs:// as the logical namespace for Agentic Web resources. The URI identifies a resource independently from the storage or transport used to retrieve it.

> w3bs://prompt/open/researcher@2
>
> w3bs://behavior/open/code-review@4
>
> w3bs://agent/acme/support
>
> w3bs://mcp/example/filesystem
>
> w3bs://schema/logicsrc/task@1
>
> w3bs://eval/security/prompt-injection@1
>
> w3bs://device/example/camera-17

A resolver must answer: What is this resource? Who published it? Where is its canonical manifest? Can it be verified? What does it depend on? What permissions does it require? How can it be consumed or executed?

# 6. Core Resource Types

| Type     | Purpose                                                                           |
| -------- | --------------------------------------------------------------------------------- |
| prompt   | Portable, versioned instruction artifact.                                         |
| behavior | Prompt + tools + permissions + schemas + evals as an executable behavior package. |
| agent    | Discoverable autonomous actor and its capabilities.                               |
| tool     | Callable capability or function.                                                  |
| mcp      | MCP server or capability descriptor.                                              |
| schema   | Machine-readable shared data contract.                                            |
| eval     | Conformance, quality, safety, or behavior test.                                   |
| identity | Human, agent, organization, service, or device identity.                          |
| service  | Discoverable network service.                                                     |
| device   | Physical/embedded capability.                                                     |
| stream   | Live or continuous data/media resource.                                           |
| swarm    | Distributed group resource or coordinated actor.                                  |

# 7. Full-Surface Contract

Every first-class W3BS operation should be available through multiple interfaces with semantic parity. Surfaces may add convenience, visualization, or local capabilities, but must not redefine the underlying resource.

| Surface   | Primary use                                                           |
| --------- | --------------------------------------------------------------------- |
| w3bs://   | Canonical identity and protocol namespace.                            |
| CLI       | Developers, automation, CI/CD, local workflows.                       |
| API       | Universal programmatic access over HTTP/JSON.                         |
| MCP       | Direct agent/model access to W3BS operations.                         |
| Web / PWA | Zero-install human UI; mobile-first and installable.                  |
| Desktop   | Local files, identity, tools, permissions, richer workflows.          |
| Mobile    | Approvals, signing, delegation, payments, alerts, quick actions.      |
| Browser   | Human browsing plus agent-readable discovery and resource inspection. |

Initial shared operations should include: search, resolve, inspect, verify, publish, version, fork, install, run, sign, revoke, and fetch conformance results.

# 8. Reference Sites and Clients

| Surface         | Role                                                                                                          |
| --------------- | ------------------------------------------------------------------------------------------------------------- |
| w3bs.org        | Standards body, governance, working groups, ecosystem map, and public mission.                                |
| specs.w3bs.org  | Normative specifications, drafts, Recommendations, test suites, and conformance docs.                         |
| prompt.w3bs.org | Reference registry for open prompts and agent behaviors.                                                      |
| browse.w3bs.org | HTTPS gateway/resolver/browser for viewing w3bs:// resources from ordinary browsers.                          |
| W3BS CLI        | Reference command-line client.                                                                                |
| W3BS API        | Reference HTTP/JSON service.                                                                                  |
| W3BS MCP        | Reference MCP server exposing W3BS operations to agents.                                                      |
| TronBrowser     | Candidate first native browser/client with direct w3bs:// support; not a required dependency of the standard. |

# 9. prompt.w3bs.org

Prompts should become portable, versioned, testable, attributable artifacts rather than strings trapped inside applications. prompt.w3bs.org is the reference registry and human interface for that standard.

## 9.1 W3BS Prompt Manifest

- stable ID and semantic version
- publisher identity and signatures
- license and provenance
- prompt or instruction content
- inputs and outputs
- compatible models/runtimes
- required tools and MCP servers
- permissions and delegated authority
- evaluation suites and results
- integrity/security metadata
- dependencies and related behaviors
- canonical and mirror locations

The registry is a reference implementation, not the standard. Any organization should be able to host a compatible W3BS Prompt registry.

## 9.2 Behavior Packages

A behavior extends a prompt into a reusable executable package: instructions + tools + MCP dependencies + schemas + permissions + model constraints + evaluations + provenance. This gives the Agentic Web something analogous to a package ecosystem for reusable machine behavior.

# 10. Trust and Instruction Boundaries

Agentic browsing creates a new class of Web security problem: retrieved content can contain text that attempts to act as instructions. W3BS should standardize explicit instruction-authority and trust metadata so clients can distinguish data from authorized instructions.

| Class             | Meaning                                                     |
| ----------------- | ----------------------------------------------------------- |
| DATA              | Untrusted data; never grants instruction authority.         |
| CONTENT           | Human-readable content; not automatically executable.       |
| USER_INSTRUCTION  | Instruction explicitly authorized by a human user.          |
| TOOL_OUTPUT       | Result from a tool; data unless separately authorized.      |
| AGENT_INSTRUCTION | Instruction from an authorized agent or behavior package.   |
| SYSTEM_POLICY     | Highest local policy layer defined by the runtime/operator. |

# 11. Discovery and Resolution

W3BS discovery should work on today’s Web and in decentralized environments. A domain-level bootstrap document should be available at /.well-known/w3bs.json, with optional discovery through DNS, DHT/P2P, registries, and signed catalogs.

browse.w3bs.org should be the public reference resolver. It allows any current browser to inspect a w3bs:// resource without native protocol support. Native clients such as TronBrowser may register w3bs:// directly.

# 12. Relationship to LogicSRC and Existing Ecosystem

W3BS is the neutral standards umbrella. LogicSRC and related Profullstack/Agentic Era projects can contribute schemas, protocols, reference implementations, and lessons learned, but W3BS must remain capable of accepting competing and complementary approaches from outside organizations.

| Project / Standard              | Relationship to W3BS                                                                                                   |
| ------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| LogicSRC                        | Potential contributor of coordination schemas and conventions for agents, tasks, runs, events, permissions, and audit. |
| OpenSwarm                       | Potential contributor/reference for decentralized files, streams, media, catalogs, and P2P distribution.               |
| MCP                             | Existing tool/context protocol to support and profile rather than replace.                                             |
| A2A and agent protocols         | Interoperate through discovery, manifests, identity, trust, and negotiation profiles.                                  |
| W3C / IETF standards            | Foundational Web and Internet standards to reuse wherever possible.                                                    |
| OAuth / DID / passkeys / PKI    | Identity and authorization building blocks; W3BS defines interoperable profiles where useful.                          |
| HTTP / WebSocket / WebRTC / DHT | Transports; W3BS resources should not depend on only one.                                                              |

# 13. Initial Standards Set

| Spec              | Scope                                                                               |
| ----------------- | ----------------------------------------------------------------------------------- |
| W3BS-URI-1        | Canonical w3bs:// syntax, identifiers, versions, and resource classes.              |
| W3BS-MANIFEST-1   | Common metadata envelope for all W3BS resources.                                    |
| W3BS-RESOLVE-1    | Resolution, discovery, gateway fallback, caching, mirrors, and verification.        |
| W3BS-SURFACE-1    | Semantic parity across CLI, API, MCP, web/PWA, desktop, mobile, and native clients. |
| W3BS-PROMPT-1     | Portable prompt and behavior artifacts.                                             |
| W3BS-TRUST-1      | Provenance, signatures, trust declarations, and instruction-authority boundaries.   |
| W3BS-DELEGATION-1 | Permissions, delegated authority, scopes, expiration, consent, and revocation.      |
| W3BS-DISCOVERY-1  | /.well-known/w3bs.json plus compatible decentralized discovery mechanisms.          |

# 14. MVP

The first public W3BS release must demonstrate an end-to-end standard rather than launch as documentation only.

- Draft W3BS-URI-1, W3BS-MANIFEST-1, and W3BS-RESOLVE-1.
- Publish w3bs.org and specs.w3bs.org with an open proposal and issue process.
- Launch browse.w3bs.org as the reference resolver/browser.
- Launch prompt.w3bs.org with at least 10 useful, versioned, signed example resources.
- Ship a W3BS CLI with search, resolve, inspect, verify, publish, and run.
- Ship equivalent W3BS API endpoints.
- Ship a W3BS MCP server exposing the same operations to agents.
- Publish /.well-known/w3bs.json on W3BS properties.
- Add experimental native w3bs:// handling to TronBrowser or another independent client.
- Provide a conformance test suite proving the same resource resolves consistently across surfaces.

# 15. MVP Acceptance Test

A single resource such as w3bs://prompt/w3bs/research@1 must resolve to the same canonical manifest and verification state from:

- browse.w3bs.org in a normal browser
- the W3BS CLI
- the W3BS API
- the W3BS MCP server
- a native W3BS-aware client such as TronBrowser

If those surfaces disagree about identity, version, permissions, provenance, or the canonical artifact, the implementation is not conformant.

# 16. Governance Model

W3BS can be founded by the Agentic Era / Profullstack ecosystem but should be designed to mature into a vendor-neutral public institution. Founder stewardship is compatible with credibility; permanent unilateral control is not.

- public proposals and issue discussion
- working groups around concrete specifications
- published meeting notes and decisions
- clear intellectual-property contribution terms
- multiple independent implementations before Recommendation status
- transparent conflict-of-interest policies
- open conformance suites and reference code
- a path for outside organizations and individual implementers to gain governance roles

# 17. Standards Lifecycle

| Stage                 | Meaning                                                                     |
| --------------------- | --------------------------------------------------------------------------- |
| Proposal              | Problem statement and initial technical direction.                          |
| Community Draft       | Open draft with active implementation and feedback.                         |
| Candidate Standard    | Feature-complete specification with test suite and implementation evidence. |
| W3BS Recommendation   | Stable interoperable standard with multiple conforming implementations.     |
| Revision / Superseded | Versioned evolution without breaking historical identifiers.                |

# 18. Non-Goals

- Creating a proprietary agent marketplace.
- Replacing the W3C, IETF, HTTP, MCP, browsers, or existing identity/payment systems.
- Requiring a specific blockchain, model provider, cloud, wallet, or database.
- Operating a closed prompt marketplace whose database is the de facto standard.
- Making TronBrowser mandatory for W3BS adoption.
- Defining one monolithic “agent framework” that every implementation must use.

# 19. Success Metrics

| Category             | Initial measure                                                                                     |
| -------------------- | --------------------------------------------------------------------------------------------------- |
| Adoption             | At least 3 independent W3BS implementations beyond the reference stack.                             |
| Interoperability     | A conformance fixture resolves identically across CLI, API, MCP, web gateway, and native client.    |
| Ecosystem            | Third parties publish prompts, agents, tools, schemas, or evals using W3BS manifests.               |
| Portability          | Resources can move between registries without changing canonical identity.                          |
| Governance           | Outside contributors participate in draft and Recommendation decisions.                             |
| Trust                | Signed provenance and permission metadata are visible and verifiable across all reference surfaces. |
| Developer experience | A new developer can publish and resolve a valid resource in under 10 minutes.                       |

# 20. Launch Sequence

| Phase             | Deliverable                                                                          |
| ----------------- | ------------------------------------------------------------------------------------ |
| 0 — Foundation    | Brand, mission, repository, governance draft, w3bs.org.                              |
| 1 — Protocol      | URI + manifest + resolver drafts; CLI/API prototypes.                                |
| 2 — Visible Web   | browse.w3bs.org and prompt.w3bs.org.                                                 |
| 3 — Agent Surface | W3BS MCP server and agent-native discovery.                                          |
| 4 — Native Client | Experimental w3bs:// implementation in TronBrowser or another client.                |
| 5 — Ecosystem     | Third-party implementations, working groups, conformance badge, Candidate Standards. |

**W3BS.org**

**The Open Web for Humans, Agents & Devices**

_Human-friendly. Agent-native. Device-friendly. Open by design._
