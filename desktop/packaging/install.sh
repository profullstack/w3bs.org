#!/bin/sh
# Agentic Desktop Layer installer. https://w3bs.org/desktop
#
#   curl --proto '=https' --proto-redir '=https' -fsSL https://w3bs.org/desktop/install.sh | sh
#
# Inspect first if you like:  curl -fsSL https://w3bs.org/desktop/install.sh -o install.sh && less install.sh && sh install.sh
#
# What this does:
#   1. Downloads the w3bs-desktop release archive for this machine from GitHub releases and
#      checks it against the release's SHA256SUMS.
#   2. Installs the binary to ~/.local/bin (override with W3BS_BIN_DIR).
#   3. Linux: installs and starts the broker as a systemd user service, adds the native control
#      (tray on KDE and others, panel extension on GNOME) to your session.
#   It never enables agent access. The broker always starts Disabled; you enable it yourself
#   from the tray, the GNOME panel, or `w3bs-desktop control`.
#
# Environment: W3BS_DESKTOP_VERSION (default below), W3BS_BIN_DIR,
#              W3BS_DESKTOP_NO_SERVICE=1 (install the files only, start nothing).
#
# Nothing runs until the last line, so a truncated download does nothing.

adl_main() {
  set -u
  VERSION="${W3BS_DESKTOP_VERSION:-0.1.0}"
  BASE="https://github.com/profullstack/w3bs.org/releases/download/desktop-v$VERSION"

  say() { printf '%s\n' "$*" >&2; }
  die() { say "w3bs-desktop installer: $*"; exit "${2:-1}"; }

  case "$(uname -s)" in
    Linux) os=unknown-linux-gnu ;;
    Darwin) die "macOS is not in release $VERSION yet; follow https://w3bs.org/desktop for the macOS release" ;;
    *) die "unsupported system $(uname -s); on Windows use the PowerShell installer shown at https://w3bs.org/desktop once it is released" ;;
  esac
  case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) die "unsupported CPU $(uname -m); x86_64 and aarch64 are supported" ;;
  esac
  triple="$arch-$os"
  [ -n "${HOME:-}" ] || die "HOME is not set"
  bin="${W3BS_BIN_DIR:-$HOME/.local/bin}"
  share="${XDG_DATA_HOME:-$HOME/.local/share}/w3bs-desktop"

  if command -v curl >/dev/null 2>&1; then
    fetch() { curl --proto '=https' --proto-redir '=https' -fsSL "$1" -o "$2"; }
  elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -q --https-only -O "$2" "$1"; }
  else
    die "need curl or wget"
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    sha() { sha256sum "$1" | cut -d' ' -f1; }
  elif command -v shasum >/dev/null 2>&1; then
    sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
  else
    die "need sha256sum or shasum"
  fi

  tmp="$(mktemp -d)" || die "cannot create a temporary directory"
  trap 'rm -rf "$tmp"' EXIT INT TERM
  archive="w3bs-desktop-$VERSION-$triple.tar.gz"
  say "Downloading Agentic Desktop Layer $VERSION for $triple"
  fetch "$BASE/$archive" "$tmp/$archive" || die "download failed: $BASE/$archive"
  fetch "$BASE/SHA256SUMS" "$tmp/SHA256SUMS" || die "download failed: $BASE/SHA256SUMS"
  want="$(grep " $archive\$" "$tmp/SHA256SUMS" | cut -d' ' -f1)"
  [ -n "$want" ] || die "$archive is not listed in SHA256SUMS"
  got="$(sha "$tmp/$archive")"
  [ "$want" = "$got" ] || die "checksum mismatch for $archive (expected $want, got $got)"
  mkdir -p "$tmp/x" && tar -xzf "$tmp/$archive" -C "$tmp/x" || die "cannot unpack $archive"

  mkdir -p "$bin" "$share" || die "cannot create $bin"
  install -m 0755 "$tmp/x/bin/w3bs-desktop" "$bin/w3bs-desktop" 2>/dev/null || { cp "$tmp/x/bin/w3bs-desktop" "$bin/w3bs-desktop" && chmod 0755 "$bin/w3bs-desktop"; } || die "cannot install to $bin"
  cp -r "$tmp/x/share/." "$share/" 2>/dev/null || true
  cp "$tmp/x/LICENSE" "$tmp/x/README.md" "$share/" 2>/dev/null || true
  say "Installed $bin/w3bs-desktop ($("$bin/w3bs-desktop" --version 2>/dev/null || echo "version unknown"))"

  if [ "${W3BS_DESKTOP_NO_SERVICE:-0}" != 1 ]; then
    if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
      unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
      mkdir -p "$unit_dir"
      cat > "$unit_dir/w3bs-desktop.service" <<EOF
# Agentic Desktop Layer broker. Starting it never enables agent access: it always starts Disabled.
[Unit]
Description=Agentic Desktop Layer broker (starts Disabled)
Documentation=https://w3bs.org/desktop
PartOf=graphical-session.target
After=graphical-session.target

[Service]
ExecStart=$bin/w3bs-desktop daemon --adapter auto
Restart=on-failure
RestartSec=2
NoNewPrivileges=yes

[Install]
WantedBy=graphical-session.target
EOF
      systemctl --user daemon-reload && systemctl --user enable w3bs-desktop.service >/dev/null 2>&1 || say "note: could not enable the user service"
      if [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ]; then
        systemctl --user restart w3bs-desktop.service 2>/dev/null && say "Broker running as a user service (agent access: Disabled)"
      else
        say "Broker service installed; it starts with your next graphical login"
      fi
    else
      say "systemd user services are not available; start the broker with: $bin/w3bs-desktop daemon"
    fi

    case ":${XDG_CURRENT_DESKTOP:-}:" in
      *:GNOME:*)
        "$bin/w3bs-desktop" setup gnome-extension >&2 || say "note: GNOME extension install failed; run: w3bs-desktop setup gnome-extension"
        ;;
      *)
        autostart="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
        mkdir -p "$autostart"
        cat > "$autostart/w3bs-desktop-tray.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Agentic Desktop Layer
Comment=Your desktop. Your agents. Your switch.
Exec=$bin/w3bs-desktop tray
Icon=input-mouse
X-GNOME-Autostart-enabled=true
NoDisplay=true
EOF
        if [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ]; then
          (sleep 1; nohup "$bin/w3bs-desktop" tray >/dev/null 2>&1 &) && say "Tray control started (and added to your session's autostart)"
        fi
        ;;
    esac
  fi

  case ":$PATH:" in
    *":$bin:"*) ;;
    *) say "Add $bin to your PATH, e.g.:  echo 'export PATH=\"$bin:\$PATH\"' >> ~/.profile" ;;
  esac
  say ""
  say "Next:"
  say "  1. Register an agent:   w3bs-desktop agents add claude-code --preset broad"
  say "  2. Connect it:          w3bs-desktop setup claude-code --agent claude-code   (prints the claude mcp add line)"
  say "  3. Enable access from the tray or GNOME panel when you want the agent to work; disable it the same way."
  say "Docs: https://w3bs.org/desktop"
}

adl_main "$@"
