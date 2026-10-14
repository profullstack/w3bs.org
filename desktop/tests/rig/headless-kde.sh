#!/bin/sh
# Agentic Desktop Layer on a non-desktop instance: a disposable KDE Plasma (KWin Wayland) container
# with the w3bs CLI installed from w3bs.org, the broker started with `--enabled-by-default`, and NO
# native control (tray, panel or `w3bs desktop control`). Agents must be able to work at once.
#
#   desktop/tests/rig/headless-kde.sh [SCRATCH_DIR]
#   KEEP=1 desktop/tests/rig/headless-kde.sh      # leave the container running afterwards
#
# Needs docker and a DRM render node (/dev/dri/renderD*), for the same reason as kde-portal.sh.
# The w3bs-desktop under test is built from this checkout (the published release may predate the
# flag); the w3bs CLI and `w3bs install agentic-desktop-layer` come from w3bs.org as a user gets them.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
SRC=${SRC:-$(cd "$HERE/../.." && pwd)}            # the desktop/ workspace
RIG=${1:-${CLAUDE_JOB_DIR:-$HOME/scratch}/headless-kde-rig}
NAME=${NAME:-adl-headless-kde}
IMAGE=${IMAGE:-fedora:44}
RENDER=${RENDER:-$(ls /dev/dri/renderD* 2>/dev/null | head -1)}
[ -n "$RENDER" ] || { echo "no /dev/dri/renderD* on this host; KWin cannot screencast without OpenGL" >&2; exit 2; }
mkdir -p "$RIG"

cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; }
[ "${KEEP:-0}" = 1 ] || trap cleanup EXIT

docker run -d --name "$NAME" --memory 6g --cpus 4 --device "$RENDER" \
  -v "$SRC":/src:ro -v "$RIG":/rig "$IMAGE" sleep infinity >/dev/null

cat > "$RIG/env.sh" <<'EOF'
export XDG_RUNTIME_DIR=/run/user/0
export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/0/bus
export XDG_CURRENT_DESKTOP=KDE XDG_SESSION_TYPE=wayland XDG_SESSION_DESKTOP=KDE
export WAYLAND_DISPLAY=wayland-0 QT_QPA_PLATFORM=wayland QT_FORCE_STDERR_LOGGING=1 LANG=C.UTF-8
export PATH=/root/.local/bin:$PATH
EOF

cat > "$RIG/mcp.sh" <<'EOF'
#!/bin/sh
# mcp.sh TOOL [ARGS_JSON] -> the tools/call response line
. /rig/env.sh
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"rig","version":"0"}}}' \
  "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":${2:-{\}}}}" \
  | w3bs desktop mcp serve --stdio --agent ci 2>&1 | tail -1
EOF

cat > "$RIG/inner.sh" <<'EOF'
#!/bin/sh
set -eu
L=/rig/logs; mkdir -p $L
pass=0; fail=0
ok()  { pass=$((pass+1)); echo "PASS: $*"; }
bad() { fail=$((fail+1)); echo "FAIL: $*"; }

echo "== packages"
dnf -y install --setopt=install_weak_deps=False kwin-wayland xdg-desktop-portal xdg-desktop-portal-kde \
  pipewire wireplumber dbus-daemon flatpak foot qt6-qttools procps-ng libcap file python3 cargo rust gcc curl >$L/dnf.log 2>&1
setcap -r /usr/bin/kwin_wayland   # Docker's bounding set lacks CAP_SYS_NICE

echo "== w3bs from w3bs.org"
curl --proto '=https' -fsSL https://w3bs.org/install.sh | W3BS_YES=1 sh >$L/w3bs-install.log 2>&1 \
  && ok "w3bs CLI installed: $(/root/.local/bin/w3bs --version 2>&1 | head -1)" || { bad "w3bs install"; tail -20 $L/w3bs-install.log; }
. /rig/env.sh
w3bs install --yes agentic-desktop-layer >$L/adl-install.log 2>&1 \
  && ok "w3bs install agentic-desktop-layer: $(w3bs-desktop --version 2>&1 | head -1)" || { bad "agentic-desktop-layer install"; tail -20 $L/adl-install.log; }

echo "== build this checkout's w3bs-desktop"
(cd /src && CARGO_HOME=/rig/cargo CARGO_TARGET_DIR=/rig/target cargo build --release -p w3bs-desktop >$L/build.log 2>&1)
install -m 0755 /rig/target/release/w3bs-desktop /root/.local/bin/w3bs-desktop
w3bs desktop daemon --help | grep -q -- --enabled-by-default && ok "w3bs desktop daemon has --enabled-by-default" || bad "flag missing"

