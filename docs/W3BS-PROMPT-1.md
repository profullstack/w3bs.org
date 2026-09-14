# W3BS-PROMPT-1: Portable instruction artifacts

Status: **Proposal**, 2026-09-14.

Prompts are versioned instruction artifacts with identity, attribution, license, provenance, input/output declarations, runtime constraints, tools, permissions, evaluations and integrity metadata. The registry is replaceable; the signed artifact defines the resource.

## Reference template runtime

`w3bs-template-v1` is a deterministic, provider-neutral rendering runtime. It requires explicit consent and a valid, non-revoked artifact. It accepts string inputs matching the declared names and replaces `{{input_name}}` tokens once. Missing required inputs, undeclared inputs and non-string inputs fail. Text inserted as an input is never recursively evaluated.

The result is rendered instructions, **not a model-generated answer**. The result reports `modelInvoked: false`, `toolsInvoked: []`, `instructionAuthority: USER_INSTRUCTION` and `inputAuthority: DATA`. The latter fields describe this explicit rendering request, not authority for unrelated actions in a caller's runtime.

The reference runtime rejects manifests requesting permissions, executable dependencies, tools or MCP servers. It neither grants scopes nor silently drops requested capabilities. Integrators can pass the rendered artifact to a separately authorized model runtime under local policy.

## Behavior packages

A behavior combines instructions, tools, MCP dependencies, schemas, model constraints, evaluations and provenance. Full behavior execution, model adapters, sandboxing, delegated tool execution and evaluation result formats require additional profiles. Do not interpret an accepted `behavior` identifier as support for arbitrary code execution.

Ten signed, MIT-licensed example prompts ship in `fixtures/resources`. Their evaluation declarations are descriptions of intended checks, not model quality claims.
