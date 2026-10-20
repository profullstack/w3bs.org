#!/bin/sh
# Agentic Desktop Layer installer for Linux and macOS. https://w3bs.org/desktop
#
#   curl --proto '=https' --proto-redir '=https' -fsSL https://w3bs.org/desktop/install.sh | sh
#
# Inspect first if you like:  curl -fsSL https://w3bs.org/desktop/install.sh -o install.sh && less install.sh && sh install.sh
# On Windows use PowerShell instead:  irm https://w3bs.org/desktop/install.ps1 | iex
#
# What this does:
#   1. Downloads the w3bs-desktop release archive for this machine from GitHub releases and
#      checks it against the release's SHA256SUMS.
#   2. Installs the binary to ~/.local/bin (override with W3BS_BIN_DIR).
#   3. Linux: installs and starts the broker as a systemd user service, adds the native control
#      (tray on KDE and others, panel extension on GNOME) to your session.
#      macOS: installs LaunchAgents for the broker (org.w3bs.desktop.broker, restarted if it
#      crashes) and the menu-bar control (org.w3bs.desktop.menubar, started at login; Quit
#      keeps it quit), and starts both.
#   It never enables agent access. The broker always starts Disabled; you enable it yourself
#   from the tray, the menu bar, the GNOME panel, or `w3bs desktop control`.
#
# Environment: W3BS_DESKTOP_VERSION (default below), W3BS_BIN_DIR,
#              W3BS_DESKTOP_NO_SERVICE=1 (install the files only, start nothing),
#              W3BS_LIBC=gnu|musl (override the detected C library; Alpine and postmarketOS
#              get the static musl build),
#              W3BS_DESKTOP_ARCHIVE=PATH with W3BS_DESKTOP_SHA256=HEX (install that local
#              archive instead of downloading, still checksum-verified; for testing).
# Uninstall (macOS):  curl -fsSL https://w3bs.org/desktop/install.sh | sh -s -- --uninstall
#
# Nothing runs until the last line, so a truncated download does nothing.

