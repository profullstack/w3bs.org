# Agentic Desktop Layer

Let authorized agents (Claude Code, OpenCode, Codex) operate your real desktop with mouse and
keyboard, behind a switch you control. **Your desktop. Your agents. Your switch.**

Spec: [`docs/agentic-desktop-layer-prd.md`](../docs/agentic-desktop-layer-prd.md) (PRD v0.2) ·
Page: https://w3bs.org/desktop · License: Apache-2.0

## Status

| Milestone | State |
|---|---|
| M0: PRD, schemas, ADR, public page | Done |
| M1: broker, input engine, mock adapter, native control, enable/disable, leases, cancellation, identity tests | Done (terminal native control; tray/panel extensions pending) |
| M2: KDE Plasma + GNOME real input | Portal backend written and compiling; **not certified on any desktop** |
| Vela | Observation, focus, occlusion and lock/polkit interlocks (clean-room); input needs `VELA_DEBUG_INPUT=1` + the wlroots virtual-input backend (pending) |
| Windows, macOS, other Linux desktops | Not started |

No platform is certified. There are no signed installers yet.

## Layout

```
crates/adl-schema    versioned schemas: states, errors, coordinate spaces, actions, results
crates/adl-broker    authorization + task broker, input engine, leases, approvals, audit, mock adapter
crates/adl-linux     Vela command-socket adapter, XDG portal adapter (KDE/GNOME), logind, keymaps
crates/w3bs-desktop  `w3bs desktop` CLI, broker daemon, terminal native control, stdio MCP server
packaging/           systemd user unit
docs/                ADRs and the clean-room record
```

## Try it (mock desktop)

```bash
cargo build --release -p w3bs-desktop
B=target/release/w3bs-desktop
$B daemon --adapter mock &               # starts Disabled
$B agents add claude-code --preset broad # saves a 0600 token
$B control                               # native control; type `enable`
# in another terminal:
$B mouse click --agent claude-code --window w-editor --x 40 --y 40
$B setup claude-code --agent claude-code # prints `claude mcp add ...`
```

On a real session use `--adapter auto` (Vela if present, else portals). It refuses to fall back
to the mock.

## Safety model, in one screen

- **Starts Disabled** after install, login, restart or crash. Only a human enables: the code goes
  to the native control, and control requests from an agent's process tree are refused.
- **Disable closes everything** the layer mediates: an atomic gate plus a revocation generation
  reject new work immediately, then leases, tasks, approvals, captures and streams are dropped and
  any synthetic held keys/buttons are released. Lock, suspend, logout, VT switch, portal revocation,
  adapter loss, native-control loss and human takeover all disable. Nothing re-enables on its own.
- **Authorization is checked before every primitive event**, not when work is queued. One agent
  holds the desktop input lease at a time. Batches are bounded (256 actions, 4096 characters, 5 s
  holds) and nothing stays held after its batch.
- **Targets are validated**: every point names its coordinate space; screenshot points die with
  the layout generation; a covered target or a focus change stops the batch (`target_stale`).
- **Approvals bind the exact request** (agent, task, target, argument digest, expiry); agent input
  is suspended while one is pending; the agent surface has no enable/grant/approve methods.
- **Audit is metadata only** (7-day default retention): never screenshots, typed text or bodies.
- **Limits**: broker policy cannot contain a hostile process that already runs as you. Broad desktop
  control cannot enforce business intent (a draft-only mail grant does not stop a browser send).
  Disabling cannot unsend, retract delivered input, or erase what an agent already received.

## Interfaces

- CLI: `w3bs desktop …` (this binary is `w3bs-desktop` until the W3BS package manager dispatches
  `w3bs <sub>` to `w3bs-<sub>`).
- Local API: newline-delimited JSON over `$XDG_RUNTIME_DIR/w3bs-desktop/{agent,control}.sock`
  (0600, 0700 dir, same-uid peers only). Schemas in `crates/adl-schema`.
- MCP: `w3bs desktop mcp serve --stdio --agent NAME`. No HTTP transport is offered.

## Tests

```bash
cargo test --workspace   # 30 broker conformance + Vela fake-socket + end-to-end daemon/CLI/MCP
```
