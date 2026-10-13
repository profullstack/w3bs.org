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

# win.py TITLE FIELD < windows-list JSON -> that window's field (bounds as "x y w h")
cat > "$RIG/win.py" <<'EOF'
import json, math, sys
def find(v):
    if isinstance(v, list) and v and isinstance(v[0], dict) and "app_id" in v[0]: return v
    for c in (v.values() if isinstance(v, dict) else v if isinstance(v, list) else []):
        r = find(c)
        if r is not None: return r
    return None
ws = find(json.load(sys.stdin)) or []
title, field = sys.argv[1], sys.argv[2]
if title == "-":   # summary
    print(len(ws), sum(1 for w in ws if w["focused"]), " ".join(sorted(w["title"] for w in ws)))
    sys.exit()
for w in ws:
    if w["title"] == title:
        v = w.get(field)
        # rounded half-up like the KWin script's Math.round (frame geometry can be fractional)
        if field == "bounds" and v: v = " ".join(str(math.floor(v[k] + 0.5)) for k in ("x", "y", "width", "height"))
        print(v if v is not None else "")
        break
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
setsid w3bs-desktop daemon --adapter auto >$L/daemon.log 2>&1 &
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

echo "== KWin window observer (window list, focus, stacking)"
w3bs-desktop --json status | grep -q '"adapter": "linux-portal+kwin"' && ok "--adapter auto picked portal+kwin" || bad "adapter: $(w3bs-desktop --json status | grep adapter)"
W windows list --agent tester > /root/wins.json
set -- $(python3 /rig/win.py - - < /root/wins.json)
[ "$1" = 2 ] && [ "$2" = 1 ] && [ "$3 $4" = "T1 T2" ] && ok "windows list: T1, T2, exactly one focused" || bad "windows list: $(cat /root/wins.json)"
[ "$(python3 /rig/win.py T1 bounds < /root/wins.json)" = "$(/rig/geom.sh T1)" ] && [ "$(python3 /rig/win.py T2 bounds < /root/wins.json)" = "$(/rig/geom.sh T2)" ] \
  && ok "geometry matches KWin frameGeometry ($(python3 /rig/win.py T1 bounds < /root/wins.json) / $(python3 /rig/win.py T2 bounds < /root/wins.json))" || bad "geometry: $(cat /root/wins.json)"
APP=$(python3 /rig/win.py T1 app_id < /root/wins.json); echo "foot app id: $APP"
T1=$(python3 /rig/win.py T1 id < /root/wins.json); T2=$(python3 /rig/win.py T2 id < /root/wins.json)
[ "$(python3 /rig/win.py T2 focused < /root/wins.json)" = True ] && ok "T2 (clicked last) is the focused one" || bad "focus: $(cat /root/wins.json)"
printf '%s' "to focused T2" | W keyboard type --agent tester --window "$T2" --stdin >/dev/null; sleep 0.5
[ "$(cat /root/t2.txt)" = "into T2to focused T2" ] && ok "type --window <focused> delivered" || bad "T2: $(cat /root/t2.txt)"
before=$(cat /root/t1.txt)
set +e; printf '%s' "MUST NOT APPEAR" | W keyboard type --agent tester --window "$T1" --stdin >/root/stale.out 2>&1; rc=$?; set -e
sleep 0.5
[ $rc != 0 ] && grep -q target_stale /root/stale.out && [ "$(cat /root/t1.txt)" = "$before" ] && ! grep -q "MUST NOT" /root/t2.txt \
  && ok "type --window <unfocused> refused: $(grep -o 'focus moved[^"]*' /root/stale.out | head -1)" || bad "unfocused type rc=$rc $(cat /root/stale.out)"
set -- $(python3 /rig/win.py T1 bounds < /root/wins.json); t1x=$1 t1y=$2 t1w=$3 t1h=$4
set -- $(python3 /rig/win.py T2 bounds < /root/wins.json); t2x=$1 t2y=$2 t2w=$3 t2h=$4
W mouse click --agent tester --window "$T1" --x 15 --y $(( t1h / 2 )) >/root/click1.out 2>&1 && sleep 0.3 && [ "$(/rig/active.sh)" = T1 ] \
  && ok "click --window T1 on its visible strip focused T1" || bad "click T1: $(cat /root/click1.out)"
