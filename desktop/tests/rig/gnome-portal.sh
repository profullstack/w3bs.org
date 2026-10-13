#!/bin/sh
# Agentic Desktop Layer: prove the XDG-portal backend drives a real GNOME Shell (Mutter Wayland)
# session, inside a disposable container. Nothing is installed on the host.
#
#   desktop/tests/rig/gnome-portal.sh [SCRATCH_DIR]
#
# Needs: docker. A DRM render node (/dev/dri/renderD*) is passed through when the host has one
# (Mutter then renders through GBM on it). It is optional: with RENDER= (empty) Mutter falls back
# to surfaceless EGL on software rendering and all checks still pass (verified 2026-10-13).
#
# Session: `gnome-shell --headless --virtual-monitor 1920x1080 --wayland --no-x11` with a system
# bus but no logind. /run/systemd/seats is removed so the shell uses its no-logind code path (with
# the directory present and no logind, GNOME Shell 50 throws in TimeLimitsManager and segfaults).
#
# Consent. GNOME has no headless pre-authorization for RemoteDesktop: xdg-desktop-portal-gnome 50
# always shows its "Remote Desktop" dialog for a session without a valid restore token, and
# "Allow Remote Interaction" in it is OFF by default (Share without it grants no input devices).
# Fabricating a restore-token row in the permission store would mean forging the backend's private
# restore data, so the rig does what a person does instead: it answers the dialog inside the
# session. A test-only shell extension (adl-rig@w3bs.org, installed only in the container) puts the
# shell in unsafe mode so org.gnome.Shell.Eval works; human-click.sh then creates a Clutter virtual
# pointer INSIDE gnome-shell and clicks the toggle and Share. That pointer stands in for the human;
# the code under test only ever talks to the RemoteDesktop portal. "Remember This Selection" is
# checked by default, so the portal returns a restore token (persist_mode 2) and every later enable
# in this run must succeed WITHOUT a dialog; the rig checks that too.
#
# Screenshot (non-interactive): xdg-desktop-portal's own permission table `screenshot`, id
# `screenshot`, app id "" (unsandboxed host apps). The rig first checks that capture without it
# fails fast with a clear message, then grants it.
#
# Readback: pointer via global.get_pointer(), focus via global.display.focus_window (Eval).
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
SRC=${SRC:-$(cd "$HERE/../.." && pwd)}            # the desktop/ workspace
RIG=${1:-${CLAUDE_JOB_DIR:-$HOME/scratch}/gnome-rig}
NAME=${NAME:-adl-gnome-rig}
IMAGE=${IMAGE:-fedora:44}
RENDER=${RENDER-$(ls /dev/dri/renderD* 2>/dev/null | head -1)}
mkdir -p "$RIG"

cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; }
[ "${KEEP:-0}" = 1 ] || trap cleanup EXIT

docker run -d --name "$NAME" --memory 6g --cpus 4 ${RENDER:+--device "$RENDER"} \
  -v "$SRC":/src:ro -v "$RIG":/rig "$IMAGE" sleep infinity >/dev/null

# ---------------------------------------------------------------- inner helpers
cat > "$RIG/env.sh" <<'EOF'
export XDG_RUNTIME_DIR=/run/user/0
export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/0/bus
export XDG_CURRENT_DESKTOP=GNOME XDG_SESSION_TYPE=wayland XDG_SESSION_DESKTOP=gnome
export WAYLAND_DISPLAY=wayland-0 LANG=C.UTF-8
export W3BS_DESKTOP_RUNTIME_DIR=/root/w3bs/run W3BS_DESKTOP_CONFIG_DIR=/root/w3bs/config W3BS_DESKTOP_STATE_DIR=/root/w3bs/state
export PATH=/rig/target/release:$PATH
EOF

