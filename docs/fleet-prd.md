# W3BS Fleet: devices, desktop app and cloud app

**Version:** 0.1  
**Date:** October 19, 2026  
**Owner:** Profullstack, Inc.  
**Product home:** https://w3bs.org/fleet  
**Status:** Specification with an incremental permissions implementation. Today's Fleet includes servers, live stats, three allowlisted actions, screen sharing, OpenDevice inventories and per-machine permission checks for supported tasks. Team roles, shell, file access, desktop app automation and the native app remain proposed work.

## 1 Direction

Grow W3BS Fleet from "watch your servers" into one place to manage every machine a person or team owns: **servers, desktops, laptops, phones and tablets, and other devices** (routers, single-board computers, VMs, containers). It ships as two surfaces over one API, the way ThreatCrush pairs its daemon with a cloud dashboard:

- **Cloud app:** w3bs.org/fleet, for any browser and phone.
- **Desktop app:** W3BS Fleet for Linux, macOS and Windows (house desktop pattern: Electron with the `w3bs` CLI bundled), also usable offline against devices on the local network.

Both use the same API, CLI (`w3bs fleet …`), MCP tools and TUI, per the house stack (every product ships TUI, CLI, MCP and API).

## 2 What exists today

`w3bs fleet join <token>` enrolls a machine; `w3bs fleet agent` reports hostname, OS, kernel, CPU, memory, root disk, load, uptime and the w3bs version every 30 seconds. The owner can grant each supported task separately: report now, update w3bs packages, restart the desktop layer, view a screen and control a screen. Every Fleet task request from the page, API or CLI checks the same server policy; a queued task is checked again when the agent asks for work. New grants are off by default. A machine’s local OS permission remains an independent gate. Shell, files and browser DevTools are not implemented in the current Fleet agent. Limits: 50 machines per account, free.

