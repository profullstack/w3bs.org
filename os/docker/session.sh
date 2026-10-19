#!/bin/sh
# The W3BS OS desktop session (os/docker/Dockerfile, target desktop): KDE Plasma on a virtual
# display, the Agentic Desktop Layer, and a VNC server. Ported from the Kubuntu fleet desktop
# (desktop/packaging/kubuntu/session.sh); the same environment works for both.
#   W3BS_DRM               the KMS device KWin drives (default: a vkms card passed in)
#   W3BS_GEOMETRY          1920x1080 by default
#   W3BS_MCP               127.0.0.1:7801 by default (the desktop layer's MCP endpoint)
#   W3BS_DESKTOP_ENABLED   1 lets agents observe and drive from the start; default off, as on
#                          any W3BS machine (docs/os-prd.md §10, §20)
#   W3BS_VNC_PASSWORD      also serve VNC on TCP 5900 with this password (6 to 8 characters);
#                          without it VNC is only the owner-only Unix socket /vnc/vnc.sock
set -eu
GEOMETRY=${W3BS_GEOMETRY:-1920x1080}
# The image sets XDG_RUNTIME_DIR, so `docker exec ... w3bs-desktop status` finds the broker.
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/tmp/runtime-$(id -u)}
export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus
export XDG_CURRENT_DESKTOP=KDE XDG_SESSION_TYPE=wayland XDG_SESSION_DESKTOP=KDE KDE_FULL_SESSION=true
export XDG_CONFIG_HOME=$HOME/.config XDG_DATA_HOME=$HOME/.local/share
export SHELL=/bin/zsh
LOGS=$HOME/.local/state/w3bs-desktop/logs
mkdir -p -m 0700 "$XDG_RUNTIME_DIR" && mkdir -p "$LOGS" "$XDG_CONFIG_HOME/xdg-desktop-portal" "$HOME/Pictures"
printf '[preferred]\ndefault=kde;gtk\n' > "$XDG_CONFIG_HOME/xdg-desktop-portal/kde-portals.conf"
# Never lock: nobody is at this screen, and a locked session disables agent access.
printf '[Daemon]\nAutolock=false\nLockOnResume=false\nTimeout=0\n' > "$XDG_CONFIG_HOME/kscreenlockerrc"
rm -f /vnc/vnc.sock

say() { echo "w3bs-os: $*"; }
wait_for() { for _ in $(seq 1 100); do [ -e "$1" ] && return 0; sleep 0.1; done; say "timed out waiting for $1"; exit 1; }
first() { for f in "$@"; do [ -x "$f" ] && { echo "$f"; return 0; }; done; return 1; }

# The display: a vkms card (a KMS display with no hardware; Mesa renders OpenGL on the CPU).
DRM=${W3BS_DRM:-}
if [ -z "$DRM" ]; then
  for card in /dev/dri/card*; do
    [ -e "$card" ] || continue
    [ "$(basename "$(readlink -f "/sys/class/drm/$(basename "$card")/device")")" = vkms ] && DRM=$card && break
  done
fi
[ -n "$DRM" ] || {
  say "no virtual display: on the Linux host run 'sudo modprobe vkms', then pass the DRM cards in with"
  say "  docker run -v /dev/dri:/dev/dri --device-cgroup-rule 'c 226:* rwm' ..."
  exit 2
}

dbus-daemon --session --address="$DBUS_SESSION_BUS_ADDRESS" --nofork --nopidfile >"$LOGS/dbus.log" 2>&1 &
wait_for "$XDG_RUNTIME_DIR/bus"
pipewire >"$LOGS/pipewire.log" 2>&1 &
wireplumber >"$LOGS/wireplumber.log" 2>&1 &

# KWin trusts screencast and fake-input clients by their desktop file, which does not hold in a
# container; everything in this compositor is one agent desktop, so the checks are off here.
export KWIN_WAYLAND_NO_PERMISSION_CHECKS=1 KWIN_SCREENSHOT_NO_PERMISSION_CHECKS=1
kbuildsycoca6 >"$LOGS/kbuildsycoca.log" 2>&1 || true

