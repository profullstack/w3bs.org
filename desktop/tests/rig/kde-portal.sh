#!/bin/sh
# Agentic Desktop Layer: prove the XDG-portal backend drives a real KDE Plasma (KWin Wayland)
# session, inside a disposable container. Nothing is installed on the host.
#
#   desktop/tests/rig/kde-portal.sh [SCRATCH_DIR]
#   TRAY=1 desktop/tests/rig/kde-portal.sh [SCRATCH_DIR]    # plus the tray stage (tray.sh below)
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
#
# Interlocks (interlocks.sh, always the last stage). Polkit: KDE's agent cannot register without a
# logind session, and the rig runs as root, whom polkit never asks; the rig tries the real agent
# and records why it cannot take requests, then opens a window carrying the agent's app id (what
# the KWin observer keys on) as the stand-in prompt. Lock: KWin's own screen locker, locked over
# org.freedesktop.ScreenSaver; it cannot be unlocked here (no password, no logind), so this stage
# runs last.
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

[ "${TRAY:-0}" = 1 ] && . /rig/tray.sh
. /rig/interlocks.sh

echo "== $pass passed, $fail failed"
[ $fail = 0 ]
EOF
# ---------------------------------------------------------------- interlocks (last stage)
cat > "$RIG/interlocks.sh" <<'EOF'
echo "== interlocks: setup"
state() { w3bs-desktop --json status | grep -o '"state": "[a-z]*"' | head -1 | cut -d'"' -f4; }
until_() { for i in $(seq 1 ${2:-25}); do eval "$1" && return 0; sleep 0.2; done; return 1; }
pgrep -f '^w3bs-desktop daemon' >/dev/null || { setsid w3bs-desktop daemon --adapter auto >$L/daemon-lock.log 2>&1 & sleep 1; }
if ! pgrep -f '^w3bs-desktop control' >/dev/null; then
  rm -f /root/ctl.fifo; mkfifo /root/ctl.fifo
  setsid sh -c 'exec sleep infinity > /root/ctl.fifo' &
  setsid w3bs-desktop control < /root/ctl.fifo >> $L/control.log 2>&1 &
  sleep 1
fi
echo enable > /root/ctl.fifo
until_ '[ "$(state)" = enabled ]' 50 || bad "could not enable for the interlock stage: $(state)"
set -- $(/rig/geom.sh T1)
W mouse click --agent tester --x $(( $1 + 15 )) --y $(( $2 + $4 / 2 )) >/dev/null; sleep 0.3

echo "== interlock: polkit authentication prompt"
dnf -y install --setopt=install_weak_deps=False polkit polkit-kde >$L/dnf-polkit.log 2>&1 || true
pgrep -x polkitd >/dev/null || { [ -S /run/dbus/system_bus_socket ] || { mkdir -p /run/dbus; setsid dbus-daemon --system --nofork --nopidfile >$L/sysdbus.log 2>&1 & sleep 1; }; }
AGENT=$(rpm -ql polkit-kde 2>/dev/null | grep -m1 -E '/polkit-kde-authentication-agent-1$' || true)
if [ -n "$AGENT" ]; then
  setsid "$AGENT" >$L/polkit-agent.log 2>&1 & PKA=$!; sleep 3
  echo "real polkit-kde agent: $(kill -0 $PKA 2>/dev/null && echo running || echo exited); log: $(grep -i -m2 -E 'regist|session|subject|error' $L/polkit-agent.log | tr '\n' ' ')"
  kill $PKA 2>/dev/null || true
else
  echo "real polkit-kde agent: not installable ($(tail -1 $L/dnf-polkit.log))"
