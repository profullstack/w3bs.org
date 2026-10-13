# ADR 0001: Broker architecture and first implementation choices

Status: accepted, 2026-10-13

## Decision

1. **Rust, std threads, no async runtime in the broker.** The broker is small, latency-bound and
   must stay responsive while an adapter hangs. Threads per connection plus atomics for the gate
   keep `disable` independent of any lock held by input dispatch. `zbus` (pure Rust) is used only
   inside the Linux D-Bus adapters.
2. **One broker, thin adapters.** Adapters report capabilities and emit primitive events. All
   authorization, leases, coordinate mapping, chunking, approvals and cancellation live in
   `adl-broker`, so a new platform cannot weaken the safety model. Adapters add only
   `safety_interlock` (e.g. "authentication prompt on screen") and session events.
3. **Two local sockets.** `agent.sock` (bearer token, agent vocabulary only) and `control.sock`
   (human controls). Same-uid peer check via `SO_PEERCRED`. Requests that raise access are refused
   when the peer's process lineage includes an agent root (the process that spawned an MCP server).
   This is defence in depth against an agent's own tool calls, not a sandbox.
4. **Enable is two-step.** `enable.request` sends a code only to native controls; confirmation must
   come from the native control or an interactive terminal typing that code.
5. **Gate = atomic flag + revocation generation.** Every primitive event re-checks flag, generation,
   lease (holder, task, generation, expiry), task state, pending approvals and hold durations.
6. **Desktop logical coordinates** are canonical. Display, window and capture spaces map into it;
   captures carry the layout generation and expire.
7. **Linux backends:** XDG RemoteDesktop + ScreenCast (Notify* methods only, never mixed with EIS on
   the same session) for KDE/GNOME; KWin scripting (scripts call back over D-Bus) for windows on Plasma; a W3BS
   GNOME Shell extension (read-only D-Bus service plus the top-bar native control) for windows on GNOME; Vela's
   command socket for observation and interlocks; logind for lock/suspend. A wlroots virtual-input backend (Vela with `VELA_DEBUG_INPUT=1`, Sway, Hyprland,
   labwc) is planned.
8. **Clean room for Vela** (GPL-3.0): see `../clean-room.md`.
9. **License:** Apache-2.0 for this directory. The w3bs.org site remains MIT.

## Alternatives considered

- *Tokio broker*: more machinery than a two-socket local service needs; harder to reason about
  cancellation latency.
- *libei/EIS directly*: better long-term input path on Plasma 6/GNOME 46+, but needs C bindings;
  the portal Notify* path ships first, EIS can be added as a separate session type.
- *Copying Vela's virtual-input client or command handling*: rejected; GPL-3.0 and the user
  asked for a clean room.
- *Trusting a caller-supplied agent name*: rejected; identity is the token, and process lineage
  is only an extra check.

## Consequences

- On Plasma, window list, focus, stacking and activation come from one-shot KWin scripts that
  report back over D-Bus (`adl-linux/src/kwin.rs`, wrapped around the portal adapter; added
  2026-10-13). On GNOME the same data comes from the W3BS GNOME Shell extension
  (`gnome-extension/`, `adl-linux/src/gnome.rs`; added 2026-10-13): it exports
  `org.w3bs.Desktop.Shell` from inside gnome-shell and answers only the process at the other end
  of its control.sock subscription (SO_PEERCRED pid, same uid), and the broker talks to it only
  when its owner is the gnome-shell process. The extension never injects input; it is also the
  GNOME native control, so unloading it disables access. Without it, targeted input on GNOME is
  refused rather than guessed.
- Windows (added 2026-10-13, `adl-windows`): desktop logical coordinates are **virtual-screen
  physical pixels**. The broker is per-monitor DPI aware (v2), so every Win32 call speaks physical
  pixels; Windows has no single DIP plane across monitors with different scale factors, so DIPs
  could not serve as one space. Displays report `scale = 1.0` (captures map 1:1) and state their
  DPI in the name. Absolute pointer motion uses `MOUSEEVENTF_VIRTUALDESK` with
  `ceil(offset * 65536 / len)` and confirms the landing pixel. The named-pipe transport uses
  overlapped I/O because synchronous pipe handles serialize reads and writes per handle, which
  would stall event delivery behind a pending read. The emergency stop is `Win+Alt+Esc`
  (mirrors Super+Alt+Escape; fallback Ctrl+Alt+Shift+F12) registered by the adapter with
  `RegisterHotKey`; the tray only names it. Physical mouse takeover uses a low-level mouse hook
  that reads only the injected flag; there is no keyboard hook. Session events come from a hidden
  top-level window (message-only windows get no broadcasts). UIPI silently drops input to
  higher-integrity windows, so such targets are refused before sending.
- Same-user hostile processes are out of scope, as the PRD states.
