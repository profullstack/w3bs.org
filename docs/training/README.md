# W3BS Training

Review, curate, version and consume demonstrations of desktop work, so an
authorized agent can use approved examples on its next task. Specified by the
[PRD](prd.md).

## Status (2026-10-15)

The data side of the M1 local vertical slice is in `training/`: format 0.1,
import, review, datasets, retrieval over MCP, exports, deletion, a local web
app and a terminal review. **Recording is not built yet**: the recorder is the
Agentic Desktop Layer's `w3bs desktop training`, and its capture feasibility
(M0) comes next. There is no hosted app at w3bs.org/training yet (M2), and no
model-training adapter (M4). See the [capability matrix](capability-matrix.md).

## Documents

| Document                                                        | For                                                        |
| --------------------------------------------------------------- | ---------------------------------------------------------- |
| [Tutorial](tutorial.md)                                         | one task from import to an agent using it, then deletion   |
| [Format 0.1 and importer spec](format-0.1.md)                   | records, bundle layout, import rules, mapping              |
| [Privacy, deletion, cost and recovery](privacy-and-deletion.md) | what is kept, defaults, retention, what deletion cannot do |
| [Capability matrix](capability-matrix.md)                       | what exists, per platform and per surface                  |
| [Threat model](threat-model.md)                                 | actors, controls, tests, known limits                      |
| [ADR 0001](adr-0001-integration.md)                             | layout, versions, routing, sessions, stack deviation       |

## Code

| Path                             | Contents                                                                |
| -------------------------------- | ----------------------------------------------------------------------- |
| `training/schema/`               | zod source of truth, generated JSON Schema                              |
| `training/core/`                 | service (all policy), store, ingest, privacy, zip, crypto, segmentation |
| `training/http.mjs`              | `/api/v1/training` router                                               |
| `training/mcp.mjs`               | MCP server (agent surface)                                              |
| `training/bin/w3bs-training.mjs` | CLI (`w3bs training ...`)                                               |
| `training/ui/`                   | local web app and hqtui terminal review                                 |
| `training/fixtures/`             | synthetic bundles, valid/invalid examples                               |
| `test/training*.test.mjs`        | core and surface tests                                                  |