mkdir -p "$RIG/ext"
cat > "$RIG/ext/metadata.json" <<'EOF'
{"uuid": "adl-rig@w3bs.org", "name": "ADL test rig", "description": "Test-only: unsafe mode, so the rig can read pointer/focus and stand in for the human via org.gnome.Shell.Eval.", "shell-version": ["50"]}
EOF
cat > "$RIG/ext/extension.js" <<'EOF'
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
export default class AdlRig extends Extension {
    enable() { global.context.unsafe_mode = true; }
    disable() { global.context.unsafe_mode = false; }
}
EOF

# eval.sh JS -> result of org.gnome.Shell.Eval, unquoted
cat > "$RIG/eval.sh" <<'EOF'
#!/bin/sh
. /rig/env.sh
gdbus call --session -d org.gnome.Shell -o /org/gnome/Shell -m org.gnome.Shell.Eval "$1" | sed -E "s/^\(true, '(.*)'\)$/\1/; s/^\"(.*)\"$/\1/"
EOF
cat > "$RIG/cursor.sh" <<'EOF'
#!/bin/sh
/rig/eval.sh 'global.get_pointer().slice(0, 2).join(",")'
EOF
cat > "$RIG/active.sh" <<'EOF'
#!/bin/sh
/rig/eval.sh 'String(global.display.focus_window ? global.display.focus_window.title : "none")'
EOF
cat > "$RIG/geom.sh" <<'EOF'
#!/bin/sh
# geom.sh TITLE -> "x y w h" of the newest window with that title (empty if none)
/rig/eval.sh "const w = global.get_window_actors().map(a => a.meta_window).filter(w => w.title == '$1').pop(); w ? (r => [r.x, r.y, r.width, r.height].join(' '))(w.get_frame_rect()) : ''"
EOF
cat > "$RIG/human-click.sh" <<'EOF'
#!/bin/sh
# human-click.sh X Y [move]: the rig's stand-in for the person at the screen. A Clutter virtual pointer
# created inside gnome-shell; NOT the code under test (that only talks to the portal).
/rig/eval.sh "const C = imports.gi.Clutter; const seat = global.stage.context.get_backend().get_default_seat();
global._adlHuman ??= seat.create_virtual_device(C.InputDeviceType.POINTER_DEVICE);
const d = global._adlHuman; d.notify_absolute_motion(C.CURRENT_TIME, $1, $2);
if ('${3:-}' != 'move') {
  d.notify_button(C.CURRENT_TIME, C.BUTTON_PRIMARY, C.ButtonState.PRESSED);
  d.notify_button(C.CURRENT_TIME, C.BUTTON_PRIMARY, C.ButtonState.RELEASED);
} 1" >/dev/null
sleep 0.4
EOF
cat > "$RIG/shellshot.sh" <<'EOF'
#!/bin/sh
. /rig/env.sh
gdbus call --session -d org.gnome.Shell.Screenshot -o /org/gnome/Shell/Screenshot -m org.gnome.Shell.Screenshot.Screenshot false false "$1" >/dev/null
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
pass=0; fail=0; known=0
ok()  { pass=$((pass+1)); echo "PASS: $*"; }
bad() { fail=$((fail+1)); echo "FAIL: $*"; }
kno() { known=$((known+1)); echo "KNOWN: $*"; }   # a documented GNOME limitation, not gated

echo "== packages"
dnf -y install --setopt=install_weak_deps=False gnome-shell mutter xdg-desktop-portal xdg-desktop-portal-gnome \
  pipewire wireplumber dbus-daemon flatpak foot procps-ng file python3 cargo rust gcc >$L/dnf.log 2>&1
rmdir /run/systemd/seats 2>/dev/null || true   # no logind here: take the shell's no-logind path

echo "== build"
(cd /src && CARGO_HOME=/rig/cargo CARGO_TARGET_DIR=/rig/target cargo build --release -p w3bs-desktop >$L/build.log 2>&1)

