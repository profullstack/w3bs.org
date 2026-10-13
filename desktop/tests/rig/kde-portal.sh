#!/bin/sh
# Agentic Desktop Layer: prove the XDG-portal backend drives a real KDE Plasma (KWin Wayland)
# session, inside a disposable container. Nothing is installed on the host.
#
#   desktop/tests/rig/kde-portal.sh [SCRATCH_DIR]
#
# Needs: docker, and a DRM render node on the host (/dev/dri/renderD*). KWin's screencast
# plugin (used by the KDE RemoteDesktop/ScreenCast portal) refuses QPainter compositing
# ("Unsupported compositing type"), and KWin's virtual backend only picks OpenGL when it can
# open a DRM device, so the render node is passed through (read/use only).
#
# Consent is satisfied the way KDE intends for headless setups, not by patching our code:
#   - RemoteDesktop+ScreenCast: xdg-desktop-portal-kde "MegaAuth", permission-store table
#     `kde-authorized`, id `remote-desktop`, app id "" (= unsandboxed host apps; that is the app id
#     xdg-desktop-portal gives a host process with no systemd app scope).
#   - Screenshot (non-interactive): xdg-desktop-portal's own table `screenshot`, id `screenshot`,
#     app id "". Without it the frontend shows "Allow Apps to Take Screenshots?" and BLOCKS every
#     other portal call (including RemoteDesktop Notify*) until that dialog is answered.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
SRC=${SRC:-$(cd "$HERE/../.." && pwd)}            # the desktop/ workspace
RIG=${1:-${CLAUDE_JOB_DIR:-$HOME/scratch}/kde-rig}
NAME=${NAME:-adl-kde-rig}
IMAGE=${IMAGE:-fedora:44}
RENDER=${RENDER:-$(ls /dev/dri/renderD* 2>/dev/null | head -1)}
[ -n "$RENDER" ] || { echo "no /dev/dri/renderD* on this host; KWin cannot screencast without OpenGL" >&2; exit 2; }
mkdir -p "$RIG"

cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; }
[ "${KEEP:-0}" = 1 ] || trap cleanup EXIT

docker run -d --name "$NAME" --memory 6g --cpus 4 --device "$RENDER" \
  -v "$SRC":/src:ro -v "$RIG":/rig "$IMAGE" sleep infinity >/dev/null

# ---------------------------------------------------------------- inner helpers
cat > "$RIG/env.sh" <<'EOF'
export XDG_RUNTIME_DIR=/run/user/0
export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/0/bus
export XDG_CURRENT_DESKTOP=KDE XDG_SESSION_TYPE=wayland XDG_SESSION_DESKTOP=KDE
export WAYLAND_DISPLAY=wayland-0 QT_QPA_PLATFORM=wayland QT_FORCE_STDERR_LOGGING=1 LANG=C.UTF-8
export W3BS_DESKTOP_RUNTIME_DIR=/root/w3bs/run W3BS_DESKTOP_CONFIG_DIR=/root/w3bs/config W3BS_DESKTOP_STATE_DIR=/root/w3bs/state
export PATH=/rig/target/release:$PATH
EOF

# Run a one-shot KWin script; its print() lines land in kwin.log prefixed "js: ".
cat > "$RIG/kwin-js.sh" <<'EOF'
#!/bin/sh
. /rig/env.sh
n=adl$$; f=/root/$n.js; cat > $f
id=$(qdbus-qt6 org.kde.KWin /Scripting org.kde.kwin.Scripting.loadScript $f $n)
qdbus-qt6 org.kde.KWin /Scripting/Script$id org.kde.kwin.Script.run >/dev/null 2>&1
sleep 0.5
qdbus-qt6 org.kde.KWin /Scripting org.kde.kwin.Scripting.unloadScript $n >/dev/null
rm -f $f
EOF
cat > "$RIG/cursor.sh" <<'EOF'
#!/bin/sh
echo 'print("ADLCURSOR " + workspace.cursorPos.x + "," + workspace.cursorPos.y);' | /rig/kwin-js.sh
grep ADLCURSOR /rig/logs/kwin.log | tail -1 | sed 's/.*ADLCURSOR //'
EOF
cat > "$RIG/active.sh" <<'EOF'
#!/bin/sh
echo 'print("ADLACTIVE " + (workspace.activeWindow ? workspace.activeWindow.caption : "none"));' | /rig/kwin-js.sh
grep ADLACTIVE /rig/logs/kwin.log | tail -1 | sed 's/.*ADLACTIVE //'
EOF
cat > "$RIG/geom.sh" <<'EOF'
#!/bin/sh
# geom.sh CAPTION -> "x y w h"
printf 'var ws = workspace.windowList(); for (var i = 0; i < ws.length; i++) if (ws[i].caption == "%s") { var g = ws[i].frameGeometry; print("ADLGEOM " + Math.round(g.x) + " " + Math.round(g.y) + " " + Math.round(g.width) + " " + Math.round(g.height)); }\n' "$1" | /rig/kwin-js.sh
grep ADLGEOM /rig/logs/kwin.log | tail -1 | sed 's/.*ADLGEOM //'
EOF
cat > "$RIG/layout.sh" <<'EOF'
#!/bin/sh
. /rig/env.sh
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"rig","version":"0"}}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"desktop_layout","arguments":{}}}' \
  | w3bs-desktop mcp serve --stdio --agent tester 2>&1 | tail -1
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
  pipewire wireplumber dbus-daemon flatpak foot qt6-qttools procps-ng libcap file python3 cargo rust gcc >$L/dnf.log 2>&1
