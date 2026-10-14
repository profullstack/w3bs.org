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
| macOS | `--adapter native` (`adl-macos`): Core Graphics events from a private, tagged event source; CGWindowList windows, focus and stacking; Accessibility focus and `inspect`; `CGDisplayCreateImage` captures (`screencapture` fallback); polled lock/console/sleep state; HID-idle takeover detection; menu-bar native control (`tray`) with a Carbon emergency-stop hot key (Control+Option+Command+Escape). On GitHub's macos-latest runner (macOS 26.6.2, arm64, 1024x768, Accessibility + PostEvent + Screen Recording pre-granted to `/bin/bash` by the image, which our broker inherits when started from the job shell) `tests/rig/macos-e2e.sh` passes 22/22: doctor reports the three permissions; windows list matches AppKit frames with exactly one focused window; typing `héllo € 😀` lands exactly; typing into an unfocused window is refused `target_stale`; Backspace and `primary+a` (Command+A) work; pointer move reads back exactly; a click focuses another window; a double click arrives with clickCount 2; capture is the display's pixel size; `inspect` returns the AXTextField with its value; disable mid-typing stops it and later input exits 4; the menu-bar item says `Agents: Disabled`, enables and disables from its menu and its title follows; the hot key (a posted CGEvent chord) disables. Not exercised there: permission-denied paths (the runner grants everything), HiDPI/multi-display/rotated layouts, lock screen, secure-input and authentication-prompt interlocks, sleep detection, physical takeover (no human), permission revocation while enabled. **Not certified** |
| Windows | Native adapter (`adl-windows`: SendInput, EnumWindows/DWM, WindowFromPoint occlusion, GDI capture, UI Automation, WTS/power/display events, Win+Alt+Esc emergency hotkey), named-pipe transport and a notification-area tray. On GitHub's `windows-latest` runner (Windows Server, 1024x768 at 96 dpi, broker elevated like everything on the runner) `tests/windows/e2e-native.ps1` drives the release binary against two WinForms windows: window list with HWND ids, DWM rects and one focused window; typing incl. `héllo € 😀` lands exactly in the target TextBox; typing into an unfocused window and clicking a covered part of a window are refused `target_stale`; pointer moves land on the exact physical pixel (corners included); a click moves focus; scroll; per-monitor PNG at the monitor's pixel size with real content; UI Automation returns the edit control with its value; disable mid-type stops typing part-way and later input exits 4; the injected Win+Alt+Esc chord disables (audited as emergency stop); the tray adds its icon, opens its menu (status in words, Enable, Stop all, Quit and disable), enables from the menu, left click disables but never enables, Explorer exiting disables and its restart re-adds the icon (`TaskbarCreated`), Quit and disable. The mock-adapter end-to-end and the named-pipe tests (concurrent read/write on one connection, first-instance ownership, parallel clients, peer SID) run there too. Not exercised on the runner: mixed-DPI and multi-monitor layouts with negative origins (code paths only; the runner has one 96 dpi monitor), lock/UAC/secure-desktop interlocks, suspend and logoff events, UIPI refusal of elevated windows (the runner broker is itself elevated), and physical-mouse takeover (all synthetic input carries the injected flag the adapter keys on). **Not certified** |
| Other Linux desktops | Not started |

No platform is certified. Release binaries and installers are not code-signed.

## Install

Every installer is per-user (no root or administrator), checks the release archive against the
release's `SHA256SUMS`, starts the broker **Disabled** and never enables access; you enable it
from the native control.

**Linux** (x86_64, aarch64; glibc 2.28+):

```bash
curl --proto '=https' --proto-redir '=https' -fsSL https://w3bs.org/desktop/install.sh | sh
```

Installs `~/.local/bin/w3bs-desktop`, a systemd user service for the broker, and the tray (KDE
and others) or the GNOME Shell extension (GNOME) in your session.

**macOS** 11+ (Apple silicon and Intel), same command. It installs `~/.local/bin/w3bs-desktop`
and two LaunchAgents in your login session, and starts both:

- `~/Library/LaunchAgents/org.w3bs.desktop.broker.plist`: `w3bs-desktop daemon --adapter auto`,
  at login, restarted by launchd if it crashes (not after a clean exit). Log:
  `~/Library/Logs/w3bs-desktop/broker.log`.
- `~/Library/LaunchAgents/org.w3bs.desktop.menubar.plist`: the menu-bar control
  (`w3bs desktop tray`), at login only; Quit keeps it quit (and disables access).

