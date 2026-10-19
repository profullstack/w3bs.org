# W3BS-PROMPT-1: Portable instruction artifacts

Status: **Proposal**, 2026-09-14.

Prompts are versioned instruction artifacts with identity, attribution, license, provenance, input/output declarations, runtime constraints, tools, permissions, evaluations and integrity metadata. The registry is replaceable; the signed artifact defines the resource.

## Reference template runtime

`w3bs-template-v1` is a deterministic, provider-neutral rendering runtime. It requires explicit consent and a valid, non-revoked artifact. It accepts string inputs matching the declared names and replaces `{{input_name}}` tokens once. Missing required inputs, undeclared inputs and non-string inputs fail. Text inserted as an input is never recursively evaluated.

The result is rendered instructions, **not a model-generated answer**. The result reports `modelInvoked: false`, `toolsInvoked: []`, `instructionAuthority: USER_INSTRUCTION` and `inputAuthority: DATA`. The latter fields describe this explicit rendering request, not authority for unrelated actions in a caller's runtime.

The reference runtime rejects manifests requesting permissions, executable dependencies, tools or MCP servers. It neither grants scopes nor silently drops requested capabilities. Integrators can pass the rendered artifact to a separately authorized model runtime under local policy.

## Runners

A runner is one such separately authorized runtime, chosen per call by the person running it. The reference CLI's `w3bs resource run --via moshcode|infernet|c0mpute` renders through the registry as above, then pipes the rendered text to that peer CLI's stdin and returns its stdout as the answer. Rendering stays model-free: the registry result is unchanged, and the CLI adds `modelInvoked: true` and the runner command it ran. `--consent` authorizes both steps; without it nothing renders, so nothing reaches a runner. A runner inherits no permissions from the artifact, which requests none.

## Behavior packages

A behavior combines instructions, tools, MCP dependencies, schemas, model constraints, evaluations and provenance. Full behavior execution, model adapters, sandboxing, delegated tool execution and evaluation result formats require additional profiles. Do not interpret an accepted `behavior` identifier as support for arbitrary code execution.

Ten signed, MIT-licensed example prompts ship in `fixtures/resources`, and 261 of Fabric's MIT-licensed patterns, converted and signed under the `fabric` namespace, in `fixtures/resources/fabric`. Their evaluation declarations are descriptions of intended checks, not model quality claims.
