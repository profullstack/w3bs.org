#!/bin/sh
# The headless Kubuntu session inside the w3bs desktop container (see Dockerfile).
# Runs as the host user (docker run --user), with $HOME on a host volume.
#   W3BS_DRM       the KMS device KWin drives (default: the vkms card passed in)
#   W3BS_GEOMETRY  1920x1080 by default
#   W3BS_MCP       127.0.0.1:7801 by default (the desktop layer's MCP endpoint)
set -eu
GEOMETRY=${W3BS_GEOMETRY:-1920x1080}
export XDG_RUNTIME_DIR=/tmp/runtime-$(id -u)
export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus
export XDG_CURRENT_DESKTOP=KDE XDG_SESSION_TYPE=wayland XDG_SESSION_DESKTOP=KDE KDE_FULL_SESSION=true
export XDG_CONFIG_HOME=$HOME/.config XDG_DATA_HOME=$HOME/.local/share
# The host user has no passwd entry in the image, so name the shell Konsole should start.
export SHELL=/bin/bash
LOGS=$HOME/.local/state/w3bs-desktop/logs
mkdir -p -m 0700 "$XDG_RUNTIME_DIR" && mkdir -p "$LOGS" "$XDG_CONFIG_HOME/xdg-desktop-portal"
printf '[preferred]\ndefault=kde;gtk\n' > "$XDG_CONFIG_HOME/xdg-desktop-portal/kde-portals.conf"
# The KDE screenshot portal writes the image under Pictures and answers "response 2" when the
# folder does not exist; a fresh home has none.
mkdir -p "$HOME/Pictures"
# Never lock: nobody is at this screen to unlock it, and a locked session makes the desktop
# layer disable agent access ("session locked").
printf '[Daemon]\nAutolock=false\nLockOnResume=false\nTimeout=0\n' > "$XDG_CONFIG_HOME/kscreenlockerrc"
rm -f /vnc/vnc.sock

say() { echo "w3bs-kubuntu: $*"; }
wait_for() { for _ in $(seq 1 100); do [ -e "$1" ] && return 0; sleep 0.1; done; say "timed out waiting for $1"; exit 1; }

# The display: the vkms card the host passed in (`w3bs fleet desktop start` finds it).
DRM=${W3BS_DRM:-}
if [ -z "$DRM" ]; then
  for card in /dev/dri/card*; do
    [ "$(basename "$(readlink -f "/sys/class/drm/$(basename "$card")/device")")" = vkms ] && DRM=$card && break
  done
fi
[ -n "$DRM" ] || { say "no vkms display passed in; on the host run: sudo modprobe vkms"; exit 2; }

dbus-daemon --session --address="$DBUS_SESSION_BUS_ADDRESS" --nofork --nopidfile >"$LOGS/dbus.log" 2>&1 &
wait_for "$XDG_RUNTIME_DIR/bus"
pipewire >"$LOGS/pipewire.log" 2>&1 &
wireplumber >"$LOGS/wireplumber.log" 2>&1 &

# KWin hands its screencast and fake-input protocols only to clients it trusts by their desktop
# file. Ubuntu's KWin 6.6 does not trust the KDE portal inside this container (it does on
# Fedora's 6.7), so the portal, and with it w0vncserver and the desktop layer, would get no
# screen. Everything in this compositor is part of one agent desktop that the desktop layer
# already lets agents see and drive, so turn the check off here; it never touches the host.
export KWIN_WAYLAND_NO_PERMISSION_CHECKS=1
# Screenshots are a separate KWin plugin with its own check (ScreenShot2 answers
# NoAuthorized to the KDE portal here), so the desktop layer's screen capture needs it too.
export KWIN_SCREENSHOT_NO_PERMISSION_CHECKS=1
kbuildsycoca6 >"$LOGS/kbuildsycoca.log" 2>&1 || true

# KWin on the virtual display; with no GPU, Mesa renders OpenGL on the CPU.
KWIN_DRM_DEVICES=$DRM kwin_wayland --drm --socket wayland-0 >"$LOGS/kwin.log" 2>&1 &
KWIN=$!
wait_for "$XDG_RUNTIME_DIR/wayland-0"
export WAYLAND_DISPLAY=wayland-0 QT_QPA_PLATFORM=wayland
# vkms starts at 1024x768.
for _ in $(seq 1 20); do kscreen-doctor -o >/dev/null 2>&1 && break; sleep 0.5; done
OUTPUT=$(kscreen-doctor -j 2>/dev/null | sed -n 's/.*"name": *"\([^"]*\)".*/\1/p' | head -1)
[ -n "$OUTPUT" ] && kscreen-doctor "output.$OUTPUT.mode.$GEOMETRY@60" >"$LOGS/kscreen.log" 2>&1 \
  || say "could not set $GEOMETRY on ${OUTPUT:-the display}; see $LOGS/kscreen.log"

/usr/lib/*/libexec/xdg-desktop-portal-kde >"$LOGS/xdg-desktop-portal-kde.log" 2>&1 &
sleep 1
/usr/libexec/xdg-desktop-portal -r >"$LOGS/xdg-desktop-portal.log" 2>&1 &
sleep 1
# Nobody can click a consent dialog on a headless desktop: grant remote control and screenshots
# up front, the way KDE intends for unattended machines. w0vncserver and the desktop layer each
# open their own portal session against this grant.
flatpak permission-set kde-authorized remote-desktop "" yes
flatpak permission-set screenshot screenshot "" yes

plasmashell >"$LOGS/plasmashell.log" 2>&1 &
konsole >"$LOGS/konsole.log" 2>&1 &

# The VNC server: a Unix socket only, owner-only (0600). No password (the socket is the lock)
# unless `w3bs fleet desktop password` set one; then the w3bs.org page asks for it.
PASSWD=$HOME/.config/tigervnc/passwd
if [ -s "$PASSWD" ]; then SECURITY="-SecurityTypes VncAuth -PasswordFile $PASSWD"; else SECURITY="-SecurityTypes None"; fi
# No blacklist: on a Unix socket every client has the same "address", so five connections that
# never logged in (a probe, a tab closed at the password prompt) locked every viewer out, for a
# timeout that doubles each try. The socket is owner-only; there is nobody else to keep out.
# shellcheck disable=SC2086
w0vncserver -rfbport -1 -rfbunixpath /vnc/vnc.sock $SECURITY -AlwaysShared -UseBlacklist=0 \
  -desktop "w3bs desktop" >"$LOGS/w0vncserver.log" 2>&1 &
W0=$!

# The Agentic Desktop Layer, Enabled from the start: this desktop exists for agents.
w3bs-desktop daemon --adapter auto --enabled-by-default --mcp-http "${W3BS_MCP:-127.0.0.1:7801}" \
  >"$LOGS/w3bs-desktop.log" 2>&1 &
say "up: KWin on $DRM, VNC on /vnc/vnc.sock, desktop layer MCP on http://${W3BS_MCP:-127.0.0.1:7801}/mcp"

# Stay up while the compositor and the VNC server live (docker restarts the container
# otherwise); tini reaps everything else, including w0vncserver's per-viewer children.
while kill -0 "$KWIN" 2>/dev/null && kill -0 "$W0" 2>/dev/null; do sleep 5; done
say "KWin or w0vncserver exited; see $LOGS"
exit 1
