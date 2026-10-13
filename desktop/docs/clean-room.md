# Clean-room record: Vela integration

Vela (https://github.com/vela-desktop/vela) is GPL-3.0-or-later. The Agentic Desktop Layer is
Apache-2.0. No Vela code is copied, translated or adapted here.

## Process

1. **Reader (separate agent session, 2026-10-13).** Cloned Vela at commit
   `06eeaf8c4ccd72c9283f1fec3317301f95cd34d8`, studied it, and wrote a functional specification
   restricted to public interface facts: Wayland protocol and request names, socket paths, line
   commands, JSON reply keys, D-Bus names, CLI names, configuration keys and environment variables,
   with behaviour described in the reader's own words. No source, comments, internal identifiers or
   algorithms. The clone was deleted afterwards.
2. **Implementer (this code).** Never opened Vela's source. Wrote `crates/adl-linux/src/vela.rs`
   and its fake-socket test only from that specification.

## Interface facts used

- Command socket `$XDG_RUNTIME_DIR/vela-$WAYLAND_DISPLAY.sock`, one command per line, one-line JSON
  replies for queries, unknown lines ignored.
- Queries `state` (keys `locked`, `focused`, `outputs[]`, `windows[]`, `layers[]`, `focusedLayer`)
  and `window-rects` (topmost first). Command `window <id> activate`.
- Polkit prompt layer namespace `vela-polkit`; overlay layer 3 with exclusive keyboard is modal.
- Desktop identity `XDG_CURRENT_DESKTOP=Vela:KDE`.
- `vela-screen-dump FILE.png` with `VELA_SCREEN_DUMP_OUTPUT=<index>`.
- Virtual input exists only with `VELA_DEBUG_INPUT=1`; this layer never sets it.

## Finding that changed the plan

Vela does not have every hook the PRD needs. It has no RemoteDesktop portal, no libei/EIS, no
consent for input or capture, no emergency-stop hotkey API, no synthetic/physical input distinction
and no accessibility bridge. Its window, geometry, lock and authentication-prompt hooks are good, and
the layer uses them; the safety model stays in the broker.

# KWin (KDE Plasma) window observer

KWin is GPL-2.0-or-later. `crates/adl-linux/src/kwin.rs` uses only KWin's public scripting and
D-Bus interfaces as documented for script authors (develop.kde.org, "KWin scripting API"); no KWin
source and no third-party tool built on it (kdotool and the like) was read or adapted.

Interface facts used:

- D-Bus `org.kde.KWin` `/Scripting` `org.kde.kwin.Scripting.loadScript(path, pluginName) -> id`,
  `unloadScript(pluginName)`, and `/Scripting/Script<id>` `org.kde.kwin.Script.run()`.
- Script globals `workspace.stackingOrder` (bottom-most first), `workspace.windowList()`,
  `workspace.activeWindow` (read and assign), `workspace.currentDesktop`, and `callDBus(service,
  path, interface, method, args...)`.
- `Window` properties `internalId`, `resourceClass`, `desktopFileName`, `caption`, `frameGeometry`,
  `minimized`, `hidden`, `normalWindow`, `dialog`, `utility`, `managed`, `deleted`,
  `onAllDesktops`, `desktops`, `output.name`.
- Window classes of KDE's polkit agent (`polkit-kde-authentication-agent-1`,
  `org.kde.polkit-kde-authentication-agent-1`) and screen-locker greeter (`kscreenlocker_greet`).
