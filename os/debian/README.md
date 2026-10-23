# W3BS OS 1.2 on Debian 13 "trixie"

W3BS OS moves from Alpine to Debian 13 (current stable, LTS after that), with systemd and
without what Ubuntu servers add (no snap, no cloud-init). Versions are semver: 1.2.0, 1.2.1, ...

## Live installer ISO (this directory)

    os/debian/build.sh 1.2.0        # -> os/debian/out/w3bs-os-1.2.0-amd64.iso (live-build, privileged container)

Done:
- live-build, amd64 hybrid ISO, BIOS (isolinux) + UEFI (grub), boot menu auto-boots after 5 s
- systemd-networkd on every wired card: DHCPv4 + IPv6 RA/DHCPv6, DNS through systemd-resolved
  (libnss-resolve: names resolve even though live-boot rewrites /etc/resolv.conf)
- SSH on by default: random one-time root password (w3bs-live-ssh.service), shown on tty1 with
  `ssh root@<address>`; tty1 and ttyS0 log root in on the machine's own consoles

Tested in QEMU, BIOS and UEFI: boots unattended, IPv4 + IPv6 SLAAC, DNS, deb.debian.org 200,
ssh root@ with the shown password.

## Next (port from os/iso, in order)

1. Installer: `w3bs-os-install` on Debian: disk review + typed erase, LUKS2 + LVM, ext4, debootstrap
   trixie, linux-image-amd64, GRUB (BIOS + UEFI, efibootmgr first in BootOrder), locale, keyboard,
   timezone, hostname/domain, root password first then users (sudo), systemd-networkd (DHCP + IPv6)
   or the static --ip/--ip6 options, eject before reboot.
2. Box setup: cli-tools root-ubuntu.sh (Debian + systemd) instead of root-w3bs-os; WEB_DOMAIN from
   the domain question; the W3BS landing page; curl wget mise bun pnpm readm3; w3bs + w3bs-desktop
   (glibc builds).
3. Graphical installer (GTK 4 under cage). The desktop is done (1.2.1): `--desktop hyprland` (the
   default with a display adapter) installs Hyprland 0.55 from trixie-backports in one transaction
   with the rest (backports preferred: Hyprland needs backports' PipeWire and libxkbcommon), greetd +
   tuigreet on tty1 (UTF-8 console), Waybar, Firefox ESR, TronBrowser, the W3BS wallpaper, and the
   Sway (pixman) fallback session. The Agentic Desktop Layer for Debian comes next.
4. CI: build + the QEMU install tests (os/iso/test/vmtest.py) on the Debian ISO; release 1.2.x and
   point w3bs.org/os at it. Alpine (os/iso) is then retired.
