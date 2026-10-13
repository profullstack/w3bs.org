# Agentic Desktop Layer

Let authorized agents (Claude Code, OpenCode, Codex) operate your real desktop with mouse and
keyboard, behind a switch you control. **Your desktop. Your agents. Your switch.**

Spec: [`docs/agentic-desktop-layer-prd.md`](../docs/agentic-desktop-layer-prd.md) (PRD v0.2) ·
Page: https://w3bs.org/desktop · License: Apache-2.0

## Status

| Milestone | State |
|---|---|
| M0: PRD, schemas, ADR, public page | Done |
| M1: broker, input engine, mock adapter, native control, enable/disable, leases, cancellation, identity tests | Done (terminal native control `control`; StatusNotifierItem tray `tray` with an emergency-stop shortcut, verified on KDE Plasma; GNOME Shell top-bar indicator from the W3BS Shell extension, verified on GNOME Shell 50.5) |
| M2: KDE Plasma + GNOME real input | KDE Plasma 6.7.5 (KWin Wayland, Fedora 44): real typing (incl. non-ASCII), pointer, clicks, portal screenshot, disable mid-typing and portal revocation pass on a headless test rig (`tests/rig/kde-portal.sh`, 29/29). Window list, geometry, focus, stacking (occlusion) and activation on Plasma come from KWin scripts (`kwin.rs`), so targeted typing/clicks and restricted per-app grants work there; polkit-prompt interlock unit-tested only. With `TRAY=1` the same rig makes the tray the only native control (kded6 as the StatusNotifierWatcher) and passes 19 more checks (48/48): item registration and Disabled/Enabled title, enable from the dbusmenu menu (also while the shortcut dialog is open), an approval shown with Approve/Deny and approved from the menu, left click disables but never enables, Quit and disable, and SIGKILL of the tray disabling the broker; the emergency stop is bound through the GlobalShortcuts portal as Super+Alt+Escape (Plasma's confirmation dialog answered in-session) and fires from a real key chord through KWin. The tray names itself `org.w3bs.Desktop` to the portal and installs a hidden desktop entry for it, since GlobalShortcuts refuses host apps without an app id. The tray on GNOME (AppIndicator extension) is untested. GNOME Shell 50.5 (Mutter Wayland, xdg-desktop-portal-gnome 50.0, Fedora 44): pointer, clicks, ASCII typing, portal screenshot (and a fast refusal without a grant), disable mid-typing, restore-token re-enable, top-bar Stop and portal-backend restart pass on a headless rig (`tests/rig/gnome-portal.sh`); the consent dialog is answered in-session by the rig (GNOME has no headless pre-authorization). The W3BS GNOME Shell extension (`gnome-extension/`, installed with `w3bs desktop setup gnome-extension`, GNOME Shell 46-50) adds window data and a top-bar native control; on the same rig (58/58 with it, all 21 earlier checks included): `--adapter auto` picks portal+gnome, `windows list` matches Mutter's frame rects with exactly one focused window, typing to the focused window works and to an unfocused one is refused `target_stale`, a click on a covered part is refused, the Activities overview blocks input (shell-modal interlock), a restricted agent is confined to its app, window data is refused to any D-Bus caller but the connected broker; the indicator shows state as icon plus text, enables from its menu, shows an approval with Approve/Deny and approves it, Disable and Quit and disable work, and disabling the extension while enabled disables the broker. Lock screen and polkit-dialog interlocks are code paths (`Main.sessionMode.isLocked`, `Main.modalCount`) not exercised by the rig; only the overview case is. Known GNOME gaps: characters outside the active keyboard layout are dropped by Mutter, a backend-side impl `Session.Close` is not reported to the client, notification banners are not counted as occluders, and a newly installed extension loads only at the next login. **Not certified** |
| Vela | Observation, focus, occlusion and lock/polkit interlocks (clean-room); input needs `VELA_DEBUG_INPUT=1` + the wlroots virtual-input backend (pending) |
| Windows, macOS, other Linux desktops | Not started |

No platform is certified. There are no signed installers yet.

## Layout

```
crates/adl-schema    versioned schemas: states, errors, coordinate spaces, actions, results
crates/adl-broker    authorization + task broker, input engine, leases, approvals, audit, mock adapter
crates/adl-linux     Vela command-socket adapter, XDG portal adapter (KDE/GNOME), KWin and GNOME Shell window observers, logind, keymaps
crates/w3bs-desktop  `w3bs desktop` CLI, broker daemon, terminal and tray native controls, stdio MCP server
gnome-extension/     GNOME Shell extension: top-bar native control + window data (org.w3bs.Desktop.Shell)
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
                                         # (or `$B tray`: StatusNotifierItem + Super+Alt+Escape stop)
# in another terminal:
$B mouse click --agent claude-code --window w-editor --x 40 --y 40
$B setup claude-code --agent claude-code # prints `claude mcp add ...`
```

On a real session use `--adapter auto` (Vela if present, else portals, plus the KWin window
observer on Plasma or the W3BS Shell extension on GNOME). It refuses to fall back to the mock.

On GNOME, install the extension once, then log out and back in (GNOME Shell loads new extensions
only at startup); its top-bar indicator is then the native control:

```bash
$B setup gnome-extension   # writes ~/.local/share/gnome-shell/extensions/agentic-desktop@w3bs.org, enables it
$B doctor                  # gnome_shell_extension: ok once it is loaded
```

Without it the portal backend still types and clicks, but window listing is unsupported and
targeted input is refused.

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
cargo test --workspace   # broker conformance, Vela fake socket, KWin and GNOME Shell canned reports, end-to-end daemon/CLI/MCP
desktop/tests/rig/kde-portal.sh     # KDE Plasma in a container (TRAY=1 adds the tray stage)
desktop/tests/rig/gnome-portal.sh   # GNOME Shell in a container, with the W3BS Shell extension
```
