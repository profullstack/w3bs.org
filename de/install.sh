#!/bin/sh
# W3BS DE installer: a minimal, agent-friendly Wayland desktop built on Hyprland.
#
#   curl -fsSL https://w3bs.org/de/install.sh | sh
#   curl -fsSL https://w3bs.org/de/install.sh | sh -s -- --yes        # no questions
#
# What it does:
#   1. installs Hyprland and the small set of tools the desktop uses (bar, launcher,
#      notifications, lock, idle, wallpaper, terminal, screenshots), with your package manager;
#   2. installs the W3BS DE defaults to ~/.local/share/w3bs-de and the `w3bs-de` command; your own
#      settings live in ~/.config/w3bs-de and are never overwritten;
#   3. adds "W3BS DE" to your login screen;
#   4. installs the Agentic Desktop Layer (w3bs.org/desktop), so agents can use this desktop
#      when, and only when, you turn agent access on.
#
# Options:
#   --yes            do not ask before installing packages
#   --no-packages    only install or update the W3BS DE files (what `w3bs-de update` runs)
#   --no-agents      skip the Agentic Desktop Layer
#   --no-session     do not add the login-screen entry (needs sudo)
# Environment: W3BS_DE_BASE (default https://w3bs.org/de), W3BS_DE_SOURCE (a local de/share
# directory instead of downloading; for test rigs).
set -eu

BASE="${W3BS_DE_BASE:-https://w3bs.org/de}"
SHARE="${W3BS_DE_SHARE:-$HOME/.local/share/w3bs-de}"
BIN="$HOME/.local/bin"
YES=0 PACKAGES=1 AGENTS=1 SESSION=1
for a in "$@"; do
  case "$a" in
    --yes|-y) YES=1 ;;
    --no-packages) PACKAGES=0 ;;
    --no-agents) AGENTS=0 ;;
    --no-session) SESSION=0 ;;
    -h|--help) sed -n '2,24p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "w3bs-de install: unknown option $a" >&2; exit 2 ;;
  esac
done

say() { printf 'w3bs-de: %s\n' "$*"; }
die() { printf 'w3bs-de: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

[ "$(uname -s)" = Linux ] || die "W3BS DE is a Linux desktop (Wayland). On macOS and Windows use the Agentic Desktop Layer: https://w3bs.org/desktop"
[ "$(id -u)" -ne 0 ] || die "run this as your own user, not root (it asks for sudo when it needs it)"

SUDO=""
if [ "$PACKAGES" = 1 ] || [ "$SESSION" = 1 ]; then
  if have sudo; then SUDO=sudo; elif have doas; then SUDO=doas; fi
fi

confirm() {
  [ "$YES" = 1 ] && return 0
  [ -r /dev/tty ] || { say "no terminal to ask on; re-run with --yes"; return 1; }
  printf 'w3bs-de: %s [Y/n] ' "$1" >/dev/tty
  read -r ans </dev/tty || return 1
  case "$ans" in ""|y|Y|yes) return 0 ;; *) return 1 ;; esac
}

# ---------------------------------------------------------------- packages

. /etc/os-release 2>/dev/null || true
ids=" ${ID:-} ${ID_LIKE:-} "
case "$ids" in
  *" nixos "*) family=nixos ;;
  *" arch "*|*" archlinux "*|*" manjaro "*|*" endeavouros "*|*" cachyos "*) family=arch ;;
  *" fedora "*) family=fedora ;;
  *" alpine "*|*" postmarketos "*) family=alpine ;;
  *" opensuse"*|*" suse "*) family=suse ;;
  *" debian "*|*" ubuntu "*) family=debian ;;
  *) family=unknown ;;
esac

