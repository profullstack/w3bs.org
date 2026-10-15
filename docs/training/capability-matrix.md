# W3BS Training capability matrix

Last updated 2026-10-15. "Shipped" means merged with tests in this repository.
Nothing here is certified.

## Recorder (Agentic Desktop Layer, `w3bs desktop training`)

| Capability                      | KDE Wayland                                                | GNOME Wayland | Windows     | macOS       | X11 / other |
| ------------------------------- | ---------------------------------------------------------- | ------------- | ----------- | ----------- | ----------- |
| `visual_capture`                | not started                                                | not started   | not started | not started | not started |
| `semantic_state`                | not started                                                | not started   | not started | not started | not started |
| `passive_pointer_events`        | unproven (InputCapture redirects input; not a passive tap) | unproven      | not started | not started | not started |
| `shortcut_events`               | not started                                                | not started   | not started | not started | not started |
| `semantic_text_edits`           | not started                                                | not started   | not started | not started | not started |
| `human_agent_attribution`       | not started                                                | not started   | not started | not started | not started |
| `protected_surface_detection`   | not started                                                | not started   | not started | not started | not started |
| `trustworthy_pre_action_frames` | not started                                                | not started   | not started | not started | not started |
| `scoped_surface_capture`        | not started                                                | not started   | not started | not started | not started |

The desktop layer already **acts** on KDE and GNOME (screenshots, window lists,
input through the broker). Action support is not evidence that passive capture
works. Recorder feasibility (M0) comes next.

## Data side (`w3bs training`)

| Capability                                                                                  | Status                                                  |
| ------------------------------------------------------------------------------------------- | ------------------------------------------------------- |
| Format 0.1 schema, JSON Schema, valid/invalid fixtures                                      | shipped                                                 |
| Native bundle import with quarantine, limits and preview                                    | shipped                                                 |
| JSON/JSONL mapping import                                                                   | shipped                                                 |
| Video import (visual-review)                                                                | shipped                                                 |
| Privacy scrub (secrets, text values, URL queries, sensitive contexts)                       | shipped                                                 |
| Encrypted artifacts (passphrase, libsecret, macOS keychain)                                 | shipped; Windows key store not yet                      |
| Review inbox with priorities and daily budget                                               | shipped                                                 |
| Complete/Partial/Failed/Unknown/Rename/Split/Merge/Discard/Exclude steps                    | shipped                                                 |
| Datasets, immutable versions, splits, digests                                               | shipped                                                 |
| Per-purpose approval, release, revoke                                                       | shipped                                                 |
| Retrieval grants, agent-access switch                                                       | shipped                                                 |
| Retrieval over MCP (stdio)                                                                  | shipped                                                 |
| Run reporting and with/without comparison                                                   | shipped (aggregates reported runs; does not run agents) |
| Exports: bundle, canonical JSONL, next-action                                               | shipped                                                 |
| Deletion by episode, by capture-time window, dataset; lineage report; tombstones; retention | shipped                                                 |
| Local web app (loopback, one-time link)                                                     | shipped                                                 |
| Terminal review on hqtui                                                                    | shipped                                                 |
| Workflows derived from episodes                                                             | shipped (always `untested`)                             |
| Model training adapters                                                                     | none; `jobs submit` returns `TRAIN_ADAPTER_UNAVAILABLE` |
| Optional model labelling (Anthropic SDK adapter)                                            | not started; cloud budget is $0                         |
| Embeddings                                                                                  | not started                                             |
| Hosted app at w3bs.org/training, sync, team review                                          | not started (M2)                                        |
| Electron desktop app                                                                        | not started                                             |
| NicheDB exchange                                                                            | not started                                             |

## Consumer clients

| Client      | Status                                                                                                            |
| ----------- | ----------------------------------------------------------------------------------------------------------------- |
| Claude Code | `w3bs training setup claude-code` prints the `claude mcp add` line; stdio MCP verified by the SDK client in tests |
| Codex       | config snippet only; not yet run against a pinned Codex version                                                   |
| OpenCode    | config snippet only; not yet run against a pinned OpenCode version                                                |

MCP access says nothing about whether the model behind a client can be fine-tuned.