KWIN_DRM_DEVICES=$DRM kwin_wayland --drm --socket wayland-0 >"$LOGS/kwin.log" 2>&1 &
KWIN=$!
wait_for "$XDG_RUNTIME_DIR/wayland-0"
export WAYLAND_DISPLAY=wayland-0 QT_QPA_PLATFORM=wayland
for _ in $(seq 1 20); do kscreen-doctor -o >/dev/null 2>&1 && break; sleep 0.5; done
# "Output: 1 Virtual-1 enabled connected ..." (colour codes stripped); the JSON field order
# differs between kscreen builds, so the plain listing is the stable source.
OUTPUT=$(kscreen-doctor -o 2>/dev/null | sed 's/\x1b\[[0-9;]*m//g' | sed -n 's/^Output: *[0-9]* *\([^ ]*\).*/\1/p' | head -1)
[ -n "$OUTPUT" ] && kscreen-doctor "output.$OUTPUT.mode.$GEOMETRY@60" >"$LOGS/kscreen.log" 2>&1 \
  || say "could not set $GEOMETRY on ${OUTPUT:-the display}; see $LOGS/kscreen.log"

PORTAL_KDE=$(first /usr/libexec/xdg-desktop-portal-kde /usr/lib/*/libexec/xdg-desktop-portal-kde) || { say "xdg-desktop-portal-kde not found"; exit 1; }
PORTAL=$(first /usr/libexec/xdg-desktop-portal /usr/lib/xdg-desktop-portal) || { say "xdg-desktop-portal not found"; exit 1; }
"$PORTAL_KDE" >"$LOGS/xdg-desktop-portal-kde.log" 2>&1 &
sleep 1
"$PORTAL" -r >"$LOGS/xdg-desktop-portal.log" 2>&1 &
sleep 1
# Nobody can click a consent dialog here: grant remote desktop and screenshots up front, the way
# KDE intends for unattended machines. This is the VNC server's screen, not agent access, which
# the desktop layer still gates.
flatpak permission-set kde-authorized remote-desktop "" yes
flatpak permission-set screenshot screenshot "" yes

plasmashell >"$LOGS/plasmashell.log" 2>&1 &
konsole >"$LOGS/konsole.log" 2>&1 &

# VNC: the owner-only Unix socket always; TCP 5900 only with a password.
w0vncserver -rfbport -1 -rfbunixpath /vnc/vnc.sock -SecurityTypes None -AlwaysShared \
  -desktop "W3BS OS" >"$LOGS/w0vncserver.log" 2>&1 &
W0=$!
if [ -n "${W3BS_VNC_PASSWORD:-}" ]; then
  printf '%s\n%s\n' "$W3BS_VNC_PASSWORD" "$W3BS_VNC_PASSWORD" | vncpasswd -f > "$XDG_RUNTIME_DIR/vncpasswd" 2>/dev/null
  chmod 0600 "$XDG_RUNTIME_DIR/vncpasswd"
  w0vncserver -rfbport 5900 -SecurityTypes VncAuth -PasswordFile "$XDG_RUNTIME_DIR/vncpasswd" \
    -AlwaysShared -desktop "W3BS OS" >"$LOGS/w0vncserver-tcp.log" 2>&1 &
  say "VNC also on TCP 5900 (password required)"
fi

ENABLED= ACCESS="off (turn it on from the tray, or run: w3bs-desktop enable)"
[ "${W3BS_DESKTOP_ENABLED:-0}" = 1 ] && ENABLED=--enabled-by-default ACCESS=on
w3bs-desktop daemon --adapter auto $ENABLED --mcp-http "${W3BS_MCP:-127.0.0.1:7801}" \
  >"$LOGS/w3bs-desktop.log" 2>&1 &
say "up: KWin on $DRM, VNC on /vnc/vnc.sock, desktop layer MCP on http://${W3BS_MCP:-127.0.0.1:7801}/mcp, agent access $ACCESS"

while kill -0 "$KWIN" 2>/dev/null && kill -0 "$W0" 2>/dev/null; do sleep 5; done
say "KWin or w0vncserver exited; see $LOGS"
exit 1
