#!/bin/sh
# Fleet screens end to end: a local w3bs.org, a disposable KDE Plasma (KWin Wayland) machine in
# Docker sharing its screen through TigerVNC's w0vncserver and `w3bs fleet screen`, and the real
# /fleet/<id>/screen page (noVNC) in headless Chromium, relayed through the server.
#
#   desktop/tests/rig/fleet-screen.sh [SCRATCH_DIR]
#   W3BS_BIN=path/to/w3bs KEEP=1 desktop/tests/rig/fleet-screen.sh
#
# Checks: the page shows the remote desktop; in view mode, input the page sends is dropped by
# the agent (the page's own switch is forced on); after `w3bs fleet screen on --control`,
# typing on the page lands in the remote terminal. Screenshots go to SCRATCH_DIR/shots.
#
# Needs docker, a DRM render node, bun, node with this repo's node_modules (Playwright
# Chromium), and a w3bs binary built from this checkout (default: package-manager's target).
# The container uses the host network so it reaches the server on localhost.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
RIG=${1:-${CLAUDE_JOB_DIR:-$HOME/scratch}/fleet-screen-rig}
NAME=${NAME:-adl-fleet-screen}
IMAGE=${IMAGE:-fedora:44}
PORT=${PORT:-3377}
VNC_PORT=${VNC_PORT:-5937}
W3BS_BIN=${W3BS_BIN:-${CARGO_TARGET_DIR:-$REPO/package-manager/target}/debug/w3bs}
RENDER=${RENDER:-$(ls /dev/dri/renderD* 2>/dev/null | head -1)}
[ -n "$RENDER" ] || { echo "no /dev/dri/renderD* on this host; KWin cannot screencast without OpenGL" >&2; exit 2; }
[ -x "$W3BS_BIN" ] || { echo "no w3bs binary at $W3BS_BIN (cargo build -p w3bs-cli, or set W3BS_BIN)" >&2; exit 2; }
rm -rf "$RIG" && mkdir -p "$RIG/shots" "$RIG/logs"
export VNC_PASSWORD=${VNC_PASSWORD:-rig-$(date +%s)}

pass=0; fail=0
ok()  { pass=$((pass+1)); echo "PASS: $*"; }
bad() { fail=$((fail+1)); echo "FAIL: $*"; }

SERVER_PID=
cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null || true
  [ "${KEEP:-0}" = 1 ] || docker rm -f "$NAME" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "== w3bs.org on localhost:$PORT"
(cd "$REPO" && PORT=$PORT DB="$RIG/registry.sqlite" OUT="$RIG/state.json" exec bun "$HERE/fleet-screen-server.mjs") >"$RIG/logs/server.log" 2>&1 &
SERVER_PID=$!
for i in $(seq 1 50); do [ -s "$RIG/state.json" ] && break; sleep 0.2; done
[ -s "$RIG/state.json" ] || { cat "$RIG/logs/server.log"; exit 1; }
TOKEN=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$RIG/state.json")
COOKIE=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["cookie"])' "$RIG/state.json")
machines() { curl -s -H "Cookie: w3bs_session=$COOKIE" "http://localhost:$PORT/api/fleet/machines"; }

echo "== machine: KDE + w0vncserver + w3bs fleet"
docker rm -f "$NAME" >/dev/null 2>&1 || true
docker run -d --name "$NAME" --network host --memory 6g --cpus 4 --device "$RENDER" \
  -v "$RIG":/rig -v "$W3BS_BIN":/usr/local/bin/w3bs:ro "$IMAGE" sleep infinity >/dev/null

cat > "$RIG/env.sh" <<'EOF'
export XDG_RUNTIME_DIR=/run/user/0
export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/0/bus
export XDG_CURRENT_DESKTOP=KDE XDG_SESSION_TYPE=wayland XDG_SESSION_DESKTOP=KDE
export WAYLAND_DISPLAY=wayland-0 QT_QPA_PLATFORM=wayland QT_FORCE_STDERR_LOGGING=1 LANG=C.UTF-8
EOF

cat > "$RIG/inner.sh" <<EOF
#!/bin/sh
set -eu
L=/rig/logs
dnf -y install --setopt=install_weak_deps=False kwin-wayland xdg-desktop-portal xdg-desktop-portal-kde \\
  pipewire wireplumber dbus-daemon flatpak foot tigervnc-wayland-server tigervnc-server-common procps-ng libcap >\$L/dnf.log 2>&1