**Inventory (since 2026-10-28).** An account can also store an [OpenDevice](https://logicsrc.com/docs/opendevice) inventory: one JSON file listing every device it runs, agent or not, with provider, location and what each costs. w3bs.org reads it with the shared package [`@profullstack/fleet`](https://www.npmjs.com/package/@profullstack/fleet) (the same reader ThreatCrush uses): `validateInventory` on write, `matchAgents` to pair inventory devices with joined machines by id, name or hostname (or outright, when a device lists `{"name": "w3bs", "id": "m_…"}` in its `agents`), `fleetCost` for the monthly run rate. The fleet page shows one row per device with its class and $/mo, joined machines with their live stats, devices without an agent with a join hint, and the monthly total. Totals stay per currency; a device with no stated amount shows as _unstated_ and is never guessed or converted.

| Surface          | Call                                                                                                                                                                                                                                                                                                  |
| ---------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Store (replaces) | `PUT /api/fleet/inventory` with the document as the body; `w3bs fleet inventory import opendevice.json` (`-` reads stdin). Errors come back in `error.details.errors`; warnings in the reply                                                                                                          |
| Read back        | `GET /api/fleet/inventory` returns the document whole (so `npx @profullstack/fleet list https://w3bs.org/api/fleet/inventory --header "Authorization: Bearer …"` works); `w3bs fleet inventory show`                                                                                                  |
| Delete           | `DELETE /api/fleet/inventory`; `w3bs fleet inventory rm` (joined machines stay)                                                                                                                                                                                                                       |
| Merged fleet     | `GET /api/fleet` returns `{ machines, devices, summary }`: each machine with its `device` (or null), the agentless `devices`, and `summary.monthly`, `monthly_text`, `unstated`, `by_profile`; `w3bs fleet cost` prints it. `GET /api/fleet/machines` keeps its array, each machine now with `device` |

All of these are owner only (session cookie or a `w3bs login` bearer). Limits: 500 devices, 256 KB per inventory.

## 3 Device classes

Device classes are the OpenDevice profiles (logicsrc.com/docs/opendevice): **server**, **desktop** (laptops included), **phone**, **tablet** and **other**. An inventory device states its profile; one that does not is a server when it has a hostname or address, else other (the rule in `@profullstack/fleet`). The page shows each class with its own icon. A joined machine takes the class of the inventory device it matches; one that matches none shows as "not in inventory". The rows below say how each class joins and what it can do once it has an agent; without an agent, every class still appears from the inventory with its cost.

| Class                                                        | How it joins                                                                                                  | What it reports                                                                              | What it can do                                                                                                                                                                      |
| ------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Server (Linux) · profile `server`                            | `w3bs fleet join` (systemd or OpenRC service)                                                                 | Stats today, plus services, open ports, pending updates, ThreatCrush findings when installed | Actions, updates, service restart, screen (when a desktop exists), shell session                                                                                                    |
| Desktop / laptop (Linux, macOS, Windows) · profile `desktop` | Same agent, user-level, tray toggle                                                                           | Stats, battery, displays, logged-in user, desktop-layer state                                | Actions, updates, screen, lock, locate (laptops, opt-in)                                                                                                                            |
| W3BS OS device (any edition) · profile of the hardware       | Joined during setup if the user chooses                                                                       | Everything above plus edition, release and suite manifest                                    | OS update (`w3bs os update`), suite repair                                                                                                                                          |
| Android phone / tablet · profile `phone` / `tablet`          | W3BS app (Nura/W3BS Mobile has the agent built in; stock Android uses the app with Android's own permissions) | Battery, storage, network, OS version, app inventory the user allows                         | Notifications, open URL, ring, app updates through the W3BS app; nothing Android does not grant an app                                                                              |
| iPhone / iPad · profile `phone` / `tablet`                   | W3BS companion app                                                                                            | What iOS lets an app read (battery, storage, OS version)                                     | Notifications, open URL. No remote control: iOS gives apps no system-wide input or screen access. Full device management on iOS needs Apple MDM enrollment, a later, separate track |
| Other (router, SBC, VM, container, IoT) · profile `other`    | Agentless over SSH from a joined machine or the desktop app, or the agent where it runs                       | Reachability, SSH host key, uptime, a command allowlist per device                           | The allowlisted commands only                                                                                                                                                       |

## 4 Teams and permissions

Adopt the model ThreatCrush runs in production: organization, teams under it, fleets owned by a team, roles read | write | admin, invited guests who see only their team's fleets, and agent keys (`w3bs_agent_…`, hashed, one team and one role, revocable, last used shown). Access rules live in one module. An unknown device and a forbidden one both answer 404. Agent keys can never manage people, invites or keys.

### Per-machine task permissions

Fleet stores a separate owner grant for every supported capability. A missing grant means denied. Changing a grant is available through the Fleet page or owner API, so CLI and browser calls pass through the same check. The agent heartbeat checks the grant again before queued work leaves the server. Revoking screen access closes live Fleet screen sessions. Unsupported capabilities (HQTerm shell, file read/write, browser DevTools and general desktop app use) appear as unavailable, never as working toggles. When those agents ship, each tool must call the same permission service and honor device-local controls.

| Permission                                                                        | Today                               | Default                                |
| --------------------------------------------------------------------------------- | ----------------------------------- | -------------------------------------- |
| `system.refresh`                                                                  | Report now                          | Denied                                 |
| `system.update`                                                                   | Update w3bs                         | Denied                                 |
| `desktop.restart`                                                                 | Restart the desktop layer           | Denied                                 |
| `desktop.view`                                                                    | View the configured screen relay    | Denied unless chosen during enrollment |
| `desktop.control`                                                                 | Control the configured screen relay | Denied unless chosen during enrollment |
| `terminal.shell`, `files.read`, `files.write`, `browser.devtools`, `desktop.apps` | Not in the current agent            | Denied and unavailable                 |

## 5 Cloud app (w3bs.org/fleet)

- Device list with class icons, online state, health, tags, fleet and owner; filters and saved views.
- Device page: live stats, history graphs (24 h free), action history, screen, shell (§7), alerts.
- Alerts: offline, disk, memory, failed update, ThreatCrush finding; delivered by email and to the desktop app, and by SMS/call through smshub for teams that turn it on.
- Bulk actions on a selection or a tag, always previewed with the exact device list.
- Installable PWA, usable on phones.
- SSH access checks (shipped 2026-10-29): the shared `@profullstack/fleetbar` panel, identical to threatcrush.com/fleet. crawlproof runs the checks 24/7 in the user's Fleet project and asks each server whether it would accept a public key from the user's logicsrc keyring (never logs in). w3bs.org proxies `/api/fleetbar/*` with its partner token and the session's email; browser session only, bearers get 401.

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

| Stage | Deliverable                                                                                                    |
| ----- | -------------------------------------------------------------------------------------------------------------- |
| F1    | Device classes and tags in the data model; desktops and laptops (Linux, macOS, Windows agent with tray toggle) |
| F2    | Teams, fleets, roles, invites, agent keys (ThreatCrush model)                                                  |
| F3    | W3BS Fleet desktop app (Linux, macOS, Windows) with local discovery and agentless SSH devices                  |
| F4    | Remote shell, alerts (email, desktop, smshub), bulk actions                                                    |
| F5    | Android agent in the W3BS app and built into W3BS Mobile; iOS companion app with what iOS allows               |
| F6    | Apple MDM track for iPhone/iPad fleets (separate enrollment and Apple requirements)                            |