common="xdg-desktop-portal-hyprland hyprlock hypridle waybar fuzzel swaybg foot grim slurp wl-clipboard brightnessctl wireplumber pavucontrol"
case "$family" in
  arch) pkgs="hyprland $common hyprpolkitagent xdg-desktop-portal-gtk swaync libnotify noto-fonts ttf-dejavu" ;;
  fedora) pkgs="hyprland $common hyprpolkitagent xdg-desktop-portal-gtk SwayNotificationCenter libnotify google-noto-sans-fonts dejavu-sans-fonts" ;;
  debian) pkgs="hyprland $common hyprpolkitagent xdg-desktop-portal-gtk sway-notification-center libnotify-bin fonts-noto-core fonts-dejavu-core" ;;
  alpine) pkgs="hyprland $common xdg-desktop-portal-gtk swaync libnotify font-noto font-dejavu dbus seatd" ;;
  suse) pkgs="hyprland $common xdg-desktop-portal-gtk SwayNotificationCenter libnotify-tools noto-sans-fonts dejavu-fonts" ;;
  *) pkgs="" ;;
esac

available() {
  case "$family" in
    debian) apt-cache show "$1" >/dev/null 2>&1 ;;
    fedora) dnf -q info "$1" >/dev/null 2>&1 ;;
    alpine) [ -n "$(apk search -x "$1" 2>/dev/null)" ] ;;
    suse) zypper -q info "$1" 2>/dev/null | grep -q "^Name" ;;
    *) return 0 ;;
  esac
}

install_packages() {
  case "$family" in
    nixos)
      say "NixOS: add this to configuration.nix and rebuild, then re-run with --no-packages:"
      echo '  programs.hyprland.enable = true;'
      echo '  environment.systemPackages = with pkgs; [ hyprlock hypridle waybar fuzzel swaynotificationcenter swaybg foot grim slurp wl-clipboard brightnessctl pavucontrol hyprpolkitagent libnotify ];'
      return 0 ;;
    unknown) say "unknown distribution (${ID:-?}); install Hyprland and these yourself, then re-run with --no-packages: $common swaync"; return 0 ;;
  esac
  [ -n "$SUDO" ] || die "installing packages needs sudo or doas (or re-run with --no-packages)"
  if [ "$family" = fedora ] && ! dnf -q repolist 2>/dev/null | grep -qi hyprland; then
    say "Fedora ships Hyprland through the COPR lionheartp/Hyprland (the one the Hyprland wiki lists)"
    confirm "enable COPR lionheartp/Hyprland?" || die "Hyprland is needed"
    $SUDO dnf -y copr enable lionheartp/Hyprland
  fi
  [ "$family" = debian ] && $SUDO apt-get update -qq
  want="" skip=""
  for p in $pkgs; do
    if available "$p"; then want="$want $p"; else skip="$skip $p"; fi
  done
  case " $want " in
    *" hyprland "*) ;;
    *) case "$family" in
         debian) die "this release has no hyprland package. Debian: use trixie-backports (0.55+), forky or sid; Ubuntu: 26.10 or later. https://wiki.hypr.land/Getting-Started/Installation/" ;;
         alpine) die "enable the community repository (edge has Hyprland): https://wiki.alpinelinux.org/wiki/Repositories" ;;
         *) die "no hyprland package found" ;;
       esac ;;
  esac
  [ -z "$skip" ] || say "not available here, skipped:$skip"
  say "packages:$want"
  confirm "install them with $SUDO?" || die "cancelled"
  case "$family" in
    arch) $SUDO pacman -S --needed --noconfirm $want ;;
    fedora) $SUDO dnf -y install $want ;;
    debian) $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends $want ;;
    alpine) $SUDO apk add $want ;;
    suse) $SUDO zypper -n install $want ;;
  esac
}

[ "$PACKAGES" = 0 ] || install_packages

# ---------------------------------------------------------------- the W3BS DE files

sha256() { if have sha256sum; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }
fetch() { curl -fsSL --retry 2 "$1" -o "$2"; }

stage="$SHARE.new.$$"
rm -rf "$stage"
mkdir -p "$stage"
trap 'rm -rf "$stage"' EXIT
if [ -n "${W3BS_DE_SOURCE:-}" ]; then
  cp -R "$W3BS_DE_SOURCE/." "$stage/"
  version="$(cat "$W3BS_DE_SOURCE/VERSION" 2>/dev/null || echo dev)"