# Docker's default bounding set lacks CAP_SYS_NICE; the file capability makes exec fail with EPERM.
setcap -r /usr/bin/kwin_wayland

echo "== build"
(cd /src && CARGO_HOME=/rig/cargo CARGO_TARGET_DIR=/rig/target cargo build --release -p w3bs-desktop >$L/build.log 2>&1)

. /rig/env.sh
mkdir -p -m 0700 $XDG_RUNTIME_DIR /root/Pictures /root/w3bs   # the KDE screenshot portal saves into ~/Pictures
mkdir -p /etc/xdg-desktop-portal && printf '[preferred]\ndefault=kde;gtk\n' > /etc/xdg-desktop-portal/kde-portals.conf

echo "== session"
setsid dbus-daemon --session --address=$DBUS_SESSION_BUS_ADDRESS --nofork --nopidfile >$L/dbus.log 2>&1 &
sleep 1
setsid pipewire >$L/pipewire.log 2>&1 &
sleep 1
setsid wireplumber >$L/wireplumber.log 2>&1 &
# KDE_FULL_SESSION must stay unset: Qt's built-in QKdeTheme segfaults kwin at startup without it.
env -u QT_QPA_PLATFORM QT_LOGGING_RULES="js.debug=true;kwin_scripting.debug=true" \
  setsid kwin_wayland --virtual --width 1920 --height 1080 --socket wayland-0 >$L/kwin.log 2>&1 &
for i in $(seq 1 50); do [ -S $XDG_RUNTIME_DIR/wayland-0 ] && break; sleep 0.2; done
QT_LOGGING_RULES="xdp-kde*=true" setsid /usr/libexec/xdg-desktop-portal-kde >$L/xdpk.log 2>&1 &
sleep 2
setsid /usr/libexec/xdg-desktop-portal -r -v >$L/xdp.log 2>&1 &
sleep 2
busctl --user call org.kde.KWin /KWin org.kde.KWin supportInformation | grep -q "Compositing Type: OpenGL" && ok "KWin composites with OpenGL" || bad "KWin is not on OpenGL (screencast will fail)"

echo "== consent (permission store)"
flatpak permission-set kde-authorized remote-desktop "" yes
flatpak permission-set screenshot screenshot "" yes

echo "== targets"
setsid foot --title=T1 sh -c 'stty -icanon -echo; exec cat > /root/t1.txt' >$L/foot1.log 2>&1 &
sleep 1.5
setsid foot --title=T2 sh -c 'stty -icanon -echo; exec cat > /root/t2.txt' >$L/foot2.log 2>&1 &
sleep 1.5

echo "== broker"
setsid w3bs-desktop daemon --adapter portal >$L/daemon.log 2>&1 &
sleep 1
w3bs-desktop agents add tester --preset broad >/dev/null
rm -f /root/ctl.fifo; mkfifo /root/ctl.fifo
setsid sh -c 'exec sleep infinity > /root/ctl.fifo' &
setsid w3bs-desktop control < /root/ctl.fifo > $L/control.log 2>&1 &
sleep 1
echo enable > /root/ctl.fifo
sleep 5
w3bs-desktop --json status | grep -q '"state": "enabled"' && ok "enabled through the portal" || { bad "enable"; cat $L/control.log; }
DISP=$(/rig/layout.sh | grep -o 'stream-[0-9]*' | head -1)
echo "display: $DISP"