fi
setsid foot --app-id=org.kde.polkit-kde-authentication-agent-1 --title="Authentication Required" sh -c 'sleep 120' >$L/foot-pk.log 2>&1 &
PK=$!
until_ '[ -n "$(/rig/geom.sh "Authentication Required")" ]' 25 || true
mark1=$(wc -c < /root/t1.txt)
set +e; printf '%s' "POLKIT" | W keyboard type --agent tester --stdin >/root/pk.out 2>&1; rc=$?
W mouse click --agent tester --x 960 --y 540 >/root/pkc.out 2>&1; rc2=$?; set -e
[ $rc != 0 ] && [ $rc2 != 0 ] && grep -q -i "authentication" /root/pk.out && grep -q -i "authentication" /root/pkc.out && [ "$(wc -c < /root/t1.txt)" = "$mark1" ] && [ "$(state)" = enabled ] \
  && ok "polkit-agent window up: type and click refused ($(grep -o 'input paused: [^;"(]*' /root/pk.out | head -1)), broker stays enabled" || bad "polkit interlock rc=$rc/$rc2 $(cat /root/pk.out) state=$(state)"
kill $PK 2>/dev/null || true
until_ '[ -z "$(/rig/geom.sh "Authentication Required")" ]' 25 || true
set -- $(/rig/geom.sh T1)
W mouse click --agent tester --x $(( $1 + 15 )) --y $(( $2 + $4 / 2 )) >/dev/null; sleep 0.3
printf '%s' " after polkit" | W keyboard type --agent tester --stdin >/dev/null 2>&1 || true; sleep 0.5
[ "$(tail -c 13 /root/t1.txt)" = " after polkit" ] && ok "prompt gone -> input flows again" || bad "after polkit: $(tail -c 30 /root/t1.txt)"