adl_main() {
  set -u
  VERSION="${W3BS_DESKTOP_VERSION:-0.7.0}"
  BASE="https://github.com/profullstack/w3bs.org/releases/download/desktop-v$VERSION"

  say() { printf '%s\n' "$*" >&2; }
  die() { say "w3bs-desktop installer: $1"; exit "${2:-1}"; }

  uninstall=0
  for a in "$@"; do
    case "$a" in
      --uninstall) uninstall=1 ;;
      *) die "unknown option $a (the only option is --uninstall)" 2 ;;
    esac
  done

  # musl (Alpine, postmarketOS) or glibc. ldd names its C library; without ldd, a glibc libc.so.6
  # wins over a musl loader (Debian's `musl` package adds one; Alpine's gcompat fakes the glibc
  # loader, not libc.so.6). W3BS_LIBC=gnu|musl overrides.
  linux_libc() {
    case "${W3BS_LIBC:-}" in gnu | musl) echo "$W3BS_LIBC"; return ;; esac
    v="$(ldd --version 2>&1 || true)"
    case "$v" in
      *musl*) echo musl; return ;;
      *GNU* | *GLIBC* | *glibc*) echo gnu; return ;;
    esac
    for f in /lib/*-linux-gnu/libc.so.6 /lib64/libc.so.6 /usr/lib64/libc.so.6 /usr/lib/libc.so.6; do
      [ -e "$f" ] && { echo gnu; return; }
    done
    for f in /lib/ld-musl-*.so.1; do
      [ -e "$f" ] && { echo musl; return; }
    done
    echo gnu
  }

  case "$(uname -s)" in
    Linux) os="unknown-linux-$(linux_libc)" ;;
    Darwin) os=apple-darwin ;;
    *) die "unsupported system $(uname -s); on Windows run in PowerShell:  irm https://w3bs.org/desktop/install.ps1 | iex" ;;
  esac
  case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) die "unsupported CPU $(uname -m); x86_64 and aarch64 are supported" ;;
  esac
  # An x86_64 shell under Rosetta on Apple silicon still gets the native build.
  if [ "$os" = apple-darwin ] && [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = 1 ]; then
    arch=aarch64
  fi
  triple="$arch-$os"
  [ -n "${HOME:-}" ] || die "HOME is not set"
  bin="${W3BS_BIN_DIR:-$HOME/.local/bin}"
  share="${XDG_DATA_HOME:-$HOME/.local/share}/w3bs-desktop"
  agents="$HOME/Library/LaunchAgents"
  logs="$HOME/Library/Logs/w3bs-desktop"
  uid="$(id -u)"

  # macOS LaunchAgents live in the user's GUI domain.
  la_unload() {
    launchctl bootout "gui/$uid/$1" >/dev/null 2>&1 || true
    # bootout returns before launchd has finished stopping the job; wait (up to 10 s) so callers
    # see it gone.
    n=0
    while launchctl print "gui/$uid/$1" >/dev/null 2>&1 && [ "$n" -lt 50 ]; do sleep 0.2; n=$((n + 1)); done
  }
  la_load() {
    # bootout completes asynchronously and a bootstrap right after it can fail, so retry.
    for _ in 1 2 3 4 5 6 7 8 9; do
      launchctl bootstrap "gui/$uid" "$agents/$1.plist" 2>/dev/null && return 0
      sleep 1
    done
    launchctl bootstrap "gui/$uid" "$agents/$1.plist"
  }

  if [ "$uninstall" = 1 ]; then
    [ "$os" = apple-darwin ] || die "--uninstall is macOS-only; on Linux run: systemctl --user disable --now w3bs-desktop.service; rm -f ~/.config/systemd/user/w3bs-desktop.service ~/.config/autostart/w3bs-desktop-tray.desktop $bin/w3bs-desktop; rm -rf $share"
    la_unload org.w3bs.desktop.menubar
    la_unload org.w3bs.desktop.broker
    rm -f "$agents/org.w3bs.desktop.menubar.plist" "$agents/org.w3bs.desktop.broker.plist" "$bin/w3bs-desktop"
    rm -rf "$share" "$logs"
    say "Agentic Desktop Layer removed (LaunchAgents, $bin/w3bs-desktop, $share)."
    say "Kept your agents and settings in ~/.config/w3bs/desktop; delete that folder to remove them too."
    say "macOS keeps the Accessibility and Screen Recording entries; remove them in System Settings > Privacy & Security."
    exit 0
  fi

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
  if [ -n "${W3BS_DESKTOP_ARCHIVE:-}" ]; then
    # Testing: a local archive, checked against the checksum given with it.
    want="${W3BS_DESKTOP_SHA256:-}"
    [ -n "$want" ] || die "W3BS_DESKTOP_ARCHIVE needs W3BS_DESKTOP_SHA256 (an archive is never installed unchecked)"
    say "Installing Agentic Desktop Layer for $triple from $W3BS_DESKTOP_ARCHIVE"
    cp "$W3BS_DESKTOP_ARCHIVE" "$tmp/$archive" || die "cannot read $W3BS_DESKTOP_ARCHIVE"
  else
    say "Downloading Agentic Desktop Layer $VERSION for $triple"
    if ! fetch "$BASE/$archive" "$tmp/$archive"; then
      case "$triple" in
        *-musl) die "download failed: $BASE/$archive (desktop-v$VERSION may predate musl builds; set W3BS_DESKTOP_VERSION to a newer release)" ;;
        *) die "download failed: $BASE/$archive" ;;
      esac
    fi
    fetch "$BASE/SHA256SUMS" "$tmp/SHA256SUMS" || die "download failed: $BASE/SHA256SUMS"
    want="$(grep " $archive\$" "$tmp/SHA256SUMS" | cut -d' ' -f1)"
    [ -n "$want" ] || die "$archive is not listed in SHA256SUMS"
  fi
  got="$(sha "$tmp/$archive")"
  [ "$want" = "$got" ] || die "checksum mismatch for $archive (expected $want, got $got)"
  mkdir -p "$tmp/x" && tar -xzf "$tmp/$archive" -C "$tmp/x" || die "cannot unpack $archive"

  # Installing restarts the broker, and a recording lives in the broker's memory: refuse while one
  # is running or waiting for W3BS Training, before anything is replaced.
  if [ -x "$bin/w3bs-desktop" ] && [ "${W3BS_DESKTOP_FORCE_RESTART:-0}" != 1 ]; then
    st="$("$bin/w3bs-desktop" --json training status 2>/dev/null | tr -d ' \n' || true)"
    case "$st" in
      *'"state":"recording"'* | *'"state":"paused"'* | *'"state":"blocked"'* | *'"state":"arming"'*)
        die "a Training recording is in progress. Finish it (tray > Finish task) and run the update again, or set W3BS_DESKTOP_FORCE_RESTART=1 to discard it." ;;
    esac
    case "$st" in
      *'"pending_handoffs":[{'*)
        die "a finished recording is still waiting for W3BS Training and would be lost. Hand it over first (control app > Training > Try again, or: w3bs desktop training handoff), or set W3BS_DESKTOP_FORCE_RESTART=1 to discard it." ;;
    esac
  fi

  mkdir -p "$bin" "$share" || die "cannot create $bin"
  # Replace rather than overwrite in place: a running broker keeps its old (still valid) image,
  # and on macOS the code signature cache never sees a modified file.
  rm -f "$bin/w3bs-desktop"
  install -m 0755 "$tmp/x/bin/w3bs-desktop" "$bin/w3bs-desktop" 2>/dev/null || { cp "$tmp/x/bin/w3bs-desktop" "$bin/w3bs-desktop" && chmod 0755 "$bin/w3bs-desktop"; } || die "cannot install to $bin"
  cp -r "$tmp/x/share/." "$share/" 2>/dev/null || true
  cp "$tmp/x/LICENSE" "$tmp/x/README.md" "$share/" 2>/dev/null || true
  # Hints use the w3bs front door when the w3bs CLI is installed, else the binary itself.
  if command -v w3bs >/dev/null 2>&1; then cmd="w3bs desktop"; else cmd="w3bs-desktop"; fi
  say "Installed $bin/w3bs-desktop ($("$bin/w3bs-desktop" --version 2>/dev/null || echo "version unknown"))"

  if [ "${W3BS_DESKTOP_NO_SERVICE:-0}" != 1 ] && [ "$os" = apple-darwin ]; then
    mkdir -p "$agents" "$logs" || die "cannot create $agents"
    # The broker: started at login and now, restarted if it crashes (not after a clean exit).
    # launchd runs the binary itself, so it is w3bs-desktop (not your terminal) that macOS asks
    # about and lists under Accessibility and Screen Recording.
    cat > "$agents/org.w3bs.desktop.broker.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Agentic Desktop Layer broker. Starting it never enables agent access: it always starts Disabled. https://w3bs.org/desktop -->
<plist version="1.0">
<dict>
  <key>Label</key><string>org.w3bs.desktop.broker</string>
  <key>ProgramArguments</key>
  <array><string>$bin/w3bs-desktop</string><string>daemon</string><string>--adapter</string><string>auto</string><string>--mcp-http</string><string>127.0.0.1:7801</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ThrottleInterval</key><integer>5</integer>
  <key>LimitLoadToSessionType</key><string>Aqua</string>
  <key>ProcessType</key><string>Interactive</string>
  <key>StandardOutPath</key><string>$logs/broker.log</string>
  <key>StandardErrorPath</key><string>$logs/broker.log</string>
</dict>
</plist>
EOF
    # The menu-bar control (the switch): started at login and now, never restarted, so Quit
    # stays quit. Quitting it disables agent access.
    cat > "$agents/org.w3bs.desktop.menubar.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Agentic Desktop Layer menu-bar control. Quitting it disables agent access. https://w3bs.org/desktop -->
<plist version="1.0">
<dict>
  <key>Label</key><string>org.w3bs.desktop.menubar</string>
  <key>ProgramArguments</key>
  <array><string>$bin/w3bs-desktop</string><string>tray</string><string>--wait-for-broker</string><string>60</string></array>
  <key>RunAtLoad</key><true/>
  <key>LimitLoadToSessionType</key><string>Aqua</string>
  <key>ProcessType</key><string>Interactive</string>
  <key>StandardOutPath</key><string>$logs/menubar.log</string>
  <key>StandardErrorPath</key><string>$logs/menubar.log</string>
</dict>
</plist>
EOF
    if launchctl print "gui/$uid" >/dev/null 2>&1; then
      # Re-install: unload the old jobs (menu bar first), then load the broker first.
      la_unload org.w3bs.desktop.menubar
      la_unload org.w3bs.desktop.broker
      if la_load org.w3bs.desktop.broker; then
        say "Broker running as a LaunchAgent (agent access: Disabled); log: $logs/broker.log"
      else
        say "note: could not load the broker LaunchAgent; it starts at your next login"
      fi
      if la_load org.w3bs.desktop.menubar; then
        say "Menu-bar control started (it reads Agents: Disabled)"
      else
        say "note: could not load the menu-bar LaunchAgent; it starts at your next login"
      fi
    else
      say "No macOS login session here; the broker and menu-bar control start at your next login"
    fi
  elif [ "${W3BS_DESKTOP_NO_SERVICE:-0}" != 1 ]; then
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
ExecStart=$bin/w3bs-desktop daemon --adapter auto --mcp-http 127.0.0.1:7801
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
      # No systemd user manager (Alpine and postmarketOS use OpenRC): start the broker with the
      # graphical session through XDG autostart instead. It still starts Disabled.
      autostart="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
      mkdir -p "$autostart"
      cat > "$autostart/w3bs-desktop-broker.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Agentic Desktop Layer broker
Comment=Starts Disabled; enable agent access from the tray. https://w3bs.org/desktop
Exec=$bin/w3bs-desktop daemon --adapter auto --mcp-http 127.0.0.1:7801
Icon=input-mouse
X-GNOME-Autostart-enabled=true
NoDisplay=true
EOF
      say "systemd user services are not available; the broker starts with your next graphical login"
      say "  ($autostart/w3bs-desktop-broker.desktop). To start it now:  $bin/w3bs-desktop daemon &"
    fi

    case ":${XDG_CURRENT_DESKTOP:-}:" in
      *:GNOME:*)
        "$bin/w3bs-desktop" setup gnome-extension >&2 || say "note: GNOME extension install failed; run: $cmd setup gnome-extension"
        ;;
      *)
        autostart="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
        mkdir -p "$autostart"
        cat > "$autostart/w3bs-desktop-tray.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Agentic Desktop Layer
Comment=Your desktop. Your agents. Your switch.
Exec=$bin/w3bs-desktop tray --wait-for-broker 60
Icon=input-mouse
X-GNOME-Autostart-enabled=true
NoDisplay=true
EOF
        if [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ]; then
          # Through the user manager when it can: it holds the desktop session's environment, so
          # an update run over ssh (or `w3bs update` from a tty) still gets a tray that can open
          # the browser. A tray started from this shell would inherit the shell's environment.
          if systemctl --user show-environment 2>/dev/null | grep -qE '^(WAYLAND_)?DISPLAY='; then
            systemd-run --user --collect --quiet --unit="w3bs-desktop-tray-$$" "$bin/w3bs-desktop" tray >/dev/null 2>&1
          else
            (sleep 1; nohup "$bin/w3bs-desktop" tray >/dev/null 2>&1 &)
          fi && say "Tray control started (and added to your session's autostart)"
        fi
        ;;
    esac
  fi

  # Alpine and postmarketOS: name the session packages the Linux backend talks to (portal,
  # PipeWire, D-Bus) that are missing. Printed, never installed: system packages are yours.
  if [ "$os" != apple-darwin ] && command -v apk >/dev/null 2>&1; then
    case ":${XDG_CURRENT_DESKTOP:-}:" in
      *:GNOME:*) want_pkgs="xdg-desktop-portal xdg-desktop-portal-gnome pipewire wireplumber dbus" ;;
      *) want_pkgs="xdg-desktop-portal xdg-desktop-portal-kde pipewire wireplumber dbus" ;;
    esac
    missing=""
    for p in $want_pkgs; do
      apk info -e "$p" >/dev/null 2>&1 || missing="$missing $p"
    done
    if [ -n "$missing" ]; then
      say ""
      say "Alpine / postmarketOS: the desktop layer needs your session's portal and PipeWire. Install them with:"
      say "  doas apk add$missing"
      say "  (or sudo; on GNOME the portal is xdg-desktop-portal-gnome, on Plasma xdg-desktop-portal-kde)"
    fi
  fi

  # shellcheck disable=SC2088 # shown to the user, not expanded
  if [ "$os" = apple-darwin ]; then profile="~/.zprofile"; else profile="~/.profile"; fi
  case ":$PATH:" in
    *":$bin:"*) ;;
    *) say "Add $bin to your PATH, e.g.:  echo 'export PATH=\"$bin:\$PATH\"' >> $profile" ;;
  esac
  if [ "$os" = apple-darwin ]; then
    say ""
    say "macOS permissions (System Settings > Privacy & Security), for $bin/w3bs-desktop:"
    say "  - Accessibility: input, focus and inspect. Enabling is refused (Blocked) without it."
    say "  - Screen Recording (\"Screen & System Audio Recording\" on macOS 15+): screenshots and window titles."
    say "  Input Monitoring is not needed. The broker LaunchAgent is the process macOS asks about."
    say "  Run  $cmd doctor --prompt  to raise the system prompts (or add w3bs-desktop with +,"
    say "  Shift+Command+G, $bin), then restart the broker so it picks them up:"
    say "    launchctl kickstart -k gui/$uid/org.w3bs.desktop.broker"
    say "  The binary is not code-signed, so after an upgrade macOS may ask again."
  fi
  say ""
  say "Next:"
  say "  1. Register an agent:   $cmd agents add claude-code --preset broad"
  say "  2. Connect it:          $cmd setup claude-code --agent claude-code --http   (prints the claude mcp add line)"
  say "     OpenCode instead:    $cmd agents add opencode --preset broad && $cmd setup opencode --agent opencode --http   (prints the opencode.json mcp block)"
  if [ "$os" = apple-darwin ]; then
    say "  3. Enable access from the menu bar (Agents: Disabled) when you want the agent to work; disable it the same way."
    say "     Emergency stop: Control+Option+Command+Escape."
  else
    say "  3. Enable access from the tray or GNOME panel when you want the agent to work; disable it the same way."
  fi
  if ! command -v w3bs-training >/dev/null 2>&1 && [ ! -x "$bin/w3bs-training" ]; then
    say "  Teach agents by example: tray > Start training… records a task. To keep and review"
    say "  recordings, install W3BS Training:  curl -fsSL https://w3bs.org/training/install.sh | sh"
  fi
  say "Docs: https://w3bs.org/desktop"
}

adl_main "$@"
