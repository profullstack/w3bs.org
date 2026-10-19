# W3BS OS Product Requirements and Architecture

**Version:** 0.3 (base changed from Ubuntu to Alpine Linux, phones on Nura/postmarketOS, §20 Docker editions as the first release; v0.2 added §16 to §19)  
**Date:** October 19, 2026  
**Owner:** Profullstack, Inc.  
**Product home:** https://w3bs.org/os  
**Status:** Implementation specification. The OS images, installer extensions, and hardware certifications described here are proposed work.

## 1 Product direction

Build **W3BS OS** as a lightweight Alpine Linux-based operating-system family with a configurable installation wizard, a desktop that people and agents can operate, and the complete Profullstack CLI and desktop collection distributed through W3BS Packages.

Use Alpine Linux (current stable branch) as the one base for containers, servers, PCs and Macs, and Nura (formerly postmarketOS, itself Alpine-based) for phones and tablets. Make KDE Plasma the default desktop and zsh the default user shell. Offer GNOME and additional qualified desktop profiles. Include Qt and GTK application support, the W3BS Agentic Desktop Layer, W3BS Training, MoshCode, PairUX, DiskPush, and every other Profullstack CLI or desktop package in the release's approved catalog snapshot.

Native installation expands through hardware-specific editions: PCs, Intel Macs, supported Apple Silicon Macs, and selected mobile devices. Application compatibility is a separate capability. Ship practical Windows and Android runtimes, native Linux/Nix support, and clearly identified Apple-app access routes.

The product goal is broad device coverage. General replacement-OS installation on stock iPhones/iPads and universal local compatibility with every Windows, macOS, iOS, and Android application are not achievable launch commitments. A companion app or remote session must remain visibly distinct from installing the operating system.

## 2 Required defaults

| Area | Requirement |
| --- | --- |
| Base | Alpine Linux stable userspace (musl, apk); Nura on mobile hardware; Docker images first (§20) |
| Desktop | KDE Plasma selected initially; GNOME selectable; LXQt and Xfce offered when their profile passes qualification |
| Toolkits | Qt and GTK runtimes and integration installed for supported desktop applications |
| Shell | zsh for newly created human users; bash and fish selectable |
| Installer | Full setup wizard covering hardware, language, keyboard, network, desktop, shell, accounts, hostname, disks, encryption, software, and final review |
| Applications | Full Profullstack CLI and desktop suite selected by default; actual local packages and dependencies included |
| Explicit inclusions | MoshCode, PairUX, DiskPush, Agentic Desktop Layer, and W3BS Training |
| Agent interfaces | Desktop control, MCP, CLI, local API, and machine-readable diagnostics |
| Agent state | Layer installed and control available; observation, input, and recording require their own authorization |
| Compatibility | Native Linux/Nix plus configured Windows and Android routes on qualified hardware |
| Hardware | Separate support matrices and installation paths for PC, Intel Mac, Apple Silicon, and mobile devices |
| Distribution | W3BS Packages is the catalog and user-facing app lifecycle; upstream package managers retain ownership of their files |
| Local operation | Local OS login and installed local applications work without a W3BS cloud account |

A user may customize their software selection. The primary advertised installation must deliver the full collection by default. A lighter software selection is an explicit user choice.

## 3 Base architecture and desktop profiles

### Base decision (v0.3, October 19, 2026)

W3BS OS is built on **Alpine Linux**. The previous draft used Ubuntu 26.04; it was replaced because one small base must cover containers, servers, desktops and phones. Measured the same day:

| Base | Container base size | KDE Plasma | GNOME | Waydroid in archive | Phones | Mac hardware |
| --- | --- | --- | --- | --- | --- | --- |
| **Alpine 3.24** (Nura on phones) | **13 MB** | 6.6.6 (edge 6.8) | 50.3 (edge 51) | yes | widest mobile Linux device list (Nura) | Apple Silicon M1/M2 via Nura's Asahi kernel package; T2 Intel needs a port |
| Ubuntu 26.04 | 163 MB | 6.6.6 | 50.1 | no | none | t2linux images; Asahi via a 9-month Ubuntu Asahi |
| Debian 13 (Mobian on phones) | 119 MB | 6.3.6 | 48.7 | no | a handful | t2linux apt repo; no Asahi port |
| Fedora 43 | 272 MB | - | - | - | tablets | Fedora Asahi Remix (Asahi flagship) |

Alpine is also the most common base for Docker images, so the server edition and the containers people already run are the same OS. Alpine stable branches are released every six months with two years of support for main; W3BS tracks the newest stable branch and moves to the next within one release cycle.

Consequences that the build must handle:

- **musl, not glibc.** Prebuilt glibc binaries do not run directly. Electron desktop apps (PairUX, DiskPush, HDTilt and others) ship as Flatpaks, whose runtimes carry glibc. Steam/Proton use Flatpak as well. Bun-compiled CLIs use Bun's musl target; the Rust desktop layer and `w3bs` CLI build for `*-linux-musl`. Nix brings its own glibc closure. `gcompat` is not a support route.
- **Init.** Alpine uses OpenRC; GNOME requires systemd and W3BS desktop components use systemd user units. Desktop profiles follow Nura's systemd variant; server and container editions may stay on OpenRC with an OpenRC service for each W3BS daemon.
- **T2 Intel Macs** need the t2linux kernel patches and drivers carried in a W3BS kernel package (Alpine has none today).