setcap -r /usr/bin/kwin_wayland   # Docker's bounding set lacks CAP_SYS_NICE
. /rig/env.sh
mkdir -p -m 0700 \$XDG_RUNTIME_DIR
mkdir -p /etc/xdg-desktop-portal && printf '[preferred]\ndefault=kde;gtk\n' > /etc/xdg-desktop-portal/kde-portals.conf
setsid dbus-daemon --session --address=\$DBUS_SESSION_BUS_ADDRESS --nofork --nopidfile >\$L/dbus.log 2>&1 &
sleep 1
setsid pipewire >\$L/pipewire.log 2>&1 &
sleep 1
setsid wireplumber >\$L/wireplumber.log 2>&1 &
env -u QT_QPA_PLATFORM setsid kwin_wayland --virtual --width 1920 --height 1080 --socket wayland-0 >\$L/kwin.log 2>&1 &
for i in \$(seq 1 50); do [ -S \$XDG_RUNTIME_DIR/wayland-0 ] && break; sleep 0.2; done
setsid /usr/libexec/xdg-desktop-portal-kde >\$L/xdpk.log 2>&1 &
sleep 2
setsid /usr/libexec/xdg-desktop-portal -r >\$L/xdp.log 2>&1 &
sleep 2
# The headless pre-grant: w0vncserver opens a portal session per viewer, so keep it in place.
flatpak permission-set kde-authorized remote-desktop "" yes
setsid foot --title=T1 sh -c 'echo "an AI agent works here"; stty -icanon; exec cat > /root/t1.txt' >\$L/foot.log 2>&1 &
sleep 2
mkdir -p /root/.config/tigervnc
echo '$VNC_PASSWORD' | vncpasswd -f > /root/.config/tigervnc/passwd
chmod 600 /root/.config/tigervnc/passwd
setsid w0vncserver -localhost -rfbport $VNC_PORT -SecurityTypes VncAuth \\
  -PasswordFile /root/.config/tigervnc/passwd -AlwaysShared >\$L/w0vncserver.log 2>&1 &
sleep 1
w3bs fleet join $TOKEN --api http://localhost:$PORT --no-service --screen view --vnc 127.0.0.1:$VNC_PORT
setsid w3bs fleet agent >\$L/agent.log 2>&1 &
EOF
chmod +x "$RIG"/*.sh
docker exec "$NAME" /rig/inner.sh >"$RIG/logs/inner.log" 2>&1 && ok "machine joined: $(grep -m1 Joined "$RIG/logs/inner.log")" || { bad "machine setup"; tail -30 "$RIG/logs/inner.log"; exit 1; }

for i in $(seq 1 60); do machines | grep -q '"screen_listening":true' && break; sleep 0.5; done
machines | grep -q '"screen_listening":true' && ok "agent's screen listener connected" || bad "listener: $(machines | head -c 600)"
machines | grep -q '"reachable":true' && ok "agent reports the VNC server reachable" || bad "reachable: $(machines | head -c 600)"

echo "== view only: the page shows the desktop, and forced input is dropped by the agent"
(cd "$REPO" && node "$HERE/fleet-screen-browser.mjs" view "$RIG/state.json" "$RIG/shots" "typed-in-view-mode") >"$RIG/logs/browser-view.log" 2>&1 \
  && ok "viewer live: $(tail -1 "$RIG/logs/browser-view.log")" || { bad "viewer (view)"; tail -20 "$RIG/logs/browser-view.log"; }
sleep 1
docker exec "$NAME" sh -c 'cat /root/t1.txt 2>/dev/null' | grep -q typed-in-view-mode && bad "view-only input reached the machine" || ok "view only: nothing typed on the machine"
for i in $(seq 1 20); do grep -q "input messages dropped" "$RIG/logs/agent.log" && break; sleep 0.5; done
grep -q "input messages dropped (view only)" "$RIG/logs/agent.log" && ok "agent: $(grep -m1 'input messages dropped' "$RIG/logs/agent.log" | sed 's/^w3bs fleet agent: //')" || bad "agent log: $(tail -5 "$RIG/logs/agent.log")"

echo "== control: w3bs fleet screen on --control, then typing lands"
docker exec "$NAME" sh -c ". /rig/env.sh; w3bs fleet screen on --control --vnc 127.0.0.1:$VNC_PORT" >"$RIG/logs/screen-on.log" 2>&1 && ok "$(head -1 "$RIG/logs/screen-on.log")" || bad "screen on: $(cat "$RIG/logs/screen-on.log")"
for i in $(seq 1 90); do machines | grep -q '"mode":"control"' && machines | grep -q '"screen_listening":true' && break; sleep 0.5; done
machines | grep -q '"mode":"control"' && ok "agent picked up control mode without a restart" || bad "mode: $(machines | head -c 400)"
(cd "$REPO" && node "$HERE/fleet-screen-browser.mjs" control "$RIG/state.json" "$RIG/shots" "typed-through-w3bs") >"$RIG/logs/browser-control.log" 2>&1 \
  && ok "viewer live: $(tail -1 "$RIG/logs/browser-control.log")" || { bad "viewer (control)"; tail -20 "$RIG/logs/browser-control.log"; }
sleep 1
docker exec "$NAME" sh -c 'cat /root/t1.txt' | grep -q typed-through-w3bs && ok "control: typed on the page, landed in the machine's terminal" || bad "terminal got: $(docker exec "$NAME" sh -c 'cat /root/t1.txt' | od -c | head -3)"

echo "== history"
machines | python3 -c 'import json,sys; a=[x for x in json.load(sys.stdin)[0]["actions"] if x["action"]=="screen"]; print(len(a), "sessions;", " | ".join(x["status"]+": "+x["output"] for x in a))' > "$RIG/logs/history.txt"
grep -q "^2 sessions" "$RIG/logs/history.txt" && ok "both sessions logged: $(cat "$RIG/logs/history.txt")" || bad "history: $(cat "$RIG/logs/history.txt")"

echo
echo "screenshots: $RIG/shots"
echo "RESULT: $pass passed, $fail failed"
[ $fail = 0 ]
