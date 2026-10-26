# liquid-agent/v1 contract (vendored)

These files are a verbatim copy of the shared Liquid Agent contract from
Infernet Protocol:

- Source: https://github.com/InfernetProtocol/infernet-protocol (PR #60)
- Path: `packages/liquid-agent/schemas/` and `packages/liquid-agent/fixtures/`
- Commit: `81060e1ee2c5bce7c3f87499ecf3b01aa5e35708`

Do not edit them here. To update, copy both folders again from a newer Infernet
commit, change the commit above, and run `bun test test/training-liquid.test.mjs`:
it checks that the schemas still hash to `fixtures/contract.json` `schemas_hash`,
that every valid fixture validates and every invalid one fails, and that W3BS
re-derives the fixture bundle's `dataset_hash` byte for byte.

The JavaScript that reads them lives one folder up (`training/liquid/*.mjs`,
ported from `packages/liquid-agent/src/` at the same commit).