| Component | Proposed implementation |
| --- | --- |
| Kernel and boot packages | Alpine `linux-lts`/`linux-stable` on PCs; Nura device kernels (including the Asahi kernel) on phones and Apple Silicon; a W3BS T2 kernel package |
| Desktop shell | KDE Plasma/KWin by default, with a separate GNOME profile |
| Additional desktops | LXQt and Xfce profiles with explicit session and agent-capability qualification |
| Installer | Calamares on PCs (packaged in Alpine) with W3BS modules; Nura's flashing tools on phones; `docker run` for containers |
| Qt system interfaces | Qt/QML or native desktop integration for installer, panel controls and OS settings |
| Existing product interfaces | Preserve each product's existing desktop implementation and bundled CLI |
| Desktop automation | Reuse the existing W3BS broker and adapters |
| App catalog and receipts | Extend the existing W3BS package manager |
| OS package ownership | apk for Alpine system files; a small signed W3BS apk repository for distro integration |
| Desktop app backends | Native W3BS packages, apk packages, Flatpak (glibc and Electron apps) or other qualified formats |
| Developer environments | Nix alongside Alpine development tools |
| Web and registry services | Extend the current W3BS repository and deployment |

The current Profullstack desktop blueprint uses Electron with the CLI bundled inside the desktop installation. A Qt-based OS desktop does not require rewriting those products. Preserve product implementations and use Qt where it serves the OS interface. [Profullstack stack](https://profullstack.com/stack)

The current W3BS repository pins Bun and lists Express and PostgreSQL dependencies. Extend the actual project rather than migrating it merely to conform to a generic new-product recipe. Keep the desktop broker, package manager and training system as shared components. [W3BS package metadata](https://github.com/profullstack/w3bs.org/blob/main/package.json)

### Desktop integration

Install one selected desktop and one appropriate display manager in the ordinary setup. Multiple desktop sessions are an advanced option. Configure the correct desktop portals, accessibility services, notifications, file dialogs, keyring and theme integration for each profile.

Support Qt and GTK applications side by side. Include the library versions required by shipped applications, including GTK 3/4 and Qt 6 where applicable. Supply legacy runtime components only when an included application needs them. Do not remove accessibility, audio, power management or firmware to make the package count look smaller.

Wayland is the primary KDE/GNOME path. XWayland compatibility for older Linux applications is distinct from installing a complete X11 desktop session. LXQt/Xfce profiles must advertise their actual session and Android-runtime support. A nested Wayland compositor can be investigated for X11 profiles, but is not assumed to provide equivalent performance or agent capabilities.

### Shell behavior

Set zsh as the selected user's login and interactive shell. Keep the system's `/bin/sh` (busybox ash) intact for system scripts. Generate shell completions and PATH integration for installed W3BS tools, and validate the same commands in bash and fish.

Use a simple, fast prompt with useful git information. Optional plugins must not require network access during shell startup. Changing shells after installation preserves user files and configuration.

## 4 Installation wizard

Use Calamares for the ordinary Linux installation framework. Its module architecture covers the standard setup sequence and can be extended with W3BS-specific configuration. Desktop-profile resolution and a visible shell selector are product work, not capabilities to assume already wired together. [Calamares user's guide](https://calamares.io/docs/users-guide/)

| Step | User choices and required behavior |
| --- | --- |
| 1 Device check | Detect architecture, exact hardware family, boot mode, disks, available storage, graphics and networking. Show the support status for this device. |
| 2 Language and accessibility | Select language, region, timezone, accessibility options and display scaling. |
| 3 Keyboard | Select layout/variant and provide a test field, including non-US layouts. |
| 4 Network | Configure Ethernet or Wi-Fi; identify which remaining choices are available offline. |
| 5 Desktop | Show KDE, GNOME and qualified alternatives with concise descriptions, previews and required download/storage sizes. |
| 6 Shell and developer setup | Select zsh, bash or fish. Offer development-tool bundles and optional SSH configuration. |
| 7 Local account | Collect full name, username, password and confirmation, hostname, and administrator-group membership. Cloud account connection is optional. |
| 8 Storage | Offer tested erase-disk, install-alongside and manual-partition paths where the hardware supports them. Configure encryption, filesystem and swap. |
| 9 Applications | Select the full Profullstack CLI/desktop suite by default. Show included products and permit deliberate customization. |
| 10 Compatibility | Show Windows, Android, Nix and remote-host options, with hardware requirements and genuine extra downloads. |
| 11 Agent setup | Explain the installed desktop layer and optional agent connections. Keep activation and training permission separate. |
| 12 Review | Display the exact target disk, partitions, desktop, shell, packages, download size and destructive changes before committing. |
| 13 Install and verify | Install from a fixed manifest; verify downloads; track progress; report failures without declaring an incomplete install successful. |
| 14 First boot | Generate machine-specific identity, finish drivers/keyring setup, expose the agent control and offer a short functional check. |

Passwords and secrets never enter installer logs or the reusable setup profile. Export a versioned profile containing nonsecret choices so the setup can be repeated.

The wizard must validate that the selected shell exists and is registered before assigning it. Hostnames and usernames need platform-appropriate validation. Automatic login is optional and must explain its interaction with encrypted keyrings and disk encryption.

### Disks and recovery

Provide full-disk encryption in the supported automatic PC layout, with a tested recovery workflow. Define recovery behavior before advertising snapshot rollback. Install-alongside is offered only when the current partition and encryption state permits the documented operation.

The final disk-change review is a normal installation step. It identifies the physical device and the affected partitions, rather than asking for a vague approval.

Apple Silicon and T2 Macs use their own preparation rules. The generic PC partitioner must not erase Apple recovery or boot structures as a side effect of a generic workflow. Android flashing is likewise a different backend with device-specific checks.

### Offline and online delivery

Provide a small network installer and a full offline image for the primary desktop profile. Other offline desktop variants may be built from the same manifest system.

An offline image must contain all packages advertised as available offline. If a desktop choice or compatibility image requires a download, say so before storage changes begin. For the default full offline installation, the Profullstack suite is present on disk at first boot. A website shortcut or deferred download does not count as a preinstalled executable.

## 5 Full Profullstack application collection

The catalog, rather than a handwritten list, defines the suite. Use the existing ownership-aware export:

```sh
w3bs packages --ours --json
```

Resolve its owned entries into a versioned suite manifest containing the appropriate CLI, desktop and bundled MCP/TUI surfaces for the target platform. Ownership must come from the catalog's verified publisher mapping; repository organization and domain alone are insufficient. MoshCode is explicitly included through its W3BS identity.

[W3BS package catalog](https://w3bs.org/packages) · [Package-manager command reference](https://github.com/profullstack/w3bs.org/blob/main/docs/package-manager.md) · [Catalog source](https://github.com/profullstack/w3bs.org/blob/main/catalog/catalog.json)

### Named inclusions

These are required examples within the full collection, not the entire membership list.

| Product | Existing W3BS package identity | Required installed surface |
| --- | --- | --- |
| MoshCode | `moshcode.sh/moshcode` | moshcode, moshscript and required runtime dependencies |
| PairUX | `pairux.com/pairux` | Desktop application and CLI |
| DiskPush | `diskpush.com/diskpush` | Desktop application and CLI |
| Agentic Desktop Layer | `w3bs.org/agentic-desktop-layer` | Broker, platform adapter and persistent human control |
| W3BS Training | `w3bs.org/w3bs-training` | Local capture/review integration, CLI and dataset tooling |
| ThreatCrush | `threatcrush.com/threatcrush` | Qualified desktop and CLI surfaces |
| Nixamp | `nixamp.com/nixamp` | Desktop player and CLI |
| HDTilt | `hdtilt.com/hdtilt` | Desktop player and CLI |
| TronBrowser | `tronbrowser.dev/tronbrowser` | Launcher, supported browser engine and CLI |
| Nightcell7 | `nightcell7.com/nightcell7` | Linux desktop game and launcher |
| Myna | `mynaposter.com/myna` | CLI and desktop build once its release artifact is qualified |
| r4ck | `r4ck.dev/r4ck` | CLI and qualified desktop surface |

Include the remaining Profullstack-owned CLI/desktop entries through the same resolution process. Reconcile desktop products from the Profullstack inventory that lack a catalog entry, including the intended media2markdown mapping, before claiming the collection is complete.

### Existing artifacts and packaging work

Release metadata confirms Linux desktop artifacts for DiskPush 0.16.2, PairUX 0.17.0 and HDTilt 0.4.0. These are inputs to qualification; listed binaries have not thereby been proven to work in W3BS OS. [DiskPush release](https://github.com/profullstack/diskpush/releases/tag/v0.16.2) · [PairUX release](https://github.com/profullstack/pairux.com/releases/tag/v0.17.0) · [HDTilt release](https://github.com/profullstack/hdtilt.com/releases/tag/v0.4.0)

Do not equate a package's current CLI installer with its desktop build. Record repository release version and embedded application version separately when they differ. An application with no qualified artifact remains an explicit packaging task, not a silently omitted product.

TronBrowser's current README describes a launcher and extension using a system Chromium/Ungoogled Chromium engine. Include and test that engine as part of its image dependency set. [TronBrowser source](https://github.com/profullstack/tronbrowser.dev/blob/main/README.md)

### Suite resolution and release membership

For every expected entry:

1. Resolve the canonical package identity, publisher and source repository.
2. Determine its actual surfaces and target architectures.
3. Pin the exact release, artifact and dependency versions.
4. Record checksums, signature metadata, licenses, executable names and desktop entries.
5. Install the application and dependencies into the image's selected package scope.
6. Register receipts with the correct update owner.
7. Verify CLI discovery and desktop launch, plus one useful representative function.
8. Fail the full-suite release gate if a required supported-target entry is absent or broken.

For software with both surfaces, install both without conflicting command shims. A bundled CLI may satisfy the CLI requirement; duplicate installations are unnecessary.

The current catalog assigns openmcp to both profullstack.com/cli-tools and logicsrc.com/openmcp, and proxy to both profullstack.com/cli-tools and profullstack.com/proxy. Resolve each collision with one canonical command owner plus explicit alternate invocation. Package installation order must not decide which tool a command starts.

The shipped suite is frozen per OS release. New official applications enter subsequent suite updates through an explicit membership change. Preserve user exclusions and removals so an update does not reinstall an app the user deliberately removed.

### Application startup and credentials

Preinstalled tools do not require every service to run at login. Ship their executables and launchers, then start application processes and compatibility runtimes when needed.

MoshCode engines, PairUX sessions, DiskPush destinations, cloud accounts and API credentials are configured by their users. Installation must not initiate remote sessions, uploads, paid agent work or background campaigns.

Package recipes must inspect upstream installer behavior. Any optional proxy, DNS, local certificate-authority or host-trust setup belongs to an explicit setup choice; it must not be baked into an image using a build-machine identity. Use the publisher's documented packaging or opt-out modes where provided.

MoshCode's current installer requires Node and production npm dependencies and attempts a separate proxy/trust integration. Package its code, resolved dependencies and useful session tooling such as tmux. When that installer is used for image preparation, apply its documented `MOSHCODE_NO_PROXY=1` mode and explicit paths; initialize any later local trust material per machine. Keep ordinary agent launch permissions intact in the default launcher. [MoshCode installer](https://github.com/moshcoder/moshcode/blob/main/install.sh) · [Runtime dependencies](https://github.com/moshcoder/moshcode/blob/main/package.json)

## 6 Package distribution and updates

W3BS is the common catalog, identity and user-facing lifecycle. Each backend remains responsible for its own package database and files.

| Package class | Authoritative owner |
| --- | --- |
| Alpine kernel, drivers, libraries and desktop packages | apk |
| W3BS OS integration and default settings packages | Signed W3BS apk repository |
| Native signed W3BS application bundles | Existing W3BS native package engine |
| apk-packaged Profullstack applications | apk with W3BS catalog receipts and orchestration |
| Flatpak applications | Flatpak |
| Nix packages and developer profiles | Nix |
| Windows app configurations | A managed runner and per-application environment |
| Android apps on desktop | The selected Android image/runtime |
| Android replacement-OS updates | Device-specific signed OTA mechanism |

The current package documentation distinguishes publisher scripts from native signed repositories: scripts can be rerun for updates but do not automatically provide verifiable artifact rollback. Use native signed releases or appropriately signed distro packages for default OS content wherever possible. [W3BS package lifecycle](https://w3bs.org/packages)

### User-facing commands

Preserve the existing `w3bs install <package>` and shared `w3bs update` behavior. The OS adds a unified update view that can orchestrate registered backends while reporting separate results. Extending the CLI to cover system providers is implementation work; the current package updater must not be described as already owning the Alpine base.

The following are proposed OS/suite extensions, not currently available installation instructions:

```sh
w3bs install w3bs.org/profullstack-suite
w3bs os status --json
w3bs os doctor --json
w3bs os setup
w3bs os update
```

Resolve exact command names against the existing CLI before implementation. Keep JSON output, errors and exit codes stable.

### Update and rollback behavior

Show a combined update plan, coordinate backend locks, preserve package pins and exclusions, and identify reboot requirements. Give each artifact one update owner. An embedded application updater must not independently modify an installation owned by APT or W3BS.

Multi-backend updates are not one atomic transaction. Persist a journal that can identify completed, failed and pending operations after interruption. App database migrations and user data need their own recovery rules.

Provide a tested recovery boot path and consistent restoration of the OS files, package database and associated kernel/boot files. A package rollback or root snapshot must not be represented as restoring every user document or cloud change.

Nix can run alongside Alpine through its supported installation model. Its profiles provide package generations; this does not make the Alpine system a NixOS system with whole-system generations. Use Linux-target Nix packages on Linux. [Nix installation](https://nixos.org/download/) · [Nix and NixOS architecture](https://nixos.org/guides/how-nix-works/)

## 7 Application compatibility

| Application family | Default route | What the product may claim |
| --- | --- | --- |
| Linux | Native packages and qualified Linux application formats | Tested applications on the stated architecture and OS release |
| Nix | Nix package manager and Linux package profiles | Linux-target packages and development environments |
| Windows desktop | Wine with a manager such as Bottles | Individually tested applications and pinned runner configurations |
| Windows games | Proton through an appropriate game integration | Per-title tested compatibility |
| Windows fallback | Optional real Windows VM and app-window integration | Tested VM workflows requiring suitable hardware and Windows/app licenses |
| Android on desktop | Waydroid in a qualified Wayland session | Tested applications matching runtime architecture and capabilities |
| macOS local | Optional Darling experiments | Specific tested applications; no general Mac desktop-app guarantee |
| macOS remote | Connection to a user's supported Mac | Remote execution, visibly labeled |
| iOS/iPadOS apps | Eligible apps on an Apple Silicon Mac running macOS, or a supported physical-device workflow | Specific Apple-hosted workflows, not a general Linux iOS runtime |

Bottles manages Wine environments. Proton's documentation identifies anti-cheat and DRM constraints, including unsupported kernel-space anti-cheat. A runtime being installed is not evidence that every application works. [Bottles](https://docs.usebottles.com/) · [Valve Proton guidance](https://partner.steamgames.com/doc/steamhardware/proton)

Waydroid documents that ARM-only APKs do not work directly on its x86-64 environment, and some GPUs require software rendering. Qualify GPU, ABI and application behavior separately; do not promise all Play Store or integrity-gated applications. [Android app installation](https://docs.waydro.id/usage/install-and-run-android-applications) · [Waydroid graphics limitations](https://docs.waydro.id/faq/get-waydroid-to-work-through-a-vm)

Darling reports working command-line tools while graphical support remains under development. Eligible iPhone/iPad apps can run on Apple Silicon Macs running macOS only where developers make them available. [Darling upstream](https://github.com/darlinghq/darling) · [Apple app availability](https://developer.apple.com/help/app-store-connect/manage-your-apps-availability/manage-availability-of-iphone-and-ipad-apps-on-macs-with-apple-silicon)

A real Windows VM is a distinct execution route. WinApps integrates applications from that guest into a Linux desktop; it is not a replacement for Windows inside the guest. Mac-hosted execution follows the applicable Apple terms. [WinApps architecture](https://github.com/winapps-org/winapps) · [Apple macOS license](https://www.apple.com/legal/sla/docs/macOS27.pdf)

### Compatibility records

Each app record must include application version, package surface, OS and CPU architecture, runner/version, desktop/session, relevant GPU, tested task, last test date, known failures and support tier.

Display clear labels: Native, Tested with Wine, Tested with Proton, Android runtime, Windows VM, Remote Mac, Experimental or Unsupported. Failed launches should explain the actual missing capability and offer a supported alternative when one exists.

CPU compatibility is separate from OS API compatibility. Translation or emulation for x86 applications on ARM requires its own engineering and test results. An ARM64 OS build does not inherit the x86-64 application's qualification.

## 8 Hardware editions and native installation

| Hardware family | Proposed route | Initial product position |
| --- | --- | --- |
| x86-64 PCs and laptops | Alpine-based UEFI USB installer | Primary release target; certify named reference devices |
| Intel Macs without T2 | Model-qualified x86-64 installation | Separate firmware/peripheral checks |
| Intel Macs with T2 | T2-enabled Linux image and preparation flow | Separate driver and boot-policy qualification |
| Apple Silicon M1/M2 | Asahi-enabled ARM64 hardware stack | First Mac ARM certification targets, model by model |
| Apple Silicon M3 family | Asahi-based preview where supported | Installer support exists; graphics/display limits require qualification |
| Newer or unsupported Apple Silicon models | Hardware research | No production installation promise until drivers and recovery pass |
| Other ARM PCs and tablets | Board-specific ARM64 image | Separate device-enablement track |
| Selected Android phones/tablets | W3BS Mobile device image | Exact unlockable model/variant list |
| Linux-oriented phones/tablets | Qualified mobile Linux image | Later branch |
| Stock iPhones/iPads | W3BS companion app or PWA | Application access; no general replacement-OS installer |

### Mac specifics

T2 Macs need hardware-specific drivers and preparation; PC Secure Boot assumptions do not carry over unchanged. Treat audio, graphics, Wi-Fi, Bluetooth, trackpad, suspend and recovery as individual checks. [T2 support matrix](https://wiki.t2linux.org/state/) · [T2 preparation](https://wiki.t2linux.org/guides/preinstall/)

Asahi added M3-family installer support in September 2026, but the current M3 matrix still records incomplete GPU/display support. Use model-specific certification, not an all-Mac claim. [Asahi M3 announcement](https://asahilinux.org/2026/09/m2-episode-1/) · [Current M3 feature matrix](https://asahilinux.org/docs/platform/feature-support/m3/)

Nura already packages an Asahi-based kernel for Apple Silicon (`linux-postmarketos-asahi`), which is the starting point for M1/M2. W3BS follows the Asahi upstream cadence for Mac hardware enablement; the Alpine branch lifecycle does not settle Mac support. [Nura Asahi kernel](https://pkgs.postmarketos.org/package/master/postmarketos/aarch64/linux-postmarketos-asahi)

For Apple Silicon, use an Asahi-compatible preparation flow from macOS, then an appropriate W3BS image or installer. Preserve required macOS, APFS, firmware and recovery structures. A generic PC ISO or generic ARM64 ISO is not sufficient. [Asahi distribution guidelines](https://asahilinux.org/docs/alt/policy/)

### Hardware support levels

Use four explicit levels: certified, preview, research and unsupported. Only certified combinations may be marketed as supported installations. Publish exact model identifiers, firmware assumptions, known nonworking components and recovery instructions.

Do not infer native hardware support from a successful virtual-machine boot. Conversely, a VM edition can be useful independently while native hardware work continues, provided it is labeled as a VM.

## 9 Mobile roadmap

### W3BS Mobile on Nura (primary)

Phones and tablets run the same Alpine base through Nura (formerly postmarketOS), with Plasma Mobile as the default shell and Waydroid for Android apps. This keeps one package base, one `w3bs` CLI and one Agentic Desktop Layer across server, desktop and phone.

Use Nura's device ports and flashing tools. A device is offered only from Nura's best-supported categories, and W3BS certifies each model itself. The installer must detect exact model/variant, required firmware, unlock support and image identity; bootloader unlocking normally wipes user data and some devices cannot be unlocked at all. [Nura/postmarketOS devices](https://wiki.postmarketos.org/wiki/Devices) · [AOSP bootloader requirements](https://source.android.com/docs/core/architecture/bootloader/locking_unlocking)

Before calling a phone supported, qualify calls, emergency calling, SMS, mobile data, cameras, audio, charging, power management, Wi-Fi, Bluetooth, GPS, sensors, encryption and signed OTA recovery. Define who maintains vendor components and security updates.

Share W3BS package identities, account/API contracts, sync and agent protocols across editions. Port Qt/GTK applications to Plasma Mobile form factors individually. KDE's installation guidance lists Nura among the supported distributions. [Plasma Mobile installation](https://plasma-mobile.org/get/)

### Android-based track (secondary)

Devices that Nura cannot support well but that need full phone function can get a later AOSP-derived W3BS image with W3BS apps as Android packages. This is a separate build and signing program; Alpine executables are not Android executables.

### iPhone and iPad

Deliver a clearly named W3BS companion app and/or PWA for catalog discovery, tasks, synchronization and supported remote access.

Apple's boot chain checks Apple signatures through the kernel. A supported general-purpose Linux replacement installer for stock iPhones/iPads is not a launchable commitment under that boot model. The companion app does not replace iOS or grant unrestricted system-wide input automation. [Apple boot security](https://support.apple.com/guide/security/boot-process-for-ipad-and-iphone-devices-secb3000f149/web)

The device reach target is broad; the native installation target is the explicit supported-hardware list.

## 10 Agentic behavior in every profile

Install the **Agentic Desktop Layer by default in every W3BS desktop profile**. Its visible human control must be part of the chosen desktop's panel or tray. Include the broker, platform adapter, diagnostic tooling and supported MCP/CLI/local API surfaces.

Preserve the existing model: human authorization, scoped capabilities, one shared input lease, immediate disable and release of held input. Startup, logout, lock and recovery must follow the layer's established state rules. [Agentic Desktop Layer](https://w3bs.org/desktop)

W3BS OS acceptance must prove real mouse movement, clicks, dragging, typing, shortcuts, window targeting and observation on its exact installed desktop version. An upstream test on another Plasma/GNOME version is not OS certification.

MoshCode provides the installed coding-agent entry point. PairUX provides its installed collaboration surface. Route W3BS-mediated desktop actions through the same authorization and input coordination instead of adding independent competing desktop-control services.

The global switch governs access mediated by the layer. Independent tools or credentials held outside it require separate controls; do not claim the switch can retract work already performed or disable every independent process.

### Training

Preinstall W3BS Training and its keyring dependencies. Keep training capture separate from permission to observe or control. Preserve local review, deletion/export, consumer grants and explicit cloud synchronization choices.

The current training implementation describes screenshot-based visual-review demonstrations rather than complete action-level training data. The OS must not convert that into a claim of automatic model training. [W3BS Training](https://w3bs.org/training)

Mobile editions need their own adapters and capability declarations. Installing a companion app on iOS is not equivalent to the desktop broker's operating-system access.

## 11 Lightweight operation

Measure the base, installed suite and running workloads separately. The full app collection increases disk and download size; idle resource usage depends on which processes run.

Keep one ordinary desktop session installed, remove duplicate default applications where the suite supplies the chosen equivalent, and avoid unnecessary application autostart. Launch Android containers, VMs, agent sessions, media indexing and heavy local models on demand.

Retain ordinary developer tools, documentation and compatibility (Nix and Flatpak cover glibc-only software). Optimize the actual processes, package dependencies and boot services that measurements identify.

Record the following against a matching upstream desktop on the same hardware and configuration:

| Metric | Measurement rule |
| --- | --- |
| Idle memory | Fixed settling time, documented caches/memory method, same display and network state |
| Background CPU | Sustained idle window, including process wakeups |
| Boot time | Separate firmware, disk unlock, login and usable-desktop phases |
| Shell startup | Local startup with network unavailable as well as available |
| Application startup | Cold and warm launch for representative CLI and desktop applications |
| Disk footprint | Base, application suite, runtime images, caches and user data reported separately |
| Laptop power | Idle battery drain, thermals and suspend/resume cycles |
| Agent overhead | Installed broker/control at idle versus enabled and actively working |

Set numerical budgets after the first instrumented baseline. Do not publish speed or memory-saving percentages before measuring the shipped build. A small installer download, low idle memory and low installed disk use are different goals.

## 12 Build and release engineering

Use a dedicated image-build project, proposed as `profullstack/w3bs-os`, for image recipes, hardware profiles, package seeds, installer configuration and qualification results. Keep W3BS website/package-manager/desktop changes in their existing repository.

Build in disposable environments from pinned inputs. Retain source revisions, package versions, suite membership, artifact hashes, image configuration and provenance. Produce a complete software bill of materials and license/source notices for each image.

Publish signed image checksums and signed update metadata. Alpine does not ship a Microsoft-signed shim, so Secure Boot on PCs needs either a W3BS shim accepted through the public shim review and signed by Microsoft, or a documented key-enrollment (MOK) step in the installer; until one exists, PC installs require Secure Boot off and the download page says so. Secure Boot must be tested on the W3BS image and installed system.

Create machine IDs, host keys, local secrets, training encryption keys and any optional local trust material on the installed device when needed. Never clone build-system identities, credentials or shared private keys into images.

Use independent W3BS OS branding and do not present the images as official Alpine or Nura releases, while honoring every component's license and source obligations (Alpine's own packages are almost entirely free software; ship source or a written offer for GPL components in every image).

## 13 Delivery milestones

| Stage | Deliverable | Exit condition |
| --- | --- | --- |
| M0 Catalog and package foundation | Frozen full-suite manifest, app-surface mapping, scoped receipts, explicit MoshCode/PairUX/DiskPush inclusion | Required artifacts/dependencies resolved and missing packages recorded as blocking work |
| M0.5 Docker editions | §20 `server` and `desktop` images on Alpine with the w3bs CLI and the Agentic Desktop Layer | Images build reproducibly, pass the §20 checks and are published with signed digests |
| M1 PC developer alpha | Minimal Alpine build, KDE and GNOME profiles, zsh, full suite, desktop layer, configurable installer | Clean installation and representative application/agent workflows pass |
| M2 PC public beta | Offline/network delivery, update/recovery workflows, qualified compatibility recipes and reference laptops | Upgrade, recovery, graphics, power and signed-image checks pass |
| M3 Additional desktop and Intel Mac support | Qualified LXQt/Xfce profiles and Intel/T2 model tracks | Each profile/device passes its own installation and agent checks |
| M4 Apple Silicon preview and certification | Asahi-enabled image and supported-model wizard | Device-specific boot, graphics, audio, storage and recovery qualification |
| M5 Android developer preview | First explicitly selected unlockable phone/tablet model | Basic phone functions, W3BS apps, signing and recovery pass |
| M6 Mobile expansion | More Android devices, mobile Linux evaluation, iOS companion | Separate device and distribution-channel gates pass |
| P1 Providers and publishing | §18 providers on Ubuntu, Fedora, Arch, macOS, Windows; §17 `w3bs publish` with signed multi-format artifacts | Install/update/remove pass per provider in clean VMs; one release reaches every declared target |
| R1 W3BS Run | §16 launcher with Wine, Proton, Waydroid and labelled remote routes | Per-app receipts drive the public compatibility list |
| S0 to S3 W3BS Studio | §19 IDE stages | Build, run and debug pass per target |

These are dependency stages, not promised dates. PC delivery should proceed while Mac and mobile hardware work follows its own track.

The original cross-platform Agentic Desktop Layer remains usable independently. W3BS OS adds an integrated distribution without requiring existing Windows/macOS users to replace their operating system.

## 14 Release acceptance criteria

A release is complete only when all applicable checks pass:

1. The selected hardware boots the installer and installed OS through the documented path.
2. KDE and GNOME configuration work; additional offered profiles have accurate qualification labels.
3. The user can set language, keyboard, shell, password, hostname and a supported disk/encryption layout.
4. The selected default shell starts successfully and finds the installed CLI collection.
5. Every required Profullstack package for the image's target has an executable/dependency receipt and passes its applicable launch check.
6. MoshCode, PairUX and DiskPush are explicitly present; desktop and CLI surfaces are checked separately.
7. Default offline images install their advertised collection without an unseen first-boot download.
8. The desktop layer can perform a real bounded mouse/keyboard task and be stopped from the human control.
9. Training remains off until selected, and installed review/keyring dependencies work.
10. Compatibility labels correspond to recorded application tests and hardware requirements.
11. Interrupted installation/update/recovery returns a truthful state and follows a tested recovery route.
12. User removals, exclusions, credentials and data remain consistent across suite updates.
13. Multiple local users do not share private accounts, recordings, app state or agent permissions.
14. Image signatures, boot behavior, notices and source obligations are complete.
15. Performance and power results are published with the exact measurement conditions.

## 15 Website and download experience

Extend **w3bs.org/os** in the existing W3BS site. Proposed sections are Overview, Download, Setup choices, Hardware support, Application compatibility, Included software, Documentation and Releases.

The download flow first identifies the device family and support tier, then offers the appropriate installer or companion app. A device with no supported native image must not receive a generic install button.

The Included software page is generated from the same released suite manifest as the image. Package details link back to **w3bs.org/packages**. The desktop and training pages remain the canonical product explanations.

Keep release status visible: specification, developer preview, beta or stable. Publish actual image links and supported-hardware results only after those artifacts and checks exist.

## 16 Native app runtime (W3BS Run)

Goal: a user double-clicks a Windows, Android, macOS or iOS app on W3BS OS and it runs as close to natively as the platform allows, through one launcher, `w3bs run <app>`, that picks the best qualified route and labels it. "Native" means the app window lives on the W3BS desktop, with its own taskbar entry, file access through portals, clipboard and the Agentic Desktop Layer able to drive it. It does not mean a visible VM desktop.

| Family | Local route on x86-64/ARM64 W3BS OS | Status |
| --- | --- | --- |
| Windows | Wine/Proton API translation with per-app pinned runners (no emulation on x86-64); FEX/box64 + Wine on ARM64 | Ship. Close to native speed for compatible apps |
| Windows (fallback) | Hidden Windows guest with RemoteApp-style windows (WinApps model), user-supplied license | Ship as opt-in |
| Android | Waydroid (real Android in a container, shared kernel), apps as desktop windows; libhoudini/libndk-style ARM translation researched for ARM-only APKs | Ship. Close to native speed |
| macOS | Darling (Mach-O loader + Darwin userspace) for CLI tools now; W3BS funds a GUI track (AppKit/Cocoa on Wayland) | Research. CLI only today |
| macOS (fallback) | Labelled remote window from the user's own Mac (Apple hardware, per Apple's license) | Ship as opt-in |
| iOS/iPadOS | No local route: iOS apps need Apple's signed runtime. Eligible apps run on the user's Apple Silicon Mac, streamed as a labelled remote window | Remote only |

Rules:

1. `w3bs run` resolves an app file (.exe, .msi, .apk, .xapk, .app, .dmg, .ipa) or catalog id to a route, installs the runner on demand, and records a compatibility receipt (§7 records).
2. Every window shows its route label (Native, Wine, Proton, Android, Windows guest, Remote Mac). Remote and guest routes are never presented as local.
3. Per-app isolation: one Wine prefix or Android profile per app by default, with its own sandbox and file grants.
4. The compatibility database is public, generated from receipts, and failures are reported with the missing capability.
5. We do not ship or download Apple or Microsoft OS images, keys or proprietary runtimes. The user supplies any license the route needs.

The PRD goal "runs every app natively" is a direction: Windows and Android get there per app through qualification; macOS gets there only as the Darling track matures; iOS stays remote unless Apple changes its boot and licensing model.

## 17 Publisher distribution (W3BS Publish)

Goal: a publisher ships one release and every W3BS user on every supported OS can install it, with signatures, updates and rollback, in minutes.

- `w3bs publish init` writes a `w3bs.toml` manifest (identity, surfaces, per-platform artifacts, permissions, runtime route); `w3bs publish` uploads, signs and submits it. CI action and API do the same.
- Identity: publisher verified by domain (DNS TXT or `/.well-known/w3bs.json`), then the existing W3BS-TRUST/W3BS-MANIFEST signing. The catalog's `publisher` mapping (§5) comes from this verification, not from vendor strings.
- Artifacts per target: native W3BS bundle (preferred), .deb/.rpm, Flatpak, AppImage, Homebrew formula/cask, winget manifest, MSIX/MSI, .dmg/.pkg, APK/AAB, and store links (Mac App Store, App Store, Play Store, F-Droid). The registry builds the downstream package-manager manifests from the one release so publishers do not maintain eight.
- Automated checks before listing: malware scan, signature and checksum, a launch smoke test in a clean VM per declared target, license and permissions review. Results show on the package page.
- Channels: stable, beta, nightly; staged rollout percentages; yank and rollback of a release.
- Install link: `https://w3bs.org/i/<package>` detects the visitor's OS and offers the right route (§15 rule: no generic button for an unsupported device).
- Paid apps use CoinPay and x402 checkout; free apps need no account.
- Existing curl publisher scripts keep working, but a package installed by default in W3BS OS must have a signed artifact (§6).

## 18 Package-manager providers and wrappers

The `w3bs` CLI runs on every OS and drives that system's own package managers as providers. W3BS records a receipt; the provider keeps ownership of its files (§6).

| OS | Providers |
| --- | --- |
| Alpine (incl. W3BS OS) | apk, flatpak, nix, w3bs native |
| Ubuntu/Debian | apt, snap, flatpak, nix, w3bs native |
| Fedora/RHEL | dnf, flatpak, nix |
| Arch | pacman, AUR helper if present, flatpak, nix |
| openSUSE | zypper, flatpak |
| Alpine | apk |
| NixOS | nix |
| macOS | Homebrew (formula and cask), MacPorts, mas (Mac App Store), w3bs native |
| Windows | winget, Scoop, Chocolatey, Microsoft Store (via winget msstore source), w3bs native |
| Android | Play Store intent, F-Droid, direct APK with signature check |
| iOS/iPadOS | App Store / TestFlight links from the companion app |
| Language managers | npm/pnpm/bun, pip/pipx/uv, cargo, go install, gem (only when a catalog entry declares them) |

Commands:

```sh
w3bs install <pkg>            # catalog id or name; picks the best provider for this OS
w3bs install apt:ripgrep      # force a provider
w3bs search <term>            # catalog + every available provider, labelled by source
w3bs update                   # one plan across providers, separate results (§6)
w3bs remove | list | info | outdated
w3bs providers                # detected providers, versions, privilege needs
w3bs apt|brew|winget|dnf|pacman|flatpak|nix ... # passthrough wrapper: runs the real tool, then records receipts
```

Rules:

1. Detection is by OS, distro ID/version (os-release), architecture and available binaries, never by guess. Missing providers are offered with the official bootstrap (Homebrew, winget, Nix), never installed silently.
2. Wrappers do not shadow the real binaries. On W3BS OS, an apk commit hook (`/etc/apk/commit_hooks.d`), an APT `DPkg::Post-Invoke` hook on Debian-family systems and equivalent hooks for other providers keep receipts current when the user runs `apt` directly.
3. Privilege: the CLI asks for elevation only for the provider step that needs it (sudo/pkexec/UAC) and never stores passwords.
4. W3BS OS ships the apk, flatpak and nix providers; the others are installed on first use.
5. Output is stable JSON with `--json`; exit codes distinguish not-found, provider-missing, permission and conflict.

## 19 W3BS Studio (open-source cross-platform IDE)

Goal: an open-source Xcode equivalent that builds, runs and debugs one project on Linux (KDE, GNOME), Windows, macOS, Android and iOS from W3BS OS, with agents as first-class users.

| Target | Build and run | Debug |
| --- | --- | --- |
| Linux KDE/GNOME | Local; per-desktop session launch (nested KWin/Mutter for the other desktop) | gdb/lldb via DAP, Qt/GTK inspectors, Wayland protocol log |
| Android | Android SDK, Gradle, emulator (KVM) or USB/Wi-Fi device; Waydroid quick run | lldb + JDWP via DAP, logcat, layout inspector |
| Windows | Cross-compile (MinGW/clang-cl/.NET) and run under the §16 Wine route; full check on a Windows guest or remote Windows machine | gdbserver/WinDbg remote, Wine debug channels |
| macOS | Remote build host: the user's Mac (or a rented Mac) runs xcodebuild over SSH | lldb remote, logs streamed back |
| iOS/iPadOS | Remote Mac builds; Simulator streamed as a labelled window; physical device over the Mac (or libimobiledevice for install/logs) | lldb via the Mac, device logs |
| Web/PWA | Local browser (TronBrowser) | Chrome DevTools protocol |

- Built on open components: Debug Adapter Protocol, Language Server Protocol, existing editors' engines where suitable. Qt shell to match the OS; projects open from any folder with a `w3bs.toml`.
- One device/target picker like Xcode's scheme bar: target OS, desktop, device or emulator, route label.
- Agents drive it through MCP and CLI (`w3bs studio build|run|test|debug --target <t> --json`) and through the Agentic Desktop Layer under the same authorization.
- `w3bs studio publish` hands the build straight to §17.
- Apple toolchains are never redistributed; macOS/iOS targets require the user's Apple hardware and Apple developer account, and the UI says so before setup.

Milestones: S0 Linux + Android build/run/debug; S1 Windows via Wine + remote Windows; S2 remote Mac host for macOS/iOS; S3 agent workflows and publish integration. These run in parallel with M1 to M3 and ship as a W3BS package installable on any OS, not only W3BS OS.

## 20 Docker editions (first release)

The first published W3BS OS artifacts are container images, because Alpine is already the common Docker base and the images need no hardware qualification. They are labelled as container editions, never as native installs (§8).

| Image | Contents | Use |
| --- | --- | --- |
| `ghcr.io/profullstack/w3bs-os:server` | Alpine stable, zsh, the `w3bs` CLI, the suite's CLIs that have musl builds, OpenRC-free single-process entry | Servers, CI, agent sandboxes; `w3bs fleet join` works inside |
| `ghcr.io/profullstack/w3bs-os:desktop` | `server` + KDE Plasma (Wayland, software rendering) + the Agentic Desktop Layer installed and visible in the panel + a VNC/noVNC endpoint | A desktop agents can operate, viewable from w3bs.org/fleet screens |

Rules:

1. Built from pinned Alpine digests and package versions in `profullstack/w3bs-os`, multi-arch (`linux/amd64`, `linux/arm64`), with an SBOM and a signed digest per tag.
2. No credentials, machine IDs, SSH host keys or desktop-layer secrets in the image; each is generated on first start.
3. The desktop layer is installed but its control starts **off**: observation and input need the same human authorization as on any other machine (§10). An environment flag may pre-authorize only in disposable CI.
4. Checks before a tag is published: the container starts, `w3bs --version` and `w3bs doctor` pass, and on `desktop` the layer completes a bounded mouse/keyboard task over its MCP interface and is stopped from its control.
5. w3bs.org/os lists the images, their digests and the `docker run` lines once these checks pass.

## Appendix Current Profullstack catalog inventory

This October 19, 2026 snapshot contains 64 entries marked Profullstack. It records expected suite membership, not proof that every CLI, desktop surface or architecture is ready. CLI command ownership conflicts are handled by the package resolution work above.

[Pinned catalog source](https://github.com/profullstack/w3bs.org/blob/e2928c20bdfb85219c69fc2cb7ffd058819e22f0/catalog/catalog.json)

The generated M0 suite manifest, `os/suite-manifest.json` (`bun scripts/os-suite.mjs`), carries the live per-entry inventory, surfaces, pins and blocking work. It replaces the hand-copied table that was here.
