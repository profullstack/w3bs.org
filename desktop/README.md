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
| M2: KDE Plasma + GNOME real input | KDE Plasma 6.7.5 (KWin Wayland, Fedora 44): real typing (incl. non-ASCII), pointer, clicks, portal screenshot, disable mid-typing and portal revocation pass on a headless test rig (`tests/rig/kde-portal.sh`, 33/33). Window list, geometry, focus, stacking (occlusion) and activation on Plasma come from KWin scripts (`kwin.rs`), so targeted typing/clicks and restricted per-app grants work there. Interlocks on the rig: a window carrying the polkit-kde agent's app id (the KWin observer's key) refuses input while it is up and the broker stays enabled; locking through KWin's own screen locker (`org.freedesktop.ScreenSaver.Lock`) disables the broker from the locker's `ActiveChanged(true)` signal, with no logind. The real polkit-kde agent cannot take requests in the container (no logind session, and the rig runs as root, whom polkit never asks), and the lock cannot be undone there (no password), so that check runs last. With `TRAY=1` the same rig makes the tray the only native control (kded6 as the StatusNotifierWatcher) and passes 19 more checks (52/52 with the interlock stage): item registration and Disabled/Enabled title, enable from the dbusmenu menu (also while the shortcut dialog is open), an approval shown with Approve/Deny and approved from the menu, left click disables but never enables, Quit and disable, and SIGKILL of the tray disabling the broker; the emergency stop is bound through the GlobalShortcuts portal as Super+Alt+Escape (Plasma's confirmation dialog answered in-session) and fires from a real key chord through KWin. The tray names itself `org.w3bs.Desktop` to the portal and installs a hidden desktop entry for it, since GlobalShortcuts refuses host apps without an app id. The tray on GNOME (AppIndicator extension) is untested. GNOME Shell 50.5 (Mutter Wayland, xdg-desktop-portal-gnome 50.0, Fedora 44): pointer, clicks, typing checked against the live keymap (Mutter silently drops a keysym the active layout has no key for, so text containing one is refused whole with `unsupported`, naming the characters, before anything is typed; on `us` é ü ß are refused and € arrives through AltGr, on `de` ä ö ü ß € arrive exactly and é is refused), portal screenshot (and a fast refusal without a grant), disable mid-typing, restore-token re-enable, top-bar Stop and portal-backend restart pass on a headless rig (`tests/rig/gnome-portal.sh`); the consent dialog is answered in-session by the rig (GNOME has no headless pre-authorization). The W3BS GNOME Shell extension (`gnome-extension/`, installed with `w3bs desktop setup gnome-extension`, GNOME Shell 46-50) adds window data and a top-bar native control; on the same rig (72/72 with it): `--adapter auto` picks portal+gnome, `windows list` matches Mutter's frame rects with exactly one focused window, typing to the focused window works and to an unfocused one is refused `target_stale`, a click on a covered part is refused, the Activities overview blocks input (shell-modal interlock), a restricted agent is confined to its app, window data is refused to any D-Bus caller but the connected broker; the indicator shows state as icon plus text, enables from its menu, shows an approval with Approve/Deny and approves it, Disable and Quit and disable work, and disabling the extension while enabled disables the broker. A RemoteDesktop session stopped through the portal backend (an impl `Session.Close`, which xdg-desktop-portal-gnome never reports to the client) is caught from the extension's list of Mutter remote-access sessions and disables the broker; later input exits 4. Interlocks on the rig: GNOME Shell's own polkit authentication dialog (opened through its polkit agent component, since that agent cannot register with polkitd without logind) refuses input while it is up; switching the shell into its lock-screen session mode (what the lock screen does; extensions are unloaded) disables the broker, and so does `org.gnome.ScreenSaver.ActiveChanged(true)`. Not exercisable in the container: the real lock screen (GNOME Shell 50 builds it only when logind is present) and a polkit request from pkexec. Known GNOME gaps: characters with no key on the active layout cannot be typed (they are refused, never dropped), notification banners are not counted as occluders, and a newly installed extension loads only at the next login. **Not certified** |
| Vela | Observation, focus, occlusion and lock/polkit interlocks (clean-room); input needs `VELA_DEBUG_INPUT=1` + the wlroots virtual-input backend (pending) |
| Windows | Native adapter (`adl-windows`: SendInput, EnumWindows/DWM, WindowFromPoint occlusion, GDI capture, UI Automation, WTS/power/display events, Win+Alt+Esc emergency hotkey), named-pipe transport and a notification-area tray. On GitHub's `windows-latest` runner (Windows Server, 1024x768 at 96 dpi, broker elevated like everything on the runner) `tests/windows/e2e-native.ps1` drives the release binary against two WinForms windows: window list with HWND ids, DWM rects and one focused window; typing incl. `héllo € 😀` lands exactly in the target TextBox; typing into an unfocused window and clicking a covered part of a window are refused `target_stale`; pointer moves land on the exact physical pixel (corners included); a click moves focus; scroll; per-monitor PNG at the monitor's pixel size with real content; UI Automation returns the edit control with its value; disable mid-type stops typing part-way and later input exits 4; the injected Win+Alt+Esc chord disables (audited as emergency stop); the tray adds its icon, opens its menu (status in words, Enable, Stop all, Quit and disable), enables from the menu, left click disables but never enables, Explorer exiting disables and its restart re-adds the icon (`TaskbarCreated`), Quit and disable. The mock-adapter end-to-end and the named-pipe tests (concurrent read/write on one connection, first-instance ownership, parallel clients, peer SID) run there too. Not exercised on the runner: mixed-DPI and multi-monitor layouts with negative origins (code paths only; the runner has one 96 dpi monitor), lock/UAC/secure-desktop interlocks, suspend and logoff events, UIPI refusal of elevated windows (the runner broker is itself elevated), and physical-mouse takeover (all synthetic input carries the injected flag the adapter keys on). **Not certified** |
| macOS, other Linux desktops | Not started |

No platform is certified. There are no signed installers yet.

## Layout

```
crates/adl-schema    versioned schemas: states, errors, coordinate spaces, actions, results
crates/adl-broker    authorization + task broker, input engine, leases, approvals, audit, mock adapter
crates/adl-windows   Windows adapter: SendInput, UI Automation, GDI capture, DWM windows, session events, hotkey
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
  (0600, 0700 dir, same-uid peers only); on Windows over `\\.\pipe\w3bs-desktop-<user>-{agent,control}`
  (owner and only ACE = the current user's SID, first instance owned by the broker, remote clients
  rejected, clients refuse a pipe another user owns). Tokens live under `%LOCALAPPDATA%\w3bs\desktop`,
  whose inherited ACL grants the user, SYSTEM and Administrators. Schemas in `crates/adl-schema`.
- MCP: `w3bs desktop mcp serve --stdio --agent NAME`. No HTTP transport is offered.

## Tests

```bash
cargo test --workspace   # broker conformance, Vela fake socket, KWin and GNOME Shell canned reports, end-to-end daemon/CLI/MCP
desktop/tests/rig/kde-portal.sh     # KDE Plasma in a container (TRAY=1 adds the tray stage)
desktop/tests/rig/gnome-portal.sh   # GNOME Shell in a container, with the W3BS Shell extension
```
