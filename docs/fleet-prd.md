# W3BS Fleet: devices, desktop app and cloud app

**Version:** 0.1  
**Date:** October 19, 2026  
**Owner:** Profullstack, Inc.  
**Product home:** https://w3bs.org/fleet  
**Status:** Specification. Today's Fleet (servers, live stats, three actions, screen sharing) is live; everything else here is proposed work.

## 1 Direction

Grow W3BS Fleet from "watch your servers" into one place to manage every machine a person or team owns: **servers, desktops, laptops, phones and tablets, and other devices** (routers, single-board computers, VMs, containers). It ships as two surfaces over one API, the way ThreatCrush pairs its daemon with a cloud dashboard:

- **Cloud app:** w3bs.org/fleet, for any browser and phone.
- **Desktop app:** W3BS Fleet for Linux, macOS and Windows (house desktop pattern: Electron with the `w3bs` CLI bundled), also usable offline against devices on the local network.

Both use the same API, CLI (`w3bs fleet …`), MCP tools and TUI, per the house stack (every product ships TUI, CLI, MCP and API).

## 2 What exists today

`w3bs fleet join <token>` enrolls a machine; `w3bs fleet agent` reports hostname, OS, kernel, CPU, memory, root disk, load, uptime and the w3bs version every 30 seconds; the owner can queue three allowlisted actions (report now, update w3bs packages, restart the desktop layer) on machines that joined with actions allowed; screens are shared view-only or with control over an outbound relay (docs/fleet-screen.md). Limits: 50 machines per account, free.

## 3 Device classes

| Class | How it joins | What it reports | What it can do |
| --- | --- | --- | --- |
| Server (Linux) | `w3bs fleet join` (systemd or OpenRC service) | Stats today, plus services, open ports, pending updates, ThreatCrush findings when installed | Actions, updates, service restart, screen (when a desktop exists), shell session |
| Desktop / laptop (Linux, macOS, Windows) | Same agent, user-level, tray toggle | Stats, battery, displays, logged-in user, desktop-layer state | Actions, updates, screen, lock, locate (laptops, opt-in) |
| W3BS OS device (any edition) | Joined during setup if the user chooses | Everything above plus edition, release and suite manifest | OS update (`w3bs os update`), suite repair |
| Android phone / tablet | W3BS app (Nura/W3BS Mobile has the agent built in; stock Android uses the app with Android's own permissions) | Battery, storage, network, OS version, app inventory the user allows | Notifications, open URL, ring, app updates through the W3BS app; nothing Android does not grant an app |
| iPhone / iPad | W3BS companion app | What iOS lets an app read (battery, storage, OS version) | Notifications, open URL. No remote control: iOS gives apps no system-wide input or screen access. Full device management on iOS needs Apple MDM enrollment, a later, separate track |
| Other (router, SBC, VM, container, IoT) | Agentless over SSH from a joined machine or the desktop app, or the agent where it runs | Reachability, SSH host key, uptime, a command allowlist per device | The allowlisted commands only |

## 4 Teams and permissions

Adopt the model ThreatCrush runs in production: organization, teams under it, fleets owned by a team, roles read | write | admin, invited guests who see only their team's fleets, and agent keys (`w3bs_agent_…`, hashed, one team and one role, revocable, last used shown). Access rules live in one module. An unknown device and a forbidden one both answer 404. Agent keys can never manage people, invites or keys.

## 5 Cloud app (w3bs.org/fleet)

- Device list with class icons, online state, health, tags, fleet and owner; filters and saved views.
- Device page: live stats, history graphs (24 h free), action history, screen, shell (§7), alerts.
- Alerts: offline, disk, memory, failed update, ThreatCrush finding; delivered by email and to the desktop app, and by SMS/call through smshub for teams that turn it on.
- Bulk actions on a selection or a tag, always previewed with the exact device list.
- Installable PWA, usable on phones.

## 6 Desktop app (W3BS Fleet)

- Everything the cloud app shows, plus **local discovery** (mDNS/SSH on the LAN) and agentless devices that never touch w3bs.org.
- Native notifications, a tray status, and native VNC/terminal windows instead of the browser viewer.
- Offline mode: the local network keeps working without the cloud; changes sync when it returns.
- Bundles the `w3bs` CLI; signed builds for Linux (AppImage, Flatpak, apk on W3BS OS), macOS (notarized dmg) and Windows (MSIX/MSI), distributed through w3bs.org/packages and signed with W3BS-SIGN-1.

## 7 Remote shell

`w3bs fleet shell <device>` and a terminal in both apps: the agent opens an outbound WebSocket (same relay as screens), a PTY runs as the agent's user, and the session is recorded only in the audit log (start, end, who), never its content. Off by default per device; enabling it is a separate choice from actions and screens.

## 8 Agents

Agents act on the fleet through MCP (`fleet_devices`, `fleet_device`, `fleet_action`, `fleet_screen`, `fleet_shell` gated by role) and the CLI, with an agent key. Desktop control still goes through each machine's Agentic Desktop Layer and its human switch; a fleet role never bypasses it.

## 9 Security

- Nothing on a device listens to the network for Fleet; every connection is outbound from the agent.
- Actions are an allowlist enforced by both the server and the agent.
- Every action, screen and shell session is in the audit log with who, when and which device.
- Enrollment tokens are one-time and expire in an hour (today's rule).

## 10 Limits and pricing

Keep Fleet free at the current 50 devices per account. Teams above that, longer history and SMS alerts are paid in CoinPay or x402; there is no card checkout.

## 11 Milestones

| Stage | Deliverable |
| --- | --- |
| F1 | Device classes and tags in the data model; desktops and laptops (Linux, macOS, Windows agent with tray toggle) |
| F2 | Teams, fleets, roles, invites, agent keys (ThreatCrush model) |
| F3 | W3BS Fleet desktop app (Linux, macOS, Windows) with local discovery and agentless SSH devices |
| F4 | Remote shell, alerts (email, desktop, smshub), bulk actions |
| F5 | Android agent in the W3BS app and built into W3BS Mobile; iOS companion app with what iOS allows |
| F6 | Apple MDM track for iPhone/iPad fleets (separate enrollment and Apple requirements) |