echo "== interlock: KWin screen locker"
if busctl --user status org.freedesktop.ScreenSaver >/dev/null 2>&1; then
  echo "org.freedesktop.ScreenSaver owner: $(busctl --user status org.freedesktop.ScreenSaver 2>/dev/null | grep -m1 '^Comm=')"
  busctl --user call org.freedesktop.ScreenSaver /ScreenSaver org.freedesktop.ScreenSaver Lock >/root/lock.out 2>&1 || echo "Lock: $(cat /root/lock.out)"
  until_ '[ "$(state)" = disabled ]' 50 && R=$(tail -n 5 /root/w3bs/state/audit/* | grep -o '"reason":"[^"]*"' | tail -1) || R=
  A=$(busctl --user call org.freedesktop.ScreenSaver /ScreenSaver org.freedesktop.ScreenSaver GetActive 2>/dev/null)
  [ "$A" = "b true" ] && [ "$(state)" = disabled ] && ok "screen locked through KWin's locker (GetActive: $A) -> broker disabled: $R" || bad "lock: GetActive '$A', state $(state)"
  set +e; printf '%s' "LOCKED" | W keyboard type --agent tester --stdin >/root/lk.out 2>&1; rc=$?; set -e
  [ $rc = 4 ] && ok "input while locked: exit 4 ($(grep -o '"code": *"[a-z_]*"' /root/lk.out | head -1))" || bad "while locked rc=$rc $(cat /root/lk.out)"
else
  bad "org.freedesktop.ScreenSaver is not on the session bus (no screen locker to test)"
fi
EOF

# ---------------------------------------------------------------- tray stage (TRAY=1)
# menu.py drives the StatusNotifierItem and its com.canonical.dbusmenu menu the way a tray host
# (plasmashell) does; appr.py is an agent that retries one identical request until approved.
cat > "$RIG/menu.py" <<'EOF'
# menu.py: drive the w3bs-desktop StatusNotifierItem over D-Bus with busctl (no extra packages).
#   menu.py items            -> registered SNI items, one per line
#   menu.py title            -> the item's Title
#   menu.py labels           -> menu labels, one per line ("-" prefix = insensitive)
#   menu.py click LABEL [N]  -> com.canonical.dbusmenu Event(id, "clicked") on the Nth item with that label
#   menu.py activate         -> org.kde.StatusNotifierItem.Activate(0, 0) (a left click)
import json, subprocess, sys, time

def bus(*a):
    r = subprocess.run(["busctl", "--user", "--json=short", *a], capture_output=True, text=True)
    if r.returncode:
        sys.exit("busctl %s: %s" % (" ".join(a), r.stderr.strip()))
    return json.loads(r.stdout) if r.stdout.strip() else None

def items():
    v = bus("get-property", "org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher", "RegisteredStatusNotifierItems")
    return v["data"]

def ours():
    for it in items():
        svc, _, path = it.partition("/")
        path = "/" + path if path else "/StatusNotifierItem"
        try:
            if bus("get-property", svc, path, "org.kde.StatusNotifierItem", "Id")["data"] == "w3bs-desktop":
                return svc, path
        except SystemExit:
            pass
    sys.exit("no w3bs-desktop item registered")

def prop(name):
    svc, path = ours()
    return bus("get-property", svc, path, "org.kde.StatusNotifierItem", name)["data"]

def unvar(v):
    return v["data"] if isinstance(v, dict) and "type" in v and "data" in v else v

def flat(node, out):
    node = unvar(node)
    nid, props, kids = node
    props = {k: unvar(x) for k, x in props.items()}
    if nid != 0:
        out.append((nid, props.get("label", "-" if props.get("type") == "separator" else ""), props.get("enabled", True)))
    for k in kids:
        flat(k, out)
    return out

def layout():
    svc, _ = ours()
    m = prop("Menu")
    v = bus("call", "--", svc, m, "com.canonical.dbusmenu", "GetLayout", "iias", "0", "-1", "0")
    return svc, m, flat(v["data"][1], [])

cmd = sys.argv[1]
if cmd == "items":
    print("\n".join(items()))
elif cmd == "title":
    print(prop("Title"))
elif cmd == "labels":
    for _, l, en in layout()[2]:
        print(("" if en else "-") + l)
elif cmd == "click":
    want, nth = sys.argv[2], int(sys.argv[3]) if len(sys.argv) > 3 else 1
    svc, m, ls = layout()
    hits = [i for i, l, en in ls if l.strip() == want and en]
    if len(hits) < nth:
        sys.exit("no enabled menu item %r in %r" % (want, [l for _, l, _ in ls]))
    bus("call", svc, m, "com.canonical.dbusmenu", "Event", "isvu", str(hits[nth - 1]), "clicked", "s", "", str(int(time.time())))
elif cmd == "activate":
    svc, path = ours()
    bus("call", svc, path, "org.kde.StatusNotifierItem", "Activate", "ii", "0", "0")
EOF
cat > "$RIG/appr.py" <<'EOF'
# appr.py AGENT TEXT: one agent task that types TEXT; on approval_required it prints the
# approval id and retries the IDENTICAL request (same task, same actions) once a second for 30 s,
# through the broker's busy answers while the approval is pending.
import json, os, socket, sys, time
agent, text = sys.argv[1], sys.argv[2]
cfg = os.environ["W3BS_DESKTOP_CONFIG_DIR"]; run = os.environ["W3BS_DESKTOP_RUNTIME_DIR"]
token = open(f"{cfg}/tokens/{agent}.token").read().strip()
s = socket.socket(socket.AF_UNIX); s.connect(f"{run}/agent.sock"); f = s.makefile("rw")
n = 0
def call(method, params):
    global n
    n += 1
    f.write(json.dumps({"v": 1, "id": str(n), "method": method, "params": params}) + "\n"); f.flush()
    while True:
        v = json.loads(f.readline())
        if "event" not in v and v.get("id") == str(n):
            return v
hello = call("hello", {"token": token, "client": "rig"})
if hello.get("error"): sys.exit("hello: %s" % hello)
task = call("task.begin", {"description": "rig approval"})["result"]["id"]
batch = {"task": task, "target": {"window": None}, "actions": [{"type": "type", "text": text}]}
first = True
for _ in range(30):
    r = call("lease.acquire", {"task": task})
    r = call("input", batch)
    err = r.get("error")
    # approval_required first; then busy ("input is suspended while an approval is on screen")
    if err and err.get("code") in ("approval_required", "busy"):
        if first and err.get("code") == "approval_required":
            print("approval_required", err.get("details", {}).get("approval_id"), flush=True); first = False
        time.sleep(1); continue
    print(json.dumps(r), flush=True)
    call("task.end", {"task": task, "outcome": "completed"})
    sys.exit(0 if not err and r.get("result", {}).get("outcome") == "completed" else 1)
sys.exit("still waiting for approval after 30 s")
EOF

# The tray is the ONLY native control here: the terminal `control` is stopped and the broker
# restarted. kded6 provides org.kde.StatusNotifierWatcher (on a real desktop plasmashell is the
# host); KWin itself serves org.kde.kglobalaccel, the GlobalShortcuts portal's backend. Plasma asks
# the human to confirm a new global shortcut ("Global Shortcuts Requested"); the rig answers it
# in-session with Enter, sent through the portal by the test agent standing in for the human.
cat > "$RIG/tray.sh" <<'EOF'
echo "== tray (StatusNotifierItem, dbusmenu, GlobalShortcuts)"
dnf -y install --setopt=install_weak_deps=False kf6-kded plasma-workspace-libs >$L/dnf-tray.log 2>&1
pkill -f '^w3bs-desktop control' || true; pkill -f '^sleep infinity' || true
pkill -f '^w3bs-desktop daemon' || true; sleep 1
setsid w3bs-desktop daemon --adapter auto >$L/daemon-tray.log 2>&1 &
setsid kded6 >$L/kded.log 2>&1 &
for i in $(seq 1 50); do busctl --user status org.kde.StatusNotifierWatcher >/dev/null 2>&1 && break; sleep 0.2; done
busctl --user status org.kde.StatusNotifierWatcher >/dev/null 2>&1 && busctl --user status org.kde.kglobalaccel >/dev/null 2>&1 \
  && ok "StatusNotifierWatcher (kded6) and kglobalaccel (KWin) on the bus" || bad "no StatusNotifierWatcher/kglobalaccel"
# a rerun starts from an unbound shortcut, so the confirmation dialog appears every time
busctl --user call org.kde.kglobalaccel /kglobalaccel org.kde.KGlobalAccel unRegister as 4 org.w3bs.Desktop emergency-stop "" "" >/dev/null 2>&1 || true
M() { python3 /rig/menu.py "$@"; }
# T1's `cat > file` keeps its write offset, so truncating would leave a hole: read what was
# appended since a mark instead.
mark() { MARK=$(wc -c < /root/t1.txt); }
since() { tail -c +$((MARK + 1)) /root/t1.txt; }
state() { w3bs-desktop --json status | grep -o '"state": "[a-z]*"' | head -1 | cut -d'"' -f4; }
until_() { for i in $(seq 1 ${2:-25}); do eval "$1" && return 0; sleep 0.2; done; return 1; }
tray_up() {
  setsid w3bs-desktop tray >>$L/tray.log 2>&1 &
  TP=$!
  until_ 'M items 2>/dev/null | grep -q "StatusNotifierItem-$TP-"' 50
}
tray_up && ok "item registered: $(M items | grep "StatusNotifierItem-$TP-")" || { bad "tray did not register: $(cat $L/tray.log)"; echo "== $pass passed, $fail failed"; exit 1; }
M title | grep -q Disabled && ok "title: $(M title)" || bad "title: $(M title)"
M labels > /root/menu0.txt
grep -qx "Enable agent access" /root/menu0.txt && grep -qx "Stop all agent activity" /root/menu0.txt && grep -qx "Quit and disable" /root/menu0.txt \
  && ok "menu has Enable / Stop all / Quit and disable" || bad "menu: $(cat /root/menu0.txt)"

echo "== tray: enable from the menu"
sleep 1
[ "$(/rig/active.sh)" = "Global Shortcuts Requested" ] && DLG=1 || DLG=0
M click "Enable agent access"
until_ '[ "$(state)" = enabled ]' && ok "Enable clicked -> broker enabled$( [ $DLG = 1 ] && echo ' (while the shortcut dialog was still open)')" || bad "enable from tray: $(state); $(tail -3 $L/tray.log)"
until_ 'M title | grep -q Enabled' && ok "title: $(M title)" || bad "title: $(M title)"

echo "== tray: emergency-stop shortcut"
if [ "$(/rig/active.sh)" = "Global Shortcuts Requested" ]; then
  W keyboard press --agent tester --key Enter >/dev/null     # the human accepting the binding
fi
SC=
until_ 'M labels | grep -q "^-Emergency stop: "' 25 && SC=$(M labels | sed -n 's/^-Emergency stop: //p')
case "$SC" in
  *Esc*) ok "shortcut bound by the desktop: $SC" ;;
  *) bad "shortcut not bound: '$SC' $(grep shortcut $L/tray.log)" ;;
esac

echo "== tray: agent input works under the tray"
set -- $(/rig/geom.sh T1)
W mouse click --agent tester --x $(( $1 + 15 )) --y $(( $2 + $4 / 2 )) >/dev/null; sleep 0.3
mark; type_ "via tray"; sleep 0.5
[ "$(since)" = "via tray" ] && ok "agent typed into T1 after a tray enable" || bad "T1: $(since)"

echo "== tray: approval"
w3bs-desktop agents add ap --preset broad --approve-for type >/dev/null 2>&1 || true
mark; python3 /rig/appr.py ap "approved text" > /root/appr.out 2>&1 &
APID=$!
until_ 'grep -q approval_required /root/appr.out' && ok "agent got approval_required" || bad "appr: $(cat /root/appr.out)"
until_ 'M labels | grep -q "^-Approval: ap wants: "' && M labels | grep -qx "    Approve" && M labels | grep -qx "    Deny" \
  && ok "menu shows the approval: $(M labels | sed -n 's/^-Approval: //p')" || bad "approval menu: $(M labels)"
[ -z "$(since)" ] && ok "nothing typed before approval" || bad "typed before approval: $(since)"
M click Approve
wait $APID && rc=0 || rc=$?
sleep 0.5
[ $rc = 0 ] && [ "$(since)" = "approved text" ] && ok "Approve clicked -> identical retry typed: $(since)" || bad "after approve rc=$rc $(cat /root/appr.out) / $(since)"
! M labels | grep -q "Approval:" && ok "approval gone from the menu" || bad "approval still listed: $(M labels)"

echo "== tray: left click"
M activate
until_ '[ "$(state)" = disabled ]' && ok "left click while enabled -> disabled" || bad "activate: $(state)"
M activate; sleep 2
[ "$(state)" = disabled ] && M title | grep -q Disabled && ok "left click while disabled does nothing ($(M title))" || bad "activate while disabled: $(state)"

echo "== tray: emergency stop"
M click "Enable agent access"; until_ '[ "$(state)" = enabled ]' || bad "re-enable: $(state)"
E0=$(cat /root/w3bs/state/audit/* | grep -c '"reason":"emergency stop"' || true)
W keyboard shortcut --agent tester --keys Super+Alt+Escape >/dev/null 2>&1 || true   # KWin takes the chord
until_ '[ "$(state)" = disabled ]' && [ "$(cat /root/w3bs/state/audit/* | grep -c '"reason":"emergency stop"')" -gt "$E0" ] \
  && ok "Super+Alt+Escape (through KWin + GlobalShortcuts) -> disabled, audited as emergency stop" || bad "emergency: $(state)"
kill -0 $TP 2>/dev/null && until_ 'M title | grep -q Disabled' && ok "tray still running: $(M title)" || bad "tray after emergency: $(M title)"

echo "== tray: losing the tray"
M click "Enable agent access"; until_ '[ "$(state)" = enabled ]' || bad "re-enable: $(state)"
kill -9 $TP
until_ '[ "$(state)" = disabled ]' && ok "tray killed (SIGKILL) while enabled -> broker disabled" || bad "after kill: $(state)"
tray_up || bad "tray restart"
M click "Enable agent access"; until_ '[ "$(state)" = enabled ]' || bad "re-enable: $(state)"
M click "Quit and disable"
until_ '! kill -0 $TP 2>/dev/null' && until_ '[ "$(state)" = disabled ]' && until_ '! M items 2>/dev/null | grep -q "StatusNotifierItem-$TP-"' \
  && ok "Quit and disable -> tray exited, item unregistered, broker disabled" || bad "quit: alive=$(kill -0 $TP 2>/dev/null && echo yes) state=$(state)"
EOF

chmod +x "$RIG"/*.sh
docker exec -e TRAY="${TRAY:-0}" "$NAME" /rig/inner.sh
