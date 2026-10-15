# W3BS Training capability matrix

Last updated 2026-10-15. "Shipped" means merged with tests in this repository.
Nothing here is certified.

## Recorder (Agentic Desktop Layer, `w3bs desktop training`)

| Capability                      | KDE Wayland                                                                                                                                     | GNOME Wayland                                                         | Windows                                           | macOS                                             | X11 / other |
| ------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------- | ------------------------------------------------- | ------------------------------------------------- | ----------- |
| `visual_capture`                | **rig-verified**: focused-window frames through the Screenshot portal, cropped to the window                                                    | implemented (same code, GNOME extension for window data), not rig-run | implemented via the native adapter, not exercised | implemented via the native adapter, not exercised | not started |
| `semantic_state`                | window level only: app id, title, focus (KWin); no AT-SPI                                                                                       | window level (extension); no AT-SPI                                   | window level; UIA not used by the recorder        | window level; AX not used by the recorder         | not started |
| `passive_pointer_events`        | **not available**: InputCapture redirects input; the recorder records none                                                                      | not available                                                         | not attempted                                     | not attempted                                     | not started |
| `shortcut_events`               | not available                                                                                                                                   | not available                                                         | not attempted                                     | not attempted                                     | not started |
| `semantic_text_edits`           | not available (no text is ever captured)                                                                                                        | not available                                                         | not attempted                                     | not attempted                                     | not started |
| `human_agent_attribution`       | lease-based only: agent input is refused during a recording (rig-verified); steps stay `unknown` (or `mixed` when agents were allowed)          | same, not rig-run                                                     | same                                              | same                                              | not started |
| `protected_surface_detection`   | approximate: an unapproved window that is focused or overlaps (3x3 probe) becomes a gap (rig-verified); notifications and OSDs are not detected | same, not rig-run                                                     | same                                              | same                                              | not started |
| `trustworthy_pre_action_frames` | no: frames are polled, never buffered before an input event                                                                                     | no                                                                    | no                                                | no                                                | no          |
| `scoped_surface_capture`        | approximate: crop to the approved window plus the overlap probe                                                                                 | same                                                                  | same                                              | same                                              | not started |

Profile delivered today on every platform: **`visual-review`**. Nothing the recorder produces
qualifies for action supervision (`next-action` exports exclude it).

The KDE rig (`desktop/tests/rig/kde-portal.sh` with `TRAY=1`) checks the whole loop on Plasma 6
under KWin: tray "Start training…" opens the control app at `#training`, Teach starts there, the
tray shows Pause/Resume/Finish, agent input is refused while teaching, an unapproved app in focus
is not captured, Finish → Complete hands a bundle over whose frames are PNG crops of the approved
window with no trace of the unapproved app, and the host's W3BS Training importer accepts it.

## Data side (`w3bs training`)

| Capability                                                                                        | Status                                                              |
| ------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------- |
| Format 0.1 schema, JSON Schema, valid/invalid fixtures                                            | shipped                                                             |
| Native bundle import with quarantine, limits and preview                                          | shipped                                                             |
| JSON/JSONL mapping import                                                                         | shipped                                                             |
| Video import (visual-review)                                                                      | shipped                                                             |
| Privacy scrub (secrets, text values, URL queries, sensitive contexts)                             | shipped                                                             |
| Encrypted artifacts (passphrase, libsecret, macOS keychain)                                       | shipped; Windows key store not yet                                  |
| Review inbox with priorities and daily budget                                                     | shipped                                                             |
| Complete/Partial/Failed/Unknown/Rename/Split/Merge/Discard/Exclude steps                          | shipped                                                             |
| Datasets, immutable versions, splits, digests                                                     | shipped                                                             |
| Per-purpose approval, release, revoke                                                             | shipped                                                             |
| Retrieval grants, agent-access switch                                                             | shipped                                                             |
| Retrieval over MCP (stdio)                                                                        | shipped                                                             |
| Run reporting and with/without comparison                                                         | shipped (aggregates reported runs; does not run agents)             |
| Exports: bundle, canonical JSONL, next-action                                                     | shipped                                                             |
| Deletion by episode, by capture-time window, dataset; lineage report; tombstones; retention       | shipped                                                             |
| Local web app (loopback, one-time link)                                                           | shipped                                                             |
| Terminal review on hqtui                                                                          | shipped                                                             |
| Workflows derived from episodes                                                                   | shipped (always `untested`)                                         |
| Model training adapters                                                                           | none; `jobs submit` returns `TRAIN_ADAPTER_UNAVAILABLE`             |
| Optional model labelling (Anthropic SDK adapter)                                                  | not started; cloud budget is $0                                     |
| Embeddings                                                                                        | not started                                                         |
| Hosted app at w3bs.org/training, sync, team review                                                | not started (M2)                                                    |
| Packaged `w3bs-training` binary (Linux x64/arm64, macOS, Windows) with curl/PowerShell installers | shipped; Linux installer verified in a clean Ubuntu 24.04 container |
| Electron desktop app                                                                              | not started                                                         |
| NicheDB exchange                                                                                  | not started                                                         |

## Consumer clients

| Client      | Status                                                                                                            |
| ----------- | ----------------------------------------------------------------------------------------------------------------- |
| Claude Code | `w3bs training setup claude-code` prints the `claude mcp add` line; stdio MCP verified by the SDK client in tests |
| Codex       | config snippet only; not yet run against a pinned Codex version                                                   |
| OpenCode    | config snippet only; not yet run against a pinned OpenCode version                                                |

MCP access says nothing about whether the model behind a client can be fine-tuned.
