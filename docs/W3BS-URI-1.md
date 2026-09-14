# W3BS-URI-1: Canonical resource identifiers

Status: **Community Draft**, draft 1, 2026-09-14. This is an experimental, unregistered URI scheme, not a W3BS Recommendation. The founding PRD is the scope authority; this draft defines the implemented subset.

## Identity and syntax

The URI identifies a logical resource independently of its registry, retrieval location or user interface. A canonical artifact identifier MUST have this form:

```text
w3bs://type/namespace/name@major.minor.patch
w3bs://prompt/w3bs/research@1.0.0
```

The authority component is the resource type, **not a DNS hostname**. `namespace` is a publisher namespace, not a transport address. A registry binds a namespace to operator-approved publisher keys. This draft does not define global namespace allocation or claim that the reference registry owns the namespace system.

Types are `prompt`, `behavior`, `agent`, `tool`, `mcp`, `schema`, `eval`, `identity`, `service`, `device`, `stream` and `swarm`. Type, namespace and name MUST use lowercase ASCII. Namespace and name consist of alphanumeric runs separated by single hyphens. URI length MUST NOT exceed 512 characters.

Versions contain three nonnegative decimal integers, each at most nine digits, without leading zeroes except zero itself. Prerelease versions, build metadata, percent escapes, user information, ports, queries, fragments and additional path segments are outside this draft and MUST be rejected rather than normalized.

## Version selectors

Consumers MAY request an exact version, a major selector or an unversioned name:

| Request                             | Resolution                           |
| ----------------------------------- | ------------------------------------ |
| `w3bs://prompt/w3bs/research@1.0.0` | Only 1.0.0                           |
| `w3bs://prompt/w3bs/research@1`     | Highest numeric version with major 1 |
| `w3bs://prompt/w3bs/research`       | Highest numeric version available    |

An alias is a mutable query, not a reproducible artifact identity. The resolver MUST return the exact canonical identifier and full manifest version. Signed manifests, dependencies and derivation references MUST pin full versions. Clients SHOULD save exact identifiers and digests when reproducing a run.

A revoked newest version MUST remain visible. The resolver MUST NOT silently downgrade a request to older executable content. An absent match returns `NOT_FOUND`. Reusing an existing full version for different content returns `IMMUTABLE_VERSION`.

## Conformance and open questions

The executable grammar is in `src/core.mjs`; the independent experimental client has a separate parser. Positive and negative vectors are in `test/core.test.mjs`.

Global namespace ownership, internationalization, scheme registration, prerelease selection, collision resolution across registries and decentralized naming remain open proposals. Implementers MUST NOT infer DNS ownership from a namespace string.

Foundational reference: https://www.rfc-editor.org/rfc/rfc3986
