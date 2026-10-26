# W3BS Training

Review, curate, version and consume demonstrations of desktop work, so an
authorized agent can use approved examples on its next task. Specified by the
[PRD](prd.md).

## Quick start: record on your desktop, use it from Claude Code

Two machines: the **desktop** where you do the task (Linux, macOS or Windows
with the Agentic Desktop Layer), and the **agent machine** where Claude Code
runs. They can be the same computer. Recordings never leave the desktop; Claude
reads them over ssh.

### On the desktop

```sh
w3bs update                       # upgrades the desktop layer and W3BS Training (approve the prompt)
```

First time instead:

```sh
curl -fsSL https://w3bs.org/install.sh | sh             # the w3bs CLI
w3bs install agentic-desktop-layer w3bs-training
sudo apt install libsecret-tools                        # Linux only: lets Training use your keyring
```

Then, each time:

1. **Record.** Tray icon → **Start training…** → say what you are about to do,
   tick the apps it may see → **Start**. Do the task. Tray → **Finish task** →
   Complete, Partial, Failed or Not sure.
2. **Review.** `w3bs training open` → **Inbox** → confirm each task.
3. **Share.** **Consumers** → **Share all reviewed tasks** (agent name
   `claude-code`). Or `w3bs training share` in a terminal. Do this again after
   reviewing new tasks; a share lasts 30 days.

### On the agent machine

```sh
w3bs update                       # keeps w3bs current (optional here)

# once: Claude Code runs Training's MCP server on the desktop over ssh (key auth)
claude mcp add --scope user w3bs-training -- \
  ssh -o BatchMode=yes you@desktop '~/.local/bin/w3bs-training' mcp serve --stdio --consumer claude-code

claude mcp list                   # w3bs-training: ✔ Connected
```

Then start a new Claude Code session and ask, for example: _"Use w3bs-training
to find how I did the release checklist, then do it for v2.3."_ Claude gets the
reviewed examples as evidence, not instructions, and no control of the desktop.

Same computer? `claude mcp add --scope user w3bs-training -- w3bs training mcp serve --stdio --consumer claude-code`.

If Claude finds nothing: the desktop must be on with you logged in (the keyring
unlocks with your session), and the tasks must be reviewed **and** shared.
`w3bs training status` on the desktop shows `active_grants`.

### Run a task again with moshcode

Open a reviewed, successful task in the app and choose **Export as moshscript**
(or `w3bs training moshscript EPISODE_ID --out task.mosh` at a terminal). You get
a `.mosh` file for [moshcode](https://moshcoding.com):

```sh
moshcode run task.mosh --dry-run   # what it will do
moshcode run task.mosh             # opens claude with the task; `codex` or another engine as the next argument
```

Nothing is replayed: the script hands the recorded steps (no coordinates, no typed
text) to the engine, which does the task through the Agentic Desktop Layer
(`w3bs-desktop` MCP), looking at the screen before each action. Set that up once
with `w3bs desktop agents add claude-code --preset broad` and
`w3bs desktop setup claude-code --agent claude-code`. The file is an export: it
lives outside W3BS and deleting the task lists it but cannot recall it.

The script carries the task as an [OpenTask](https://logicsrc.com/docs/opentask):
the goal, the apps, the values it asks for by name, and the steps as meaning.

### Publish a task to w3bs.org/tasks

**Publish to w3bs.org/tasks…** in the app (or `w3bs training publish EPISODE_ID`
after `w3bs login`) makes a reviewed, successful task public as an OpenTask, with
a moshscript anyone can run. What goes: the goal, your explanation, app names and
the step list. What stays: screenshots, coordinates, typed text and window titles
(pass `--window-titles` or tick the box to keep them; they can show addresses,
subjects and order numbers). `--workflow "Release a package"` also adds it to that
workflow of yours on [w3bs.org/workflows](https://w3bs.org/workflows), an
[OpenWorkflow](https://logicsrc.com/docs/openworkflow) listing each task by URL and
SHA-256. `w3bs training unpublish TASK_ID` takes it down; deleting the episode does
not. The publish is recorded as an export, so deletion reports name it.

## Status (2026-10-15)

The data side of the M1 local vertical slice is in `training/`: format 0.1,
import, review, datasets, retrieval over MCP, exports, deletion, a local web
app and a terminal review. Recording is the Agentic Desktop Layer's
`w3bs desktop training` (tray: **Start training…**): focused-window frames of
approved applications only, `visual-review` profile, verified on a KDE Plasma
rig; no input events or text are captured (see the capability matrix). The
hosted workspace is at [w3bs.org/training/app](https://w3bs.org/training/app/):
the same app with your W3BS account, `w3bs training push` to send reviewed
tasks there, and MCP over HTTP with per-agent tokens. See the
[capability matrix](capability-matrix.md).

**Teach an agent (2026-10-26):** the local app's **Teach** tab and
`w3bs training teach` record scoped demonstrations, train a CfC liquid policy
with a GRU baseline through Infernet (locally, or on a trusted c0mpute worker
after an upload preview), evaluate it on mock sites and let it suggest or act
with your approval. Everything is off until you turn it on. See
[Teach an agent](liquid-agent.md).

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
| [Teach an agent (Liquid Agent)](liquid-agent.md)                | wireflow, permissions, executor, threat model, how to run  |

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
| `training/liquid/`               | Teach an agent: liquid-agent/v1 contract, masking, executor, mock sites |
| `test/training*.test.mjs`        | core and surface tests                                                  |
