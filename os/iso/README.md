# W3BS OS installer ISO (v0.1, network installer)

`w3bs-os-<version>-x86_64.iso` is a hybrid ISO for x86_64 PCs. It boots under UEFI and
legacy BIOS from a DVD or a USB stick, starts an Alpine Linux 3.24 live system that runs
from RAM, and offers the text installer `w3bs-os-install` on the first console.

It is a **network installer**: the base system comes from the ISO, but the desktop and
everything else downloads from the Alpine mirrors during the install. A wired (DHCP)
connection is required. The full offline image in docs/os-prd.md §4 is not built yet.

**Secure Boot must be off.** Alpine has no Microsoft-signed shim, so the ISO and the
installed system do not boot with Secure Boot on.

## What gets installed

| Choice | Default | Option |
| --- | --- | --- |
| Disk | the largest internal disk (removable and installer media are skipped), erased after a review | `--disk /dev/X` |
| Encryption | LUKS2 full-disk encryption (LVM on LUKS: swap and ext4 root), passphrase at boot | `--no-encrypt` |
| Mode | Alpine `sys` install (setup-disk `-m sys`), grub on UEFI, syslinux on BIOS | |
| Keyboard, timezone, hostname | `us us`, `UTC`, `w3bs` | `--keymap`, `--timezone`, `--hostname` |
| Network | DHCP on every wired interface, IPv6 autoconfigured (SLAAC) by default; the internet is checked (IPv6 first, then IPv4) before anything is erased, and with no DHCP the interactive installer asks for the provider's static address | `--ip ADDR/PREFIX --gateway GW [--dns "A B"]`, `--ip6 ADDR/PREFIX --gateway6 GW6`, `--iface NAME` |
| User | `w3bs` ("W3BS User"), zsh login shell, `wheel` + doas; root login locked | `--user`, `--fullname` |
| Install type | **desktop** (graphical) when a display adapter is found, **server** (command line only) when none is; asked first | `--server` (= `--desktop none`) |
| Desktop | Hyprland with greetd + tuigreet (below); the installer shows a numbered menu | `--desktop hyprland\|plasma\|gnome\|xfce\|none` |
| W3BS | `w3bs` and `w3bs-desktop` (musl builds) in `/usr/local/bin`; the W3BS repository (`https://w3bs.org/repo/`, pinned to `registry/FINGERPRINT`) is added for each user at first login | |
| Browsers | desktop installs: Firefox (SUPER+B) and [TronBrowser](https://tronbrowser.dev) (SUPER+SHIFT+B) for every user; TronBrowser runs on the Flathub Ungoogled Chromium, since its own engine is a glibc build | |
| Dev tools | every install: curl, wget, Node (Alpine, musl), mise managing bun and pnpm per user (`mise use -g bun npm:pnpm`), and `pnpm i -g @profullstack/readm3`; on PATH via /etc/profile.d/w3bs-dev.sh | |
| Box setup | every install runs [root-w3bs-os](https://github.com/profullstack/cli-tools/blob/master/root-w3bs-os.sh) (cli-tools, pinned + sha256-checked) for the users: oh-my-zsh, oh-my-tmux, threatcrush, moshcode, hqterm, ufw, swap/zram/earlyoom, per-user Caddy; services it cannot start in the install chroot are finished by a one-time run at first boot (/var/log/w3bs-first-boot.log) | |
| Encryption choice | text: the yes/no question or `--no-encrypt`; graphical: "Encrypt the disk" or "Do not encrypt" on the disk page | |
| Agentic Desktop Layer | broker and tray autostart in the desktop session; agent access is **off** until the user turns it on (never `--enabled-by-default`) | |

The installer shows a review screen (exact disk, size, model, current partitions, every
choice) and writes nothing until you type the disk name, for example `/dev/nvme0n1`.
It exits non-zero and prints `INSTALL FAILED` on any error, and verifies the installed
system (user shell, groups, locked root, bootloader, kernel, LUKS, the w3bs binaries,
the desktop packages and display manager) before it reports success. Passwords are read
without echo (or from files), never passed on a command line, and never written to the log
(`/var/log/w3bs-os-install.log`, copied to the installed system) or to the nonsecret
install profile (`/etc/w3bs-os/install-profile`).

### Hyprland

The default desktop (`--desktop hyprland`, menu choice 1) installs [Hyprland](https://hypr.land/) from Alpine
community on `setup-wayland-base` (elogind, polkit, udev, dbus), since `setup-desktop` has no
Hyprland. Login is greetd + tuigreet on tty1, the screen the machine boots to (user name, then password; tty2-6 keep text logins); the session is
`/usr/local/bin/w3bs-hyprland`, which copies the W3BS default
`/usr/share/w3bs-os/hypr/hyprland.conf` to `~/.config/hypr/hyprland.conf` on the first login
(edit that copy; the keyboard layout comes from `--keymap`). Included: Waybar, foot (SUPER+Return),
fuzzel (SUPER+D), mako, Font Awesome (Waybar icons), swaylock/swayidle, grim/slurp (Print), PipeWire, Thunar, Firefox,
hyprpolkitagent, and xdg-desktop-portal-wlr + gtk (`xdg-desktop-portal-hyprland` is not in
Alpine 3.24, so screen sharing goes through the wlr portal). The Agentic Desktop Layer broker
and tray start with the session (`/usr/share/w3bs-os/hypr/desktop-layer.sh`, which waits for
Waybar's tray host; log in `~/.local/state/w3bs-os/desktop-layer.log`) with agent access off,
and SUPER+ALT+Escape turns agent access off; Hyprland has no certified adapter yet
(docs/agentic-desktop-layer-prd.md lists it as an expansion target), so `w3bs-desktop doctor`
reports what works.

### In a virtual machine (VirtualBox, VMware, QEMU)

Hyprland needs a display adapter with a kernel DRM driver. In VirtualBox use Settings >
Display > Screen: Graphics Controller **VMSVGA**, Video Memory **128 MB**, and Enable 3D
Acceleration (turn it off if the desktop still does not start). The installer adds the
VirtualBox guest additions when it runs inside VirtualBox. In a VM the session uses software
cursors, and if Hyprland exits within seconds `w3bs-hyprland` retries once with software
rendering (Mesa llvmpipe); if that fails as well it explains why on the console (driver found,
VM settings to change, the tail of the log) instead of dropping back to the greeter. Log:
`~/.local/state/w3bs-os/hyprland-session.log`. Press **Host+F** (Right Ctrl+F) to leave
VirtualBox full-screen mode.

## Graphical installer

The first thing the live system asks on the screen is how to install: **1 graphical
installer** (the default when the machine has a display), **2 text installer**, or 3 a shell.
`w3bs.installer=gui` or `w3bs.installer=text` on the kernel command line answers it in advance.

The graphical installer (`w3bs-os-install-gui`) is a GTK 4 wizard, full screen under the
`cage` kiosk compositor with software rendering (wlroots pixman + GTK cairo), so it runs on
any display a kernel driver drives, VMs included. Its packages are on the ISO and load into
RAM only when it is chosen. Pages: install type (desktop or server), desktop, keyboard / time
zone / computer name, user and password, disk and encryption, review (a checkbox confirms the
erase), live progress, and Reboot. It only collects answers: it runs `w3bs-os-install` with
the matching flags, so both installers make the same install. "Use the text installer" is on
every page, and any failure to start falls back to the text installer. Log:
`/var/log/w3bs-os-install-gui.log`.

## After the install

Both installers eject the installer DVD before they reboot, so the machine (or VM) starts
the installed system: the login screen asks for your user name, then your password (and,
with encryption, the disk passphrase comes first). A USB stick is only unmounted: take it
out, or pick **Boot the installed system** in the ISO's boot menu. In VirtualBox the
virtual DVD is removed on eject unless "Live CD/DVD" is ticked under Settings > Storage.

## Installer usage

Boot the ISO. tty1 logs in as root on the live system and asks whether to start the
installer; answer the prompts (Enter accepts the default). Or run it yourself:

```sh
w3bs-os-install                         # interactive
w3bs-os-install --desktop plasma        # interactive, KDE Plasma (SDDM) instead of Hyprland
w3bs-os-install --desktop gnome         # interactive, GNOME instead of Hyprland
w3bs-os-install --defaults --disk /dev/sda --yes
                                        # unattended apart from the two passwords
w3bs-os-install --defaults --disk /dev/sda --yes --no-encrypt \
  --password-file /root/pw              # fully unattended (first line of the file)
w3bs-os-install --defaults --disk /dev/sda --yes \
  --password-file /root/pw --passphrase-file /root/luks
w3bs-os-install --help
```

`--defaults` accepts every default but still asks for the user's password and the disk
passphrase on a terminal unless the files are given. `--yes` replaces the typed erase
confirmation; the review is still printed. The disk must be a whole disk of at least 8 GB
(20 GB or more with a desktop).

## Write it to a USB stick

Check the download first:

```sh
sha256sum -c SHA256SUMS
```

Linux (replace `/dev/sdX` with the stick, not a partition; `lsblk` lists disks; everything
on the stick is erased):

```sh
sudo dd if=w3bs-os-0.1.0-x86_64.iso of=/dev/sdX bs=4M status=progress oflag=sync
```

macOS: `diskutil list`, `diskutil unmountDisk /dev/diskN`, then
`sudo dd if=w3bs-os-0.1.0-x86_64.iso of=/dev/rdiskN bs=4m`.

Windows, macOS and Linux: [balenaEtcher](https://etcher.balena.io/) writes the image as-is.
[Ventoy](https://www.ventoy.net/) also works: copy the ISO onto a Ventoy stick and choose it
from the Ventoy menu (normal mode). Rufus users should pick "DD image" mode.

## Build

On any x86_64 Linux host with Docker (about 6 GB of disk, network access):

```sh
os/iso/build.sh 0.1.0
# -> os/iso/out/w3bs-os-0.1.0-x86_64.iso and os/iso/out/SHA256SUMS
```

1. `Dockerfile.bins` builds `w3bs` (package-manager/) and `w3bs-desktop` (desktop/) natively
   for musl in `alpine:3.24`, as the Docker editions do.
2. `build-in-container.sh` runs in `alpine:3.24`: it installs abuild, alpine-sdk, xorriso,
   squashfs-tools, grub-efi, mtools and syslinux, generates a throwaway abuild signing key
   (never committed, gone with the container), clones aports `3.24-stable` and runs
   `scripts/mkimage.sh --profile w3bs` with `mkimg.w3bs.sh` and `genapkovl-w3bs.sh` from
   `~/.mkimage`.
3. `profile_w3bs` is Alpine's `extended` profile plus zsh, the installer's tools and the
   W3BS overlay, with "W3BS OS" boot menus (grub on UEFI, isolinux on BIOS) and a serial
   console on ttyS0 as well as the screen.
4. `genapkovl-w3bs.sh` packs `overlay/` (installer, live tty1 autologin and banner, branding,
   first-login hook, zsh prompt, desktop-layer autostart entries) with the binaries, the
   version and `registry/FINGERPRINT` into `w3bs-os.apkovl.tar.gz`, which the live system
   unpacks at boot.

CI: `.github/workflows/os-iso.yml` runs the same build on a tag `os-v<version>` (or by hand),
boots the result under BIOS and UEFI in QEMU, and attaches the ISO and SHA256SUMS to a
**draft prerelease** `os-v<version>` for a person to review and publish.

## Test in QEMU

`test/vmtest.py` drives QEMU over the serial console and the monitor (screenshots with
`screendump`). It uses KVM when `/dev/kvm` exists and TCG otherwise. Without QEMU on the
host, run it in the test container:

```sh
docker build -t w3bs-os-qemu -f os/iso/test/Dockerfile.qemu os/iso/test
docker run --rm -v "$PWD/os/iso:/w" -w /w w3bs-os-qemu sh -c '
  python3 test/vmtest.py boot --iso out/w3bs-os-0.1.0-x86_64.iso --firmware uefi --out out/test
  python3 test/vmtest.py boot --iso out/w3bs-os-0.1.0-x86_64.iso --firmware bios --out out/test
  qemu-img create -f qcow2 out/test/disk.qcow2 20G
  python3 test/vmtest.py install --iso out/w3bs-os-0.1.0-x86_64.iso --disk out/test/disk.qcow2 --out out/test
  python3 test/vmtest.py inspect --iso out/w3bs-os-0.1.0-x86_64.iso --disk out/test/disk.qcow2 --out out/test
  python3 test/vmtest.py installed --disk out/test/disk.qcow2 --login w3bs:w3bs-test-pass --out out/test
  qemu-img create -f qcow2 out/test/crypt.qcow2 12G
  python3 test/vmtest.py install --iso out/w3bs-os-0.1.0-x86_64.iso --disk out/test/crypt.qcow2 --encrypt --desktop none --out out/test'
```

Add `--device /dev/kvm` to `docker run` when the host has KVM; under TCG the Plasma install
takes about an hour.

## Known gaps (v0.1)

- Text installer only. The Calamares wizard of docs/os-prd.md §4 (previews, keyboard test
  field, install-alongside, manual partitioning, application selection) is not built.
- Network installer only: no offline package set, so no install without a wired network.
  Wi-Fi is not configured by the installer (run `setup-interfaces` on the live system first
  and the base install proceeds; the installer itself only lists wired interfaces).
- No Secure Boot (no Microsoft-signed shim for Alpine).
- Desktop profiles use Alpine's stock OpenRC setup-desktop, not the systemd variant the PRD
  plans for desktops; the W3BS desktop layer starts from XDG autostart, not a user unit.
- Only zsh is offered as the shell; bash/fish selection and shell completions are not wired.
- Encryption covers the root and swap (LVM on LUKS); `/boot` (or the EFI system partition)
  is not encrypted, and there is no recovery key or TPM unlock yet.
- x86_64 only; no Apple Silicon, T2 Mac or ARM images.
- The ISO's package index is signed with a per-build throwaway key; the official W3BS apk
  repository and signed W3BS apk packages (PRD §3) do not exist yet.