else
  have curl || die "curl is needed"
  fetch "$BASE/manifest.txt" "$stage/.manifest" || die "cannot download $BASE/manifest.txt"
  version="$(sed -n 's/^# version //p' "$stage/.manifest")"
  grep -v '^#' "$stage/.manifest" | while read -r sum path; do
    case "$path" in ""|/*|*..*) die "bad manifest path: $path" ;; esac
    mkdir -p "$stage/$(dirname "$path")"
    fetch "$BASE/files/$path" "$stage/$path" || die "download failed: $path"
    [ "$(sha256 "$stage/$path")" = "$sum" ] || die "checksum mismatch: $path"
  done
  rm -f "$stage/.manifest"
fi
# Paths and the version are filled in here, so the configs need no environment to find them.
for f in "$stage"/default/hypr/*.conf "$stage/bin/w3bs-de"; do
  [ -f "$f" ] && sed -i "s|@W3BS_DE_SHARE@|$SHARE|g; s|@W3BS_DE_VERSION@|$version|g" "$f"
done
chmod 755 "$stage/bin/w3bs-de"
rm -rf "$SHARE.old"
[ -d "$SHARE" ] && mv "$SHARE" "$SHARE.old"
mv "$stage" "$SHARE"
rm -rf "$SHARE.old"
trap - EXIT
mkdir -p "$BIN"
ln -sf "$SHARE/bin/w3bs-de" "$BIN/w3bs-de"
say "installed W3BS DE $version to $SHARE (your settings: ${XDG_CONFIG_HOME:-$HOME/.config}/w3bs-de)"

# ---------------------------------------------------------------- login screen

if [ "$SESSION" = 1 ] && [ "$PACKAGES" = 1 ] && [ -n "$SUDO" ] && [ ! -f /usr/share/wayland-sessions/w3bs-de.desktop ]; then
  if confirm "add \"W3BS DE\" to the login screen (/usr/share/wayland-sessions)?"; then
    # Each user's own copy of W3BS DE runs; this wrapper only finds it.
    printf '#!/bin/sh\nexec "${W3BS_DE_SHARE:-$HOME/.local/share/w3bs-de}/bin/w3bs-de" "$@"\n' | $SUDO tee /usr/local/bin/w3bs-de >/dev/null
    $SUDO chmod 755 /usr/local/bin/w3bs-de
    $SUDO mkdir -p /usr/share/wayland-sessions
    printf '[Desktop Entry]\nName=W3BS DE\nComment=Minimal, agent-friendly Wayland desktop built on Hyprland (w3bs.org/de)\nExec=/usr/local/bin/w3bs-de session\nType=Application\nDesktopNames=Hyprland\n' | $SUDO tee /usr/share/wayland-sessions/w3bs-de.desktop >/dev/null
    say "added \"W3BS DE\" to the login screen"
  fi
fi

# ---------------------------------------------------------------- agents

if [ "$AGENTS" = 1 ]; then
  need=1
  if have w3bs-desktop || [ -x "$BIN/w3bs-desktop" ]; then
    v="$("$(command -v w3bs-desktop || echo "$BIN/w3bs-desktop")" --version 2>/dev/null | sed 's/^w3bs desktop //')"
    case "$v" in 0.[0-7].*|"") need=1 ;; *) need=0 ;; esac
  fi
  if [ "$need" = 1 ]; then
    say "installing the Agentic Desktop Layer (Hyprland support needs 0.8 or later)"
    curl -fsSL https://w3bs.org/desktop/install.sh | sh || say "the Agentic Desktop Layer did not install; retry: curl -fsSL https://w3bs.org/desktop/install.sh | sh"
  fi
fi

cat <<EOF

W3BS DE is installed.
  Log in:   pick "W3BS DE" on your login screen, or run  w3bs-de session  from a text console.
  Check:    w3bs-de doctor
  Keys:     SUPER+SPACE launcher, SUPER+RETURN terminal, SUPER+A agents, SUPER+ALT+ESC agents off
  Settings: ${XDG_CONFIG_HOME:-$HOME/.config}/w3bs-de/hypr/user.lua (kept across updates)
  Agents:   access starts OFF; turn it on from the bar when you want an agent to use this desktop.
EOF