. /rig/env.sh
mkdir -p -m 0700 $XDG_RUNTIME_DIR /root/w3bs
mkdir -p /root/Pictures /run/dbus /root/.config/foot
printf '[tweak]\nfont-monospace-warn=no\n' > /root/.config/foot/foot.ini
D=/root/.local/share/gnome-shell/extensions/adl-rig@w3bs.org; mkdir -p $D; cp /rig/ext/* $D/

echo "== session"
setsid dbus-daemon --system --nofork --nopidfile >$L/sysdbus.log 2>&1 &
setsid dbus-daemon --session --address=$DBUS_SESSION_BUS_ADDRESS --nofork --nopidfile >$L/dbus.log 2>&1 &
sleep 1
gsettings set org.gnome.shell enabled-extensions "['adl-rig@w3bs.org']"
gsettings set org.gnome.shell welcome-dialog-last-shown-version '999'
gsettings set org.gnome.desktop.interface enable-hot-corners false   # the virtual pointer starts at 0,0
setsid pipewire >$L/pipewire.log 2>&1 &
sleep 1
setsid wireplumber >$L/wireplumber.log 2>&1 &
setsid gnome-shell --headless --virtual-monitor 1920x1080 --wayland --no-x11 --wayland-display wayland-0 >$L/shell.log 2>&1 &
for i in $(seq 1 100); do [ "$(/rig/eval.sh 'global.context.unsafe_mode' 2>/dev/null)" = true ] && break; sleep 0.3; done
[ "$(/rig/eval.sh 'global.context.unsafe_mode')" = true ] || { echo "gnome-shell did not come up (or the rig extension did not load)"; tail -40 $L/shell.log; exit 1; }
echo "renderer: $(grep -o -E "Created gbm renderer for '[^']*'|llvmpipe[^)]*|surfaceless[^ ]*" $L/shell.log | head -1)"
echo "$(gnome-shell --version), $(rpm -q --qf '%{NAME} %{VERSION}' xdg-desktop-portal-gnome)"
G_MESSAGES_DEBUG=all setsid /usr/libexec/xdg-desktop-portal-gnome -v >$L/xdpg.log 2>&1 &
sleep 2
setsid /usr/libexec/xdg-desktop-portal -r -v >$L/xdp.log 2>&1 &
sleep 2

echo "== targets"
setsid foot --title=T1 sh -c 'stty -icanon -echo; exec cat > /root/t1.txt' >$L/foot1.log 2>&1 &
sleep 1.5
setsid foot --title=T2 sh -c 'stty -icanon -echo; exec cat > /root/t2.txt' >$L/foot2.log 2>&1 &
sleep 1.5
/rig/eval.sh 'Main.overview.hide(); 1' >/dev/null
# A new Clutter virtual device drops its first events while it is being added; warm it up now.
/rig/human-click.sh 960 1000 move; sleep 1; /rig/human-click.sh 960 1010 move

echo "== broker"
setsid w3bs-desktop daemon --adapter portal >$L/daemon.log 2>&1 &
sleep 1
w3bs-desktop agents add tester --preset broad >/dev/null
rm -f /root/ctl.fifo; mkfifo /root/ctl.fifo
setsid sh -c 'exec sleep infinity > /root/ctl.fifo' &
setsid w3bs-desktop control < /root/ctl.fifo > $L/control.log 2>&1 &
sleep 1
state() { w3bs-desktop --json status | grep -o '"state": "[a-z]*"' | head -1 | cut -d'"' -f4; }

# enable_ [share-only]: enable from the native control; if the portal dialog appears, answer it as
# the human: turn on "Allow Remote Interaction" (off by default) and press Share, or with
# share-only press Share alone. Sets DIALOG=1 when a dialog appeared. Returns 0 once enabled, 1 on
# timeout, 2 when the broker reports blocked.
enable_() {
  DIALOG=0; mode=${1:-}
  echo enable > /root/ctl.fifo
  for i in $(seq 1 40); do
    case "$(state)" in enabled) return 0 ;; blocked) return 2 ;; esac
    g=$(/rig/geom.sh "Remote Desktop")
    if [ -n "$g" ]; then
      DIALOG=1; sleep 0.5
      set -- $g
      /rig/shellshot.sh /rig/consent-dialog.png
      [ "$mode" = share-only ] || /rig/human-click.sh $(( $1 + $3 - 80 )) $(( $2 + 156 ))   # toggle
      /rig/human-click.sh $(( $1 + $3 - 43 )) $(( $2 + 22 ))                                  # Share
    fi
    sleep 0.5
  done
  return 1
}

echo "== consent"
set +e; enable_ share-only; rc=$?; set -e
[ $rc = 2 ] && [ $DIALOG = 1 ] && grep -q 'granted no input devices' $L/control.log && ok "Share without \"Allow Remote Interaction\" -> blocked: granted no input devices" || bad "share-only: rc=$rc dialog=$DIALOG $(tail -2 $L/control.log)"
[ ! -e /root/w3bs/config/portal-restore-token ] && ok "no restore token kept for the input-less grant" || bad "a restore token was saved for an input-less grant (next enable would skip the dialog)"
enable_ && [ $DIALOG = 1 ] && ok "enabled through the portal (dialog shown again, answered in-session)" || { bad "enable: dialog=$DIALOG $(tail -3 $L/control.log)"; echo "== $pass passed, $fail failed"; exit 1; }
[ -s /root/w3bs/config/portal-restore-token ] && ok "portal returned a restore token (persist_mode 2)" || bad "no restore token saved"
DISP=$(/rig/layout.sh | grep -o 'stream-[0-9]*' | head -1)
[ -n "$DISP" ] && ok "layout reports display $DISP" || { bad "layout: $(/rig/layout.sh)"; exit 1; }

W() { w3bs-desktop --json "$@"; }
type_() { printf '%s' "$1" | W keyboard type --agent tester --stdin >/dev/null; }

echo "== pointer"
for p in "400 300" "1500 900"; do
  set -- $p
  W mouse move --agent tester --x $1 --y $2 --display $DISP >/dev/null; sleep 0.3
  got=$(/rig/cursor.sh)
  [ "$got" = "$1,$2" ] && ok "cursor at $got" || bad "cursor: wanted $1,$2 got $got"
done

echo "== click switches focus, keyboard lands in the clicked window"
set -- $(/rig/geom.sh T1); x1=$(( $1 + 15 )); y1=$(( $2 + $4 / 2 ))     # left strip of T1 (T2 cascades right/down)
set -- $(/rig/geom.sh T2); x2=$(( $1 + $3 - 15 )); y2=$(( $2 + $4 / 2 ))
W mouse click --agent tester --x $x1 --y $y1 >/dev/null; sleep 0.3
[ "$(/rig/active.sh)" = T1 ] && ok "click focused T1" || bad "click did not focus T1 (focus: $(/rig/active.sh))"
type_ "hello from w3bs desktop 123"; W keyboard press --agent tester --key Enter >/dev/null
type_ "héllo üß €"; W keyboard press --agent tester --key Enter >/dev/null
W mouse click --agent tester --x $x2 --y $y2 >/dev/null; sleep 0.3
[ "$(/rig/active.sh)" = T2 ] && ok "click focused T2" || bad "click did not focus T2 (focus: $(/rig/active.sh))"
type_ "into T2"; sleep 0.5
[ "$(sed -n 1p /root/t1.txt)" = "hello from w3bs desktop 123" ] && ok "ASCII typed exactly" || bad "ASCII: $(sed -n 1p /root/t1.txt | od -c | head -2)"
# Mutter maps a keysym only onto a keycode of the active layout (us here); é ü ß € are not on it,
# so Mutter drops them ("No keycode found for keyval ... in current group") and the portal call
# still succeeds. KWin remaps such keysyms; Mutter does not.
l2=$(sed -n 2p /root/t1.txt)
if [ "$l2" = "héllo üß €" ]; then ok "non-ASCII keysym text typed exactly"
elif [ "$l2" = "hllo  " ] && grep -q "No keycode found for keyval e9" $L/shell.log; then kno "non-ASCII outside the layout dropped by Mutter: got '$l2' for 'héllo üß €'; shell.log: $(grep -m1 -o 'No keycode found for keyval e9 in current group' $L/shell.log)"
else bad "non-ASCII: $(printf %s "$l2" | od -c | head -2)"; fi
[ "$(cat /root/t2.txt)" = "into T2" ] && ok "second window got only its text" || bad "T2: $(cat /root/t2.txt | od -c | head -2)"

echo "== screenshot"
set +e; timeout 30 w3bs-desktop --json screen capture --agent tester --display $DISP --output /rig/noshot.png >/root/noshot.out 2>&1; rc=$?; set -e
[ $rc != 124 ] && grep -q 'not pre-authorized' /root/noshot.out && ok "capture without a grant fails fast (rc=$rc): blocked, not pre-authorized" || bad "capture without grant rc=$rc $(cat /root/noshot.out)"
type_ "!"; sleep 0.3
[ "$(cat /root/t2.txt)" = "into T2!" ] && ok "input still flows after the refused capture" || bad "input after refused capture: $(cat /root/t2.txt)"
flatpak permission-set screenshot screenshot "" yes
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

echo "== re-enable uses the restore token"
enable_ && [ $DIALOG = 0 ] && ok "re-enabled with no dialog (restore token honoured)" || bad "re-enable: state $(state), dialog shown: $DIALOG"

echo "== revocation: GNOME's own 'Stop Screen Sharing' top-bar button"
sleep 1
g=$(/rig/eval.sh 'const i = Main.panel.statusArea.screenSharing; i.visible ? (([x, y]) => [Math.round(x + i.width / 2), Math.round(y + i.height / 2)].join(" "))(i.get_transformed_position()) : ""')
if [ -n "$g" ]; then
  /rig/human-click.sh $g; sleep 2
  [ "$(state)" = disabled ] && ok "top-bar Stop (human click at $g) -> portal Session.Closed -> broker disabled" || bad "broker still $(state) after top-bar Stop"
else
  bad "no screen-sharing indicator in the top bar while enabled (state $(state))"
fi

echo "== revocation: portal backend restart"
enable_ || true
[ "$(state)" = enabled ] || bad "could not re-enable before the backend-restart check"
pkill -f /usr/libexec/xdg-desktop-portal-gnome
sleep 2
[ "$(state)" = disabled ] && ok "xdg-desktop-portal-gnome killed -> broker disabled" || bad "broker still $(state) after the backend died"

echo "== impl Session.Close called directly (what the KDE rig uses for desktop-side revocation)"
# Bring the backend back by D-Bus activation (a hand-started copy races the activated one for the
# name, and the loser's NameOwnerChanged rightly disables the broker).
busctl --user call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus StartServiceByName su org.freedesktop.impl.portal.desktop.gnome 0 >/dev/null
sleep 2
enable_ || bad "re-enable after the backend restart: $(state)"
S=$(busctl --user tree org.freedesktop.impl.portal.desktop.gnome 2>/dev/null | grep -o '/org/freedesktop/portal/desktop/session/[^ ]*/[^ ]*' | tail -1)
busctl --user call org.freedesktop.impl.portal.desktop.gnome "$S" org.freedesktop.impl.portal.Session Close
sleep 2
# xdg-desktop-portal-gnome stops the Mutter session on Close but, per the impl spec, emits no
# Closed for a Close it was asked to do, so the frontend never tells the client. GNOME's own UI
# never takes this path (its Stop button goes through Mutter, checked above).
st=$(state); s2=$(stat -c %s /root/t2.txt)
W mouse click --agent tester --x $x2 --y $y2 >/dev/null || true; sleep 0.3; type_ "lost" || true; sleep 0.5
if [ "$st" = disabled ]; then ok "impl Session.Close -> broker disabled"
else kno "impl Session.Close: frontend sent no Closed, broker stays $st and input is silently lost (T2 grew $(( $(stat -c %s /root/t2.txt) - s2 )) bytes)"; fi

/rig/shellshot.sh /rig/final.png || true
echo "== $pass passed, $fail failed, $known known GNOME limitations"
[ $fail = 0 ]
EOF
chmod +x "$RIG"/*.sh
docker exec "$NAME" /rig/inner.sh
