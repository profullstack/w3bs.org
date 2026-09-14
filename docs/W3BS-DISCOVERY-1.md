# W3BS-DISCOVERY-1: A machine-readable front door

Status: **Proposal**, 2026-09-14. The HTTPS bootstrap subset is implemented.

Each W3BS property serves `/.well-known/w3bs.json`. The document identifies the reference registry, resolution and search endpoints, raw manifests, schemas, MCP endpoint, trust metadata, specification documents and available surfaces. Its version is `W3BS-DISCOVERY-1-draft-1`.

The reference service can serve `w3bs.org`, `specs.w3bs.org`, `browse.w3bs.org` and `prompt.w3bs.org` from one deployment. The root path selects the appropriate surface by host; explicit `/mission`, `/specs`, `/browse` and `/prompts` paths work on every configured host.

Discovery is data. Clients must select or approve their registry and key policy independently. Never derive execution authority from an endpoint response or trust a discovered key solely because it was included in the same response as a signature.

DNS records, signed catalogs, DHT/P2P lookup, decentralized namespace delegation and device-oriented compact documents remain open work. A compliant future profile must preserve canonical identity and define bootstrap trust and freshness, including failure and revocation behavior.

Reference: https://www.rfc-editor.org/rfc/rfc8615