W() { w3bs-desktop --json "$@"; }
type_() { printf '%s' "$1" | W keyboard type --agent tester --stdin >/dev/null; }

echo "== pointer"
for p in "400 300" "1500 900"; do
  set -- $p
  W mouse move --agent tester --x $1 --y $2 --display $DISP >/dev/null
  got=$(/rig/cursor.sh)
  [ "$got" = "$1,$2" ] && ok "cursor at $got" || bad "cursor: wanted $1,$2 got $got"
done

echo "== click switches focus, keyboard lands in the clicked window"
set -- $(/rig/geom.sh T1); x1=$(( $1 + 15 )); y1=$(( $2 + $4 / 2 ))     # left strip of T1 (T2 cascades right)
set -- $(/rig/geom.sh T2); x2=$(( $1 + $3 - 15 )); y2=$(( $2 + $4 / 2 ))
W mouse click --agent tester --x $x1 --y $y1 >/dev/null; sleep 0.3
[ "$(/rig/active.sh)" = T1 ] && ok "click focused T1" || bad "click did not focus T1"
type_ "hello from w3bs desktop 123"; W keyboard press --agent tester --key Enter >/dev/null
type_ "héllo üß €"; W keyboard press --agent tester --key Enter >/dev/null
W mouse click --agent tester --x $x2 --y $y2 >/dev/null; sleep 0.3
[ "$(/rig/active.sh)" = T2 ] && ok "click focused T2" || bad "click did not focus T2"
type_ "into T2"; sleep 0.5
[ "$(sed -n 1p /root/t1.txt)" = "hello from w3bs desktop 123" ] && ok "ASCII typed exactly" || bad "ASCII: $(sed -n 1p /root/t1.txt | od -c | head -2)"
[ "$(sed -n 2p /root/t1.txt)" = "héllo üß €" ] && ok "non-ASCII keysym text typed exactly" || bad "non-ASCII: $(sed -n 2p /root/t1.txt | od -c | head -2)"
[ "$(cat /root/t2.txt)" = "into T2" ] && ok "second window got only its text" || bad "T2: $(cat /root/t2.txt)"

echo "== screenshot"
W screen capture --agent tester --display $DISP --output /rig/shot.png >/dev/null
file /rig/shot.png | grep -q "PNG image data, 1920 x 1080" && ok "$(file -b /rig/shot.png)" || bad "screenshot: $(file /rig/shot.png)"

echo "== disable during a long type"
W mouse click --agent tester --x $x1 --y $y1 >/dev/null; sleep 0.3
: > /root/t1.txt
python3 -c 'print("x"*3000, end="")' > /root/long.txt
( W keyboard type --agent tester --stdin < /root/long.txt > /root/long.out 2>&1 || true ) &
tpid=$!
sleep 0.3
W disable > /root/disable.json
wait $tpid; sleep 1
n=$(wc -c < /root/t1.txt)
[ "$n" -lt 3000 ] && ok "typing stopped at $n/3000 chars" || bad "typed all $n chars"
grep -q '"tasks_cancelled": 1' /root/disable.json && ok "daemon cancelled the task" || bad "disable: $(cat /root/disable.json)"
grep -q '"cancelled"' /root/long.out && ok "type call reported cancelled" || bad "type: $(cat /root/long.out)"
set +e; printf hi | W keyboard type --agent tester --stdin >/root/after.out 2>&1; rc=$?; set -e
[ $rc = 4 ] && grep -q disabled /root/after.out && ok "input after disable: exit 4 disabled" || bad "after disable rc=$rc $(cat /root/after.out)"

echo "== revocation from the desktop side"
echo enable > /root/ctl.fifo; sleep 4
S=$(busctl --user tree org.freedesktop.impl.portal.desktop.kde 2>/dev/null | grep -o '/org/freedesktop/portal/desktop/session/[^ ]*/[^ ]*' | tail -1)
busctl --user call org.freedesktop.impl.portal.desktop.kde "$S" org.freedesktop.impl.portal.Session Close
sleep 2
w3bs-desktop --json status | grep -q '"state": "disabled"' && ok "portal session closed -> broker disabled" || bad "broker still $(w3bs-desktop --json status | grep '"state"')"

echo "== $pass passed, $fail failed"
[ $fail = 0 ]
EOF
chmod +x "$RIG"/*.sh
docker exec "$NAME" /rig/inner.sh