# T1 is raised now, so it covers T2 where they overlap; aim at T2 inside that overlap.
ox=$(( (t2x > t1x ? t2x : t1x) )); oy=$(( (t2y > t1y ? t2y : t1y) ))
ex=$(( (t2x + t2w < t1x + t1w ? t2x + t2w : t1x + t1w) )); ey=$(( (t2y + t2h < t1y + t1h ? t2y + t2h : t1y + t1h) ))
cx=$(( (ox + ex) / 2 - t2x )); cy=$(( (oy + ey) / 2 - t2y ))
echo "overlap ($ox,$oy)-($ex,$ey); T2-relative target ($cx,$cy)"
set +e; W mouse click --agent tester --window "$T2" --x $cx --y $cy >/root/covered.out 2>&1; rc=$?; set -e
sleep 0.3
[ $rc != 0 ] && grep -q target_stale /root/covered.out && grep -q covered /root/covered.out && [ "$(/rig/active.sh)" = T1 ] \
  && ok "click on covered part of T2 refused: $(grep -o 'target is covered[^"]*' /root/covered.out | head -1)" || bad "covered click rc=$rc $(cat /root/covered.out)"
printf '%s' "kwin T1" | W keyboard type --agent tester --window "$T1" --stdin >/dev/null; sleep 0.5
[ "$(cat /root/t1.txt)" = "${before}kwin T1" ] || [ "$(tail -c 7 /root/t1.txt)" = "kwin T1" ] && ok "type --window T1 after focusing it" || bad "T1: $(tail -c 40 /root/t1.txt)"
set +e; W inspect --agent tester --window "$T1" >/root/inspect.out 2>&1; set -e
grep -q unsupported /root/inspect.out && ok "inspect stays unsupported" || bad "inspect: $(cat /root/inspect.out)"

echo "== restricted agent (app grant enforced against KWin's app ids)"
w3bs-desktop agents add r --preset restricted --app "$APP" >/dev/null
setsid foot --app-id=org.w3bs.rig.other --title=T3 sh -c 'stty -icanon -echo; exec cat > /root/t3.txt' >$L/foot3.log 2>&1 &
sleep 1.5
W windows list --agent tester > /root/wins3.json
T3=$(python3 /rig/win.py T3 id < /root/wins3.json); echo "T3 app id: $(python3 /rig/win.py T3 app_id < /root/wins3.json)"
W windows list --agent r > /root/winsr.json
[ -n "$T3" ] && [ "$(python3 /rig/win.py - - < /root/winsr.json | cut -d' ' -f1)" = 2 ] && [ -z "$(python3 /rig/win.py T3 id < /root/winsr.json)" ] \
  && ok "restricted agent sees only the granted app's 2 windows" || bad "restricted list: $(cat /root/winsr.json)"
set +e; printf '%s' "NOPE" | W keyboard type --agent r --window "$T3" --stdin >/root/r3.out 2>&1; rc=$?; set -e
sleep 0.3
[ $rc = 4 ] && grep -q "not in this agent's application grant" /root/r3.out && [ ! -s /root/t3.txt ] && ok "restricted type into another app denied" || bad "restricted T3 rc=$rc $(cat /root/r3.out)"
set -- $(python3 /rig/win.py T3 bounds < /root/wins3.json)
set +e; W mouse click --agent r --x $(( $1 + $3 / 2 )) --y $(( $2 + $4 / 2 )) >/root/r3c.out 2>&1; rc=$?; set -e
[ $rc = 4 ] && grep -q "org.w3bs.rig.other is not in this agent's application grant" /root/r3c.out && ok "restricted click on another app's window denied" || bad "restricted click rc=$rc $(cat /root/r3c.out)"
W mouse click --agent r --window "$T1" --x 15 --y $(( t1h / 2 )) >/dev/null; sleep 0.3
printf '%s' " restricted ok" | W keyboard type --agent r --window "$T1" --stdin >/root/r1.out 2>&1; sleep 0.5
[ "$(tail -c 14 /root/t1.txt)" = " restricted ok" ] && ok "restricted agent typed into the granted terminal" || bad "restricted T1: $(cat /root/r1.out) / $(tail -c 40 /root/t1.txt)"
pkill -f 'app-id=org.w3bs.rig.other' || true; sleep 0.5

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
