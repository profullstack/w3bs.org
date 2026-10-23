# W3BS OS 1.2 on Debian 13 "trixie"

W3BS OS moves from Alpine to Debian 13 (current stable, LTS after that), with systemd and
without what Ubuntu servers add (no snap, no cloud-init). Versions are semver: 1.2.0, 1.2.1, ...
CI (`.github/workflows/os-debian.yml`) builds, tests and publishes `os-v1.2.<n>` on every merge.

## Live installer ISO (this directory)

    os/debian/build.sh 1.2.2        # -> os/debian/out/w3bs-os-1.2.2-amd64.iso (live-build, privileged container)

- amd64 hybrid ISO, BIOS (isolinux) + UEFI (grub), boot menu auto-boots after 5 s
- systemd-networkd on every wired card: DHCPv4 + IPv6 RA/DHCPv6, DNS through systemd-resolved
  (libnss-resolve: names resolve even though live-boot rewrites /etc/resolv.conf)
- SSH on by default: random one-time root password (w3bs-live-ssh.service), shown on tty1 with
  `ssh root@<address>`; tty1 and ttyS0 log root in on the machine's own consoles
- tty1 asks how to install: **1 graphical** (default when there is a display adapter), 2 text,
  3 shell

## Installers

`w3bs-os-install` (text, also every flag for unattended installs: `--help`) does the install;
`w3bs-os-install-gui` is a GTK 4 front end (`/usr/share/w3bs-os/installer-gui.py`) run full
screen under cage with software rendering (wlroots pixman, GSK cairo), so it works on any
display a kernel driver drives, VMs included. Every GUI choice becomes an installer flag, so the
two installs are the same install. GUI pages: install type (desktop or server), region (XKB
layout + variant, time zone, computer name, web domain), network (automatic, or static IPv4 /
IPv6 / DNS on a chosen card), accounts (root password first, then the user), disk + encryption,
review with an "I understand" check, progress with the live log, done (ejects the DVD, reboots).

What an install gets: GPT (bios_grub + EFI + /boot + root), LUKS2 by default, debootstrap
trixie, GRUB BIOS + UEFI (first in BootOrder, plus the removable path), locale, keyboard,
timezone, hostname/domain, root then users (sudo, zsh), systemd-networkd or the static address,
the W3BS landing page, curl wget git, w3bs CLI, mise (node, bun, pnpm), readm3, and cli-tools
root-ubuntu.sh at first boot (nginx pages, firewall, oh-my-zsh, tmux).

## Desktop

`--desktop hyprland` (the default with a display adapter) installs **W3BS DE** (`de/` in this
repo, w3bs.org/de) on Hyprland 0.55 from trixie-backports, in one apt transaction with the rest
(backports preferred: Hyprland needs backports' PipeWire and libxkbcommon). The ISO carries the
DE from the same commit (`build.sh` copies `de/`); the installer sets it up in each user's home
(`~/.local/share/w3bs-de`, `w3bs-de update` later) and seeds `~/.config/w3bs-de/hypr/user.lua`
with the chosen keyboard. greetd + tuigreet on tty1 run `w3bs-hyprland`, which starts
`w3bs-de session` and handles VMs (scale 1, software cursors via a per-login `session.lua`),
a llvmpipe retry, and a Sway (pixman) fallback with the same keys. Firefox ESR and TronBrowser
come with it, and the **Agentic Desktop Layer** (`w3bs-desktop`, glibc build in /usr/local/bin):
`w3bs-de autostart` starts its broker and tray, and agent access stays OFF until the user turns it
on.

## Tests

`test/dtest.py` boots the ISO in QEMU (KVM when available) and checks the live system, an
install, and the installed system (login, sudo, tools, user units; for a desktop: Hyprland,
Waybar, config errors, the Agentic Desktop Layer broker + tray, agent access off). CI runs four:
UEFI server, BIOS `--encrypt`, UEFI `--desktop`, and UEFI `--gui` (the graphical installer clicks
through by itself via its test hooks, then the desktop checks run on the result).
