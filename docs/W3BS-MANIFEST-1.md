# W3BS-MANIFEST-1: The common resource envelope

Status: **Community Draft**, draft 1, 2026-09-14. Not a Recommendation.

## Required envelope

The machine-readable schema is available at `/schemas/manifest.json`, generated from the same strict schema used by the reference validator. Unknown fields MUST be rejected in this draft. JSON field names are case-sensitive.

| Field                   | Meaning                                                                                      |
| ----------------------- | -------------------------------------------------------------------------------------------- |
| `specVersion`           | Exactly `W3BS-MANIFEST-1-draft-1`                                                            |
| `id`, `type`, `version` | Fully versioned W3BS identity; all three must agree                                          |
| `name`, `description`   | Human-readable resource metadata                                                             |
| `publisher`             | Namespace, display name and pinned signing-key identifier                                    |
| `license`, `createdAt`  | License identifier and ISO 8601 UTC creation time                                            |
| `provenance`            | HTTPS source and pinned `derivedFrom` resource identifiers                                   |
| `trust.classification`  | Publisher-declared content class, never a grant of authority                                 |
| `permissions`           | Requested capabilities; an empty list requests none                                          |
| `dependencies`          | Fully versioned dependencies                                                                 |
| `locations`             | Canonical HTTPS artifact location and optional HTTPS mirrors                                 |
| `content`               | Instruction artifact, typed inputs, output, runtime, tools, MCP dependencies and evaluations |
| `proof`                 | Ed25519 signature, signing key ID and SHA-256 digest                                         |

This first implementation specializes the envelope's `content` to text instruction artifacts and the `w3bs-template-v1` runtime. The other resource classes are recognized identifiers; richer binary, stream, device and service payload schemas require future profiles. Do not claim those runtimes are implemented.

## Signing procedure

1. Validate the manifest, omitting `proof` while signing.
2. Remove the entire top-level `proof` field.
3. Serialize the remaining object using RFC 8785 JSON Canonicalization Scheme, encoded as UTF-8.
4. Compute SHA-256 of those bytes and encode as `sha256:` followed by lowercase hexadecimal.
5. Sign the same bytes using Ed25519 and encode the signature as unpadded base64url.
6. Add `proof` with `type: Ed25519`, `keyId`, `digest` and `signature`.

`publisher.keyId` and `proof.keyId` MUST match. The signed publisher namespace MUST match the namespace in the identifier. The public key MUST be pinned to that namespace by the operator. An embedded or newly discovered key MUST NOT confer trust on itself. Public keys use PEM SPKI encoding; private keys stay with their owner.

All semantic fields, including permissions, provenance, instructions and locations, are covered by the signature. Moving a resource to a new canonical location requires a newly signed version. A mirror can serve the original bytes without changing identity.

## Verification result

The reference result contains `valid`, `signatureValid`, `publisherTrusted`, `revoked`, `digest`, `keyId` and `authority`. `valid` requires a valid signature, a namespace-bound trusted publisher key and no key or resource revocation. `authority` is always `DATA` after retrieval. A valid signature is not a permission grant or a certificate of real-world identity.

Declared evaluation IDs describe intended checks. They are not signed evidence that a model passed an evaluation. Reference fixture checks and a recorded cross-surface test report are separate artifacts.

References: https://www.rfc-editor.org/rfc/rfc8785 and https://www.rfc-editor.org/rfc/rfc8032
