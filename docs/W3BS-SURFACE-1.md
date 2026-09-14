# W3BS-SURFACE-1: Semantic parity

Status: **Proposal**, 2026-09-14. The reference subset is executable; broader surface obligations remain under discussion.

Every surface must preserve canonical identity, version, permissions, provenance, artifact content and verification state. UI conveniences must not alter resource semantics.

The implemented shared operations are `search`, `resolve`, `inspect`, `verify`, `publish`, `run`, `revoke` and `conformance`. HTTP and MCP expose all eight. The CLI exposes all eight and local key management, signing, forking, version-draft creation and saving a verified manifest. The web UI exposes search, resolution, inspection and consented template rendering, with verification and manifest endpoints linked from the inspector. The experimental native client resolves and verifies; it does not execute instructions.

The reference stack does **not yet claim full operation parity on every UI surface**. Remote signing is deliberately absent: private keys remain local. A future signing profile should define a portable signing request rather than uploading a private key to a registry. Mobile-native and desktop editing, delegated execution, persistent installations and complete publish workflows are future work.

## Acceptance fixture

`w3bs://prompt/w3bs/research@1` must produce byte-equivalent canonical JSON manifests and equivalent verification objects through the API, CLI subprocess, MCP transport, web inspector and independent native client. The test suite also compares exact version and digest. The browser suite loads the actual page and tests its forms.

Run `npm run conformance` for a machine-readable report. The report records tested surfaces and date. Passing this reference suite is evidence for one implementation; it does not establish independent organizational interoperability or Recommendation status.