Then grant the permissions below to `~/.local/bin/w3bs-desktop` (see
[macOS onboarding](#macos-onboarding)). Uninstall with
`curl -fsSL https://w3bs.org/desktop/install.sh | sh -s -- --uninstall` (keeps
`~/.config/w3bs/desktop`, your agents and settings).

**Windows** 10/11 (x86_64; ARM64 under emulation), in PowerShell 5.1 or 7:

```powershell
irm https://w3bs.org/desktop/install.ps1 | iex
```

Installs to `%LOCALAPPDATA%\Programs\w3bs-desktop` (`bin\w3bs-desktop.exe`, the console CLI, and
`bin\w3bs-desktopw.exe`, the same program built for the GUI subsystem), adds `bin` to your user
PATH, and registers the broker and the notification-area tray to start at sign-in (HKCU `Run`
entries `W3BS Desktop Broker` and `W3BS Desktop Tray`, both running `w3bs-desktopw.exe`, so no
console window opens), then starts both. Their output goes to
`%LOCALAPPDATA%\w3bs\desktop\state\logs\{daemon,tray}.log`. Emergency stop: Win+Alt+Esc.
Options go through a script block:

```powershell
& ([scriptblock]::Create((irm https://w3bs.org/desktop/install.ps1))) -Version 0.2.0 -NoService
& ([scriptblock]::Create((irm https://w3bs.org/desktop/install.ps1))) -Uninstall   # add -Purge to delete agents and settings
```

Then, on any system:

```bash
w3bs desktop agents add claude-code --preset broad
w3bs desktop setup claude-code --agent claude-code   # prints the `claude mcp add` line
```

OpenCode: register an agent for it and merge the printed `mcp` block into
`~/.config/opencode/opencode.json` (or a project `opencode.json`). Add `--http` to point it at the
broker's MCP endpoint instead of spawning a stdio server; the bearer token is read from the token
file with OpenCode's `{file:...}` substitution, so it never lands in the config.

```bash
w3bs desktop agents add opencode --preset broad
w3bs desktop setup opencode --agent opencode          # {"mcp": {"w3bs-desktop": {"type": "local", ...}}}
w3bs desktop setup opencode --agent opencode --http   # {"mcp": {"w3bs-desktop": {"type": "remote", ...}}}
```

Tools appear in OpenCode as `w3bs-desktop_desktop_*`. Screenshots come back as image content, so
the model must take image input: for a custom provider, declare it on the model or OpenCode drops
the image and tells the model it cannot see it:

```json
"models": { "<model>": { "tool_call": true, "modalities": { "input": ["text", "image"], "output": ["text"] } } }
```

Testing hooks (all installers): `W3BS_DESKTOP_VERSION`, `W3BS_DESKTOP_NO_SERVICE=1`, and
`W3BS_DESKTOP_ARCHIVE` (a local archive) with `W3BS_DESKTOP_SHA256` (its checksum, required).
The `installer-macos` and `installer-windows` CI jobs install a freshly packaged build this way
on GitHub's runners and check the result, then uninstall (`tests/installer/`).

## Layout

```
crates/adl-schema    versioned schemas: states, errors, coordinate spaces, actions, results
crates/adl-broker    authorization + task broker, input engine, leases, approvals, audit, mock adapter
crates/adl-macos     macOS adapter: CGEvent input, CGWindowList, Accessibility, capture, session polling, TCC checks, hot key
crates/adl-windows   Windows adapter: SendInput, UI Automation, GDI capture, DWM windows, session events, hotkey
crates/adl-linux     Vela command-socket adapter, XDG portal adapter (KDE/GNOME), KWin and GNOME Shell window observers, logind, keymaps
crates/w3bs-desktop  `w3bs desktop` CLI, broker daemon, terminal and tray native controls, stdio MCP server
gnome-extension/     GNOME Shell extension: top-bar native control + window data (org.w3bs.Desktop.Shell)
packaging/           install.sh (Linux, macOS), install.ps1 (Windows), package.sh (release archives), systemd user unit
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

## macOS onboarding

The broker needs two privacy permissions, granted to the process macOS holds responsible for it:
`~/.local/bin/w3bs-desktop` when the installer's LaunchAgent runs it (the normal case), or your
terminal app (Terminal, iTerm) when you start `w3bs desktop daemon` from it yourself. After
granting, restart the broker (`launchctl kickstart -k gui/$(id -u)/org.w3bs.desktop.broker`). The
binary is not code-signed, so macOS may ask again after an upgrade replaces it:

1. **Accessibility** (System Settings > Privacy & Security > Accessibility): posting input events,
   focus and `inspect`. Without it, enabling is refused (Blocked) with the pane named.
2. **Screen Recording** (System Settings > Privacy & Security > Screen Recording, called "Screen &
   System Audio Recording" on macOS 15 and later): screenshots and window titles. Without it,
   captures are refused and window titles are empty (windows are reported as degraded).

**Input Monitoring is not needed**: the layer never reads keys; takeover is detected from HID idle
time, and the emergency stop is a registered hot key. `w3bs desktop doctor` shows what is allowed
without prompting; `w3bs desktop doctor --prompt` (or "Check macOS permissions" in the menu-bar
control) shows macOS's prompts. Run `w3bs desktop daemon --adapter native`, then `w3bs desktop
tray` for the menu-bar control (its title reads `Agents: Disabled` / `Agents: Enabled`; emergency
stop: Control+Option+Command+Escape).

Known macOS gaps: `Char` keys in shortcuts use US ANSI positions (text is layout-independent);
without Accessibility the Dock bar is not counted as an occluder (its whole-display overlay window
is ignored) and focus falls back to the front-most normal window; our own posted events also reset
the HID idle clock, so human input landing within ~50 ms of agent input is not told apart;
Mission Control / Launchpad are not detected as modal.

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
desktop/tests/rig/macos-e2e.sh      # macOS logged-in session (GitHub macos-latest job `macos-e2e`)
desktop/tests/installer/macos.sh     # install.sh on macOS: LaunchAgents, launchd broker, menu bar, uninstall (job `installer-macos`)
desktop/tests/installer/windows.ps1  # install.ps1 on Windows: autostart, windowless broker + tray, uninstall (job `installer-windows`)
```
