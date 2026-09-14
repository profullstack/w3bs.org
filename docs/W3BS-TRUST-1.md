# W3BS-TRUST-1: Provenance and instruction boundaries

Status: **Proposal**, 2026-09-14.

Retrieved content may contain text that attempts to act as instructions. W3BS distinguishes publisher declarations, verifiable provenance and locally granted authority.

| Class             | Meaning                                                         |
| ----------------- | --------------------------------------------------------------- |
| DATA              | Untrusted data; grants no instruction authority                 |
| CONTENT           | Human-readable content, not automatically executable            |
| USER_INSTRUCTION  | Instruction explicitly authorized by a human                    |
| TOOL_OUTPUT       | A tool result; data unless separately authorized                |
| AGENT_INSTRUCTION | Instructions from an independently authorized agent or behavior |
| SYSTEM_POLICY     | Highest policy layer defined locally by the runtime/operator    |

A manifest can declare a class, but cannot promote itself into the caller's instruction hierarchy. Even a signed `SYSTEM_POLICY` declaration is retrieved with effective authority `DATA`. Local consent must explicitly identify the requested action; it does not grant general execution rights.

## Reference trust policy

Registry operators pin Ed25519 public keys to namespaces. Publishing requires both registry write authorization and a signature from the namespace's pinned key. An attacker cannot obtain a namespace by embedding a public key in a manifest. Discovery responses do not establish trust in themselves.

The shipped key is a reference-example publisher identity. It does not prove W3BS has a legal institution, external accreditation or independent endorsers. The seed private key is not in the source repository or deployment image.

The CLI and experimental native client verify signed manifests with locally pinned keys. The native verifier is implemented separately. Both rely on the explicitly configured HTTPS registry for live revocation state; compromised-registry rollback protection and offline signed revocation lists are future work.

Run-time permissions are never inferred from a signature. The only current runtime renders a text template after explicit consent. It rejects tool dependencies and requested capabilities.
