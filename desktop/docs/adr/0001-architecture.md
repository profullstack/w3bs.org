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
   the same session) for KDE/GNOME; Vela's command socket for observation and interlocks; logind for
   lock/suspend. A wlroots virtual-input backend (Vela with `VELA_DEBUG_INPUT=1`, Sway, Hyprland,
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

- The KDE/GNOME backend has no window list or focus until the KWin script / GNOME Shell extension
  exist, so targeted input on those desktops is refused rather than guessed.
- Same-user hostile processes are out of scope, as the PRD states.
