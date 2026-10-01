# Contributing to W3BS

Bring a concrete interoperability problem, an implementation report or a reproducible defect. Founders' implementations do not receive architectural preference.

For a standards proposal, state the problem, affected actors and surfaces, existing standards considered, proposed wire contract, trust boundaries, migration implications and a testable acceptance example. Use the proposal issue template. Identify whether you are proposing text, implementing a draft or reporting interoperability.

For code, run `bun run check`, `bun run test` and the relevant browser checks before submitting. Describe the observable behavior and validation. Preserve signed fixture versions; new content requires a new version. Never commit private keys, tokens or `.local`.

Submit commits with a Developer Certificate of Origin sign-off (`git commit -s`) only when you can attest to https://developercertificate.org/. Contributions use this repository's MIT license. This is not an adopted standards patent policy; that policy requires open review before Recommendation status.

Keep discussion specific and respectful. Disclose relevant employment, funding and conflicts when proposing governance or standards decisions. Maintainers should record material decisions and unresolved objections in `docs/decisions`.

The public issue process begins when this repository is published. No external review or ratification is implied by a locally prepared proposal.
