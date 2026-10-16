# W3BS Training

Review, curate, version and consume demonstrations of desktop work, so an
authorized agent can use approved examples on its next task. Specified by the
[PRD](prd.md).

## Status (2026-10-15)

The data side of the M1 local vertical slice is in `training/`: format 0.1,
import, review, datasets, retrieval over MCP, exports, deletion, a local web
app and a terminal review. Recording is the Agentic Desktop Layer's
`w3bs desktop training` (tray: **Start training…**): focused-window frames of
approved applications only, `visual-review` profile, verified on a KDE Plasma
rig; no input events or text are captured (see the capability matrix). The
hosted workspace is at [w3bs.org/training/app](https://w3bs.org/training/app/):
the same app with your W3BS account, `w3bs training push` to send reviewed
tasks there, and MCP over HTTP with per-agent tokens. There is no
model-training adapter yet (M4). See the [capability matrix](capability-matrix.md).

## Install

```sh
curl -fsSL https://w3bs.org/training/install.sh | sh        # Linux, macOS
irm https://w3bs.org/training/install.ps1 | iex             # Windows (PowerShell)
w3bs install w3bs-training                                  # with the w3bs CLI
```

On Linux, also install libsecret's `secret-tool` so the encryption key can live
in your desktop keyring (KWallet or GNOME Keyring). Most desktops do not ship it:

```sh
sudo apt install libsecret-tools     # Debian, Ubuntu, Kubuntu
sudo dnf install libsecret           # Fedora
sudo pacman -S libsecret             # Arch
```

One self-contained `w3bs-training` binary (no Bun or Node), checked against the
release's SHA256SUMS. `w3bs training …` runs it through the w3bs CLI, and the
desktop recorder finds it on its own (no restart needed). Recordings are
encrypted with a key from your keyring (libsecret's `secret-tool` on Linux, the
login keychain on macOS, DPAPI on Windows) or `W3BS_TRAINING_PASSPHRASE`; with
none of those it refuses to store anything. For recordings from the desktop
tray, the passphrase has to be in the desktop layer's environment, not just
your shell's, so the keyring is the easier route.

If the control app says a recording is waiting, install what it names and press
**Try again**. Waiting recordings are held in memory only, so do this before you
log out or restart the desktop layer.

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