mkdir -p -m 0700 $XDG_RUNTIME_DIR /root/Pictures
mkdir -p /etc/xdg-desktop-portal && printf '[preferred]\ndefault=kde;gtk\n' > /etc/xdg-desktop-portal/kde-portals.conf

echo "== session"
setsid dbus-daemon --session --address=$DBUS_SESSION_BUS_ADDRESS --nofork --nopidfile >$L/dbus.log 2>&1 &
sleep 1
setsid pipewire >$L/pipewire.log 2>&1 &
sleep 1
setsid wireplumber >$L/wireplumber.log 2>&1 &
env -u QT_QPA_PLATFORM setsid kwin_wayland --virtual --width 1920 --height 1080 --socket wayland-0 >$L/kwin.log 2>&1 &
for i in $(seq 1 50); do [ -S $XDG_RUNTIME_DIR/wayland-0 ] && break; sleep 0.2; done
setsid /usr/libexec/xdg-desktop-portal-kde >$L/xdpk.log 2>&1 &
sleep 2
setsid /usr/libexec/xdg-desktop-portal -r >$L/xdp.log 2>&1 &
sleep 2
# Portal consent, pre-granted the way KDE intends for headless setups (see kde-portal.sh).
flatpak permission-set kde-authorized remote-desktop "" yes
flatpak permission-set screenshot screenshot "" yes

setsid foot --title=T1 sh -c 'stty -icanon -echo; exec cat > /root/t1.txt' >$L/foot1.log 2>&1 &
sleep 2

echo "== broker: --enabled-by-default, no native control"
setsid w3bs desktop daemon --adapter auto --enabled-by-default --mcp-http 127.0.0.1:7801 >$L/daemon.log 2>&1 &
for i in $(seq 1 40); do w3bs desktop --json status 2>/dev/null | grep -q '"state": "enabled"' && break; sleep 0.5; done
w3bs desktop --json status | grep -q '"state": "enabled"' && ok "enabled at startup: $(w3bs desktop status | head -1)" || { bad "not enabled: $(w3bs desktop status 2>&1 | head -1)"; cat $L/daemon.log; }
pgrep -f "w3bs-desktop (control|tray)" >/dev/null && bad "a native control is running" || ok "no tray, panel or control running"
w3bs desktop --json doctor | grep -q "not required: started with --enabled-by-default" && ok "doctor: native control not required" || bad "doctor"

echo "== an agent works straight away"
w3bs desktop agents add ci --preset broad >/dev/null && ok "agent ci registered"
W() { w3bs desktop --json "$@"; }
DISP=$(/rig/mcp.sh desktop_layout | grep -o 'stream-[0-9]*' | head -1)
W mouse move --agent ci --x 640 --y 360 --display "$DISP" >/dev/null && ok "mouse move" || bad "mouse move"
TID=$(/rig/mcp.sh desktop_windows | python3 -c 'import sys,json,re; print(re.search(r"T1", sys.stdin.read()) and "T1" or "")')
[ "$TID" = T1 ] && ok "MCP desktop_windows sees the T1 terminal" || bad "windows: $(/rig/mcp.sh desktop_windows | head -c 400)"
printf 'typed by an agent, nobody enabled me' | W keyboard type --agent ci --stdin >/dev/null; sleep 0.5
[ "$(cat /root/t1.txt)" = "typed by an agent, nobody enabled me" ] && ok "keyboard text landed in T1" || bad "T1: $(cat /root/t1.txt | od -c | head -2)"
W screen capture --agent ci --display "$DISP" --output /rig/shot.png >/dev/null
file /rig/shot.png | grep -q "PNG image data, 1920 x 1080" && ok "screenshot: $(file -b /rig/shot.png)" || bad "screenshot: $(file /rig/shot.png)"
/rig/mcp.sh desktop_status | grep -q 'enabled' && ok "MCP desktop_status: enabled" || bad "mcp status"
curl -s -o /dev/null -w '%{http_code}' -X POST http://127.0.0.1:7801/mcp | grep -q 401 && ok "MCP HTTP on 127.0.0.1:7801 (bearer required)" || bad "mcp http"

echo "== the off switch still works, and enable needs no code"
w3bs desktop disable >/dev/null
w3bs desktop --json status | grep -q '"state": "disabled"' && ok "disable -> disabled" || bad "disable"
printf x | W keyboard type --agent ci --stdin >/dev/null 2>&1 && bad "typed while disabled" || ok "agent refused while disabled"
w3bs desktop enable </dev/null | head -1 | grep -q ENABLED && ok "w3bs desktop enable (no tty, no code) -> enabled" || bad "re-enable: $(w3bs desktop status | head -1)"

echo
echo "RESULT: $pass passed, $fail failed"
[ $fail = 0 ]
EOF

chmod +x "$RIG"/*.sh
docker exec "$NAME" /rig/inner.sh
