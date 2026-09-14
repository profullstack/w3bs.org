# W3BS-DELEGATION-1: Permissions and delegated authority

Status: **Proposal**, 2026-09-14. Delegated execution is not implemented in the reference release.

A future delegation profile must bind an issuer, recipient, target resource, action, scope, expiration, consent evidence and revocation mechanism. Grants must narrow authority instead of amplifying it. A device or agent must be able to decline a grant incompatible with local policy.

Open design questions include grant representation, OAuth interoperability, issuer discovery, attenuation, offline verification, replay protection and human approval UX. Existing identity and authorization standards should supply the building blocks wherever possible.

## Implemented boundary

The reference registry uses an operator-managed bearer token for administrative publish and revoke operations. This token is not a portable delegation credential and has registry-wide write authority. Signed namespace-bound artifacts are still required for publication. Individual publisher accounts and scoped tokens are future work.

The `run` operation accepts explicit consent only for deterministic template rendering. It fails if the artifact requests permissions, tools, MCP servers or executable dependencies. It does not act on devices, make payments or delegate tasks.

Revocation in this release withdraws a fully versioned resource while retaining the immutable artifact for inspection. It is not yet credential or delegation revocation. Revoked content cannot run; aliases keep a revoked newest version visible instead of downgrading silently.
