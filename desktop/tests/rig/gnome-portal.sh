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
#
# Characters outside the active layout: Mutter drops keysyms it has no key for, so the GNOME
# backend checks text against the compositor's live keymap and refuses it whole, naming the
# characters. The rig checks the refusal on `us` (nothing typed), then switches the input source to
# `de` and checks that the characters `de` has (ä ö ü ß, and € on AltGr) are typed exactly.
#
# Interlocks (ext-interlocks.sh). Lock: GNOME Shell 50 builds its lock screen (Main.screenShield)
# only when logind is present, and this container has none (see above), so the shield cannot be
# raised here. The rig does what ScreenShield does when it locks: it switches the shell into the
# `unlock-dialog` session mode, which unloads every extension that does not declare that mode
# (the W3BS one included), and checks the broker disables. It also emits the
# org.gnome.ScreenSaver.ActiveChanged(true) signal gnome-shell sends when its shield goes up, and
# checks that disables too. Polkit: gnome-shell's polkit agent cannot register without a logind
# session ("Failed to register AuthenticationAgent"), so pkexec cannot reach it; the rig opens the
# shell's own authentication dialog through that agent's entry point instead (the dialog a real
# request shows), checks input is refused while it is up, and cancels it.
#
# GNOME Shell extension (agentic-desktop@w3bs.org, gnome-extension/ in this workspace). It is
# installed with `w3bs-desktop setup gnome-extension` before the shell starts (GNOME Shell loads
# new extensions only at startup) and is the second native control next to the terminal one.
# With it, `--adapter auto` becomes portal+gnome: window list, geometry, stacking, focus and the
# shell-modal interlock come from the extension's org.w3bs.Desktop.Shell service, which answers
# only the broker it is connected to. Its top-bar indicator is driven through Eval the way a
# person drives the menu (menu items are activated; the broker API is not called directly), and
# the last stage makes it the ONLY native control so disabling the extension must disable the
# broker.
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
{"uuid": "adl-rig@w3bs.org", "name": "ADL test rig", "description": "Test-only: unsafe mode, so the rig can read pointer/focus and stand in for the human via org.gnome.Shell.Eval.", "shell-version": ["50"], "session-modes": ["user", "unlock-dialog"]}
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

# win.py TITLE FIELD < windows-list JSON -> that window's field (bounds as "x y w h"); TITLE - = summary
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
if title == "-":
    print(len(ws), sum(1 for w in ws if w["focused"]), " ".join(sorted(w["title"] for w in ws)))
    sys.exit()
for w in ws:
    if w["title"] == title:
        v = w.get(field)
        if field == "bounds" and v: v = " ".join(str(math.floor(v[k] + 0.5)) for k in ("x", "y", "width", "height"))
        print(v if v is not None else "")
EOF
# appr.py AGENT TEXT: one agent task that types TEXT; on approval_required it prints the
# approval id and retries the IDENTICAL request once a second for 30 s (as in the KDE rig).
cat > "$RIG/appr.py" <<'EOF'
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
    call("lease.acquire", {"task": task})
    r = call("input", batch)
    err = r.get("error")
    if err and err.get("code") in ("approval_required", "busy"):
        if first and err.get("code") == "approval_required":
            print("approval_required", err.get("details", {}).get("approval_id"), flush=True); first = False
        time.sleep(1); continue
    print(json.dumps(r), flush=True)
    call("task.end", {"task": task, "outcome": "completed"})
    sys.exit(0 if not err and r.get("result", {}).get("outcome") == "completed" else 1)
sys.exit("still waiting for approval after 30 s")
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
UUID=agentic-desktop@w3bs.org
w3bs-desktop setup gnome-extension > /root/setup.out 2>&1
gsettings set org.gnome.shell welcome-dialog-last-shown-version '999'
gsettings set org.gnome.desktop.interface enable-hot-corners false   # the virtual pointer starts at 0,0
setsid pipewire >$L/pipewire.log 2>&1 &
sleep 1
setsid wireplumber >$L/wireplumber.log 2>&1 &
setsid gnome-shell --headless --virtual-monitor 1920x1080 --wayland --no-x11 --wayland-display wayland-0 >$L/shell.log 2>&1 &
for i in $(seq 1 100); do [ "$(/rig/eval.sh 'global.context.unsafe_mode' 2>/dev/null)" = true ] && break; sleep 0.3; done
[ "$(/rig/eval.sh 'global.context.unsafe_mode')" = true ] || { echo "gnome-shell did not come up (or the rig extension did not load)"; tail -40 $L/shell.log; exit 1; }
echo "== GNOME Shell extension installed by \`setup gnome-extension\`"
EXT=/root/.local/share/gnome-shell/extensions/$UUID
[ -s $EXT/extension.js ] && [ -s $EXT/metadata.json ] && [ -s $EXT/stylesheet.css ] && gsettings get org.gnome.shell enabled-extensions | grep -q "'$UUID'" \
  && grep -q "log out and back in" /root/setup.out && ok "setup wrote $EXT, enabled it, and said a new login is needed" || bad "setup: $(cat /root/setup.out); $(ls $EXT); $(gsettings get org.gnome.shell enabled-extensions)"
I='Main.panel.statusArea["agentic-desktop"]'
until_() { for i in $(seq 1 ${2:-25}); do eval "$1" && return 0; sleep 0.2; done; return 1; }
until_ '[ "$(/rig/eval.sh "!!$I")" = true ]' && ok "extension loaded at shell start: top-bar indicator present ($(gnome-extensions info $UUID | grep -i state | tr -s ' '))" \
  || bad "no indicator: $(gnome-extensions info $UUID 2>&1 | tr '\n' ' '); $(grep -i -m5 'agentic\|extension' $L/shell.log)"
[ "$(/rig/eval.sh "$I._statusItem.label.text")" = "Status: Broker not running" ] && [ "$(/rig/eval.sh "$I._label.text")" = "Agents: No broker" ] \
  && ok "without a broker: label 'Agents: No broker', status 'Broker not running'" || bad "no-broker view: $(/rig/eval.sh "$I._label.text") / $(/rig/eval.sh "$I._statusItem.label.text")"
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
setsid w3bs-desktop daemon --adapter auto >$L/daemon.log 2>&1 &
sleep 1
w3bs-desktop agents add tester --preset broad >/dev/null
rm -f /root/ctl.fifo; mkfifo /root/ctl.fifo
setsid sh -c 'exec sleep infinity > /root/ctl.fifo' &
setsid w3bs-desktop control < /root/ctl.fifo > $L/control.log 2>&1 &
sleep 1
state() { w3bs-desktop --json status | grep -o '"state": "[a-z]*"' | head -1 | cut -d'"' -f4; }
until_ '[ "$(/rig/eval.sh "$I._statusItem.label.text")" = "Status: Disabled" ]' 50 && ok "extension connected to the broker as a native control: status 'Disabled', label '$(/rig/eval.sh "$I._label.text")'" \
  || bad "extension did not connect: $(/rig/eval.sh "$I._statusItem.label.text")"
w3bs-desktop --json status | grep -q '"adapter": "linux-portal+gnome"' && ok "--adapter auto picked portal+gnome" || bad "adapter: $(w3bs-desktop --json status | grep adapter); $(head -5 $L/daemon.log)"
w3bs-desktop --json doctor > /root/doctor.json
python3 -c 'import json; c=[x for x in json.load(open("/root/doctor.json"))["checks"] if x["check"]=="gnome_shell_extension"]; print(c[0]["ok"], c[0]["detail"]) if c else print("missing")' > /root/doctor.txt
grep -q '^True' /root/doctor.txt && ok "doctor: gnome_shell_extension $(cat /root/doctor.txt)" || bad "doctor: $(cat /root/doctor.txt)"
set +e; gdbus call --session -d org.w3bs.Desktop.Shell -o /org/w3bs/Desktop/Shell -m org.w3bs.Desktop.Shell.GetWindows >/root/stranger.out 2>&1; rc=$?; set -e
[ $rc != 0 ] && grep -q AccessDenied /root/stranger.out && ok "window data refused to a caller that is not the connected broker ($(grep -o 'only the connected W3BS desktop broker may query window data' /root/stranger.out))" || bad "stranger GetWindows rc=$rc $(cat /root/stranger.out)"

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
set +e; printf '%s' "héllo üß €" | W keyboard type --agent tester --stdin >/root/nonascii.out 2>&1; NA_RC=$?; set -e
type_ "ASCII ~!@#%&*()_+{}:<>?"; W keyboard press --agent tester --key Enter >/dev/null
W mouse click --agent tester --x $x2 --y $y2 >/dev/null; sleep 0.3
[ "$(/rig/active.sh)" = T2 ] && ok "click focused T2" || bad "click did not focus T2 (focus: $(/rig/active.sh))"
type_ "into T2"; sleep 0.5
[ "$(sed -n 1p /root/t1.txt)" = "hello from w3bs desktop 123" ] && ok "ASCII typed exactly" || bad "ASCII: $(sed -n 1p /root/t1.txt | od -c | head -2)"
# Mutter maps a keysym only onto a keycode of the active layout (us here) and drops the rest
# ("No keycode found for keyval ... in current group") while the portal call succeeds, so the
# backend refuses such text up front: whole, naming the characters, with nothing typed.
[ $NA_RC != 0 ] && grep -q '"unsupported"' /root/nonascii.out && grep -q "'é' (U+00E9), 'ü' (U+00FC), 'ß' (U+00DF)" /root/nonascii.out \
  && ! grep -q "No keycode found" $L/shell.log && ok "non-ASCII outside the us layout refused before typing: $(grep -o 'has no key for[^.]*' /root/nonascii.out | head -1)" \
  || bad "non-ASCII on us: rc=$NA_RC $(cat /root/nonascii.out); shell.log: $(grep -m2 'No keycode' $L/shell.log)"
l2=$(sed -n 2p /root/t1.txt)
[ "$l2" = "ASCII ~!@#%&*()_+{}:<>?" ] && ok "nothing of the refused text reached T1; shifted ASCII typed exactly" || bad "line 2: $(printf %s "$l2" | od -c | head -3)"
[ "$(cat /root/t2.txt)" = "into T2" ] && ok "second window got only its text" || bad "T2: $(cat /root/t2.txt | od -c | head -2)"

echo "== non-ASCII on a layout that has it"
# The us keymap Mutter builds has EuroSign on a third (AltGr) level, so '€' passes the check on
# us; it must then arrive exactly.
mark2=$(wc -c < /root/t2.txt)
type_ " 5€"; sleep 0.5
got=$(tail -c +$((mark2 + 1)) /root/t2.txt)
[ "$got" = " 5€" ] && ok "on us: '€' typed exactly (the us keymap has EuroSign on AltGr)" || bad "€ on us: $(printf %s "$got" | od -c | head -3)"
gsettings set org.gnome.desktop.input-sources sources "[('xkb', 'de')]"
/rig/eval.sh 'import("resource:///org/gnome/shell/misc/keyboardManager.js").then(m => { global._adlKbm = m.getKeyboardManager(); }); 1' >/dev/null
until_ '[ "$(/rig/eval.sh "global._adlKbm?.currentLayout?.id")" = de ]' 25 || bad "layout did not switch to de: $(/rig/eval.sh "global._adlKbm?.currentLayout?.id")"
sleep 0.5
mark2=$(wc -c < /root/t2.txt)
type_ "äöü ß € zy"; sleep 0.5
set +e; printf '%s' "é" | W keyboard type --agent tester --stdin >/root/de-e.out 2>&1; rc=$?; set -e
sleep 0.3
got=$(tail -c +$((mark2 + 1)) /root/t2.txt)
[ "$got" = "äöü ß € zy" ] && ok "on de: '$got' typed exactly (ß, ä ö ü on their keys, € through AltGr, y/z swapped keys)" || bad "on de: $(printf %s "$got" | od -c | head -3)"
[ $rc != 0 ] && grep -q "'é' (U+00E9)" /root/de-e.out && grep -q German /root/de-e.out && [ "$(tail -c +$((mark2 + 1)) /root/t2.txt)" = "$got" ] \
  && ok "on de: 'é' (a dead-key character, no keysym on any key) refused, layout named: $(grep -o 'layout ([^)]*)' /root/de-e.out | head -1)" || bad "é on de: rc=$rc $(cat /root/de-e.out)"
gsettings set org.gnome.desktop.input-sources sources "[('xkb', 'us')]"
until_ '[ "$(/rig/eval.sh "global._adlKbm?.currentLayout?.id")" = us ]' 25 || bad "layout did not switch back to us"
sleep 0.5
! grep -q "No keycode found" $L/shell.log && ok "Mutter never dropped a keysym (no 'No keycode found' in the shell log)" || bad "shell.log: $(grep -m3 'No keycode' $L/shell.log)"

echo "== screenshot"
set +e; timeout 30 w3bs-desktop --json screen capture --agent tester --display $DISP --output /rig/noshot.png >/root/noshot.out 2>&1; rc=$?; set -e
[ $rc != 124 ] && grep -q 'not pre-authorized' /root/noshot.out && ok "capture without a grant fails fast (rc=$rc): blocked, not pre-authorized" || bad "capture without grant rc=$rc $(cat /root/noshot.out)"
type_ "!"; sleep 0.3
[ "$(tail -c 1 /root/t2.txt)" = "!" ] && ok "input still flows after the refused capture" || bad "input after refused capture: $(tail -c 20 /root/t2.txt)"
flatpak permission-set screenshot screenshot "" yes
W screen capture --agent tester --display $DISP --output /rig/shot.png >/dev/null
file /rig/shot.png | grep -q "PNG image data, 1920 x 1080" && ok "$(file -b /rig/shot.png)" || bad "screenshot: $(file /rig/shot.png)"

. /rig/ext-windows.sh
. /rig/ext-interlocks.sh

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
# Closed for a Close it was asked to do, so the frontend never tells the client. Mutter does report
# the stop to the shell (remote-access handle 'stopped'); the extension lists the live sessions and
# the broker treats its own one vanishing as revocation. GNOME's own UI never takes this path (its
# Stop button goes through Mutter, checked above).
st=$(state); s2=$(stat -c %s /root/t2.txt)
set +e; W mouse click --agent tester --x $x2 --y $y2 >/root/closed-click.out 2>&1; rc1=$?
printf lost | W keyboard type --agent tester --stdin >/root/closed-type.out 2>&1; rc2=$?; set -e
sleep 0.5
[ "$st" = disabled ] && grep -q "permission lost: the GNOME remote desktop session was stopped" /root/w3bs/state/audit/* \
  && ok "impl Session.Close -> broker disabled ($(grep -o 'permission lost: the GNOME remote desktop session was stopped' /root/w3bs/state/audit/* | head -1 | cut -d: -f2-))" || bad "impl Session.Close: broker $st"
[ $rc1 = 4 ] && [ $rc2 = 4 ] && [ "$(stat -c %s /root/t2.txt)" = "$s2" ] && ok "input after the backend-side close: click and type exit 4, nothing delivered" || bad "after close: click rc=$rc1 type rc=$rc2 $(cat /root/closed-type.out); T2 grew $(( $(stat -c %s /root/t2.txt) - s2 ))"

. /rig/ext-panel.sh

/rig/shellshot.sh /rig/final.png || true
echo "== $pass passed, $fail failed, $known known GNOME limitations"
[ $fail = 0 ]
EOF
# ---------------------------------------------------------------- extension: window data
cat > "$RIG/ext-windows.sh" <<'EOF'
echo "== GNOME Shell extension: window list, focus, stacking (org.w3bs.Desktop.Shell)"
W windows list --agent tester > /root/wins.json
set -- $(python3 /rig/win.py - - < /root/wins.json)
[ "$1" = 2 ] && [ "$2" = 1 ] && [ "$3 $4" = "T1 T2" ] && ok "windows list: T1, T2, exactly one focused" || bad "windows list: $(cat /root/wins.json)"
[ "$(python3 /rig/win.py T1 bounds < /root/wins.json)" = "$(/rig/geom.sh T1)" ] && [ "$(python3 /rig/win.py T2 bounds < /root/wins.json)" = "$(/rig/geom.sh T2)" ] \
  && ok "geometry matches Mutter's frame rects ($(python3 /rig/win.py T1 bounds < /root/wins.json) / $(python3 /rig/win.py T2 bounds < /root/wins.json))" || bad "geometry: $(cat /root/wins.json) vs $(/rig/geom.sh T1) / $(/rig/geom.sh T2)"
APP=$(python3 /rig/win.py T1 app_id < /root/wins.json); echo "foot app id: $APP"
T1=$(python3 /rig/win.py T1 id < /root/wins.json); T2=$(python3 /rig/win.py T2 id < /root/wins.json)
[ "$(python3 /rig/win.py T2 focused < /root/wins.json)" = True ] && [ "$(/rig/active.sh)" = T2 ] && ok "T2 (clicked last) is the focused one (id $T2)" || bad "focus: $(cat /root/wins.json)"
before2=$(cat /root/t2.txt)
printf '%s' "to focused T2" | W keyboard type --agent tester --window "$T2" --stdin >/dev/null; sleep 0.5
[ "$(cat /root/t2.txt)" = "${before2}to focused T2" ] && ok "type --window <focused> delivered" || bad "T2: $(cat /root/t2.txt)"
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
printf '%s' "gnome T1" | W keyboard type --agent tester --window "$T1" --stdin >/dev/null; sleep 0.5
[ "$(tail -c 8 /root/t1.txt)" = "gnome T1" ] && ok "type --window T1 after focusing it" || bad "T1: $(tail -c 40 /root/t1.txt)"

echo "== GNOME Shell extension: shell-modal interlock"
/rig/eval.sh 'Main.overview.show(); 1' >/dev/null; sleep 1
mark1=$(wc -c < /root/t1.txt)
set +e; printf '%s' "OVERVIEW" | W keyboard type --agent tester --stdin >/root/ov.out 2>&1; rc=$?; set -e
/rig/eval.sh 'Main.overview.hide(); 1' >/dev/null
until_ '[ "$(/rig/eval.sh "Main.overview.visible")" = false ]' 25 || true; sleep 0.5
[ $rc != 0 ] && grep -q "overview is open" /root/ov.out && [ "$(wc -c < /root/t1.txt)" = "$mark1" ] && ok "input refused while the Activities overview is up: $(grep -o '[A-Za-z ]*overview is open' /root/ov.out | head -1)" || bad "overview interlock rc=$rc $(cat /root/ov.out)"
printf '%s' " back" | W keyboard type --agent tester --window "$T1" --stdin >/dev/null 2>&1 || true; sleep 0.5
[ "$(tail -c 5 /root/t1.txt)" = " back" ] && ok "input flows again once the overview closed" || bad "after overview: $(tail -c 20 /root/t1.txt)"

echo "== restricted agent (app grant enforced against the extension's app ids)"
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
EOF

# ---------------------------------------------------------------- interlocks: polkit dialog, lock
cat > "$RIG/ext-interlocks.sh" <<'EOF'
echo "== interlock: the shell's polkit authentication dialog"
set -- $(/rig/geom.sh T1)
W mouse click --agent tester --x $(( $1 + 15 )) --y $(( $2 + $4 / 2 )) >/dev/null; sleep 0.3
PA='Main.componentManager._allComponents.polkitAgent'
grep -q "Failed to register AuthenticationAgent" $L/shell.log && echo "(gnome-shell's polkit agent did not register: no logind session in this container)"
/rig/eval.sh "$PA._onInitiate(null, 'org.w3bs.rig.check', 'Authentication is required to run the W3BS rig check', 'dialog-password-symbolic', 'rig-cookie', ['root']); 1" >/dev/null
until_ '[ "$(/rig/eval.sh "Main.modalCount")" -gt 0 ]' 25 || true
/rig/shellshot.sh /rig/polkit-dialog.png || true
mark1=$(wc -c < /root/t1.txt)
set +e; printf '%s' "POLKIT" | W keyboard type --agent tester --stdin >/root/pk.out 2>&1; rc=$?
W mouse click --agent tester --x 960 --y 540 >/root/pkc.out 2>&1; rc2=$?; set -e
dlg=$(/rig/eval.sh "$PA._currentDialog ? 'up' : 'none'")
[ "$dlg" = up ] && [ $rc != 0 ] && [ $rc2 != 0 ] && grep -q "system dialog" /root/pk.out && grep -q "system dialog" /root/pkc.out && [ "$(wc -c < /root/t1.txt)" = "$mark1" ] && [ "$(state)" = enabled ] \
  && ok "polkit dialog up (/rig/polkit-dialog.png): type and click refused (exit $rc/$rc2: $(grep -o 'input paused: [^;(]*' /root/pk.out | head -1)), broker stays enabled" \
  || bad "polkit interlock: dialog=$dlg rc=$rc/$rc2 $(cat /root/pk.out) state=$(state)"
/rig/eval.sh "$PA._currentDialog.cancel(); 1" >/dev/null
until_ '[ "$(/rig/eval.sh "Main.modalCount")" = 0 ]' 25 || true
set -- $(/rig/geom.sh T1); W mouse click --agent tester --x $(( $1 + 15 )) --y $(( $2 + $4 / 2 )) >/dev/null; sleep 0.3
printf '%s' " after polkit" | W keyboard type --agent tester --stdin >/dev/null 2>&1 || true; sleep 0.5
[ "$(tail -c 13 /root/t1.txt)" = " after polkit" ] && ok "dialog cancelled -> input flows again" || bad "after polkit: $(tail -c 30 /root/t1.txt)"

echo "== interlock: lock-screen session mode (what ScreenShield does on lock)"
/rig/eval.sh 'Main.sessionMode.pushMode("unlock-dialog"); 1' >/dev/null
until_ '[ "$(state)" = disabled ]' 25 && R=$(tail -n 5 /root/w3bs/state/audit/* | grep -o '"reason":"[^"]*"' | tail -1) || R=
[ "$(/rig/eval.sh "Main.sessionMode.isLocked")" = true ] && [ "$(state)" = disabled ] && ok "shell locked (isLocked=true, extensions unloaded) -> broker disabled: $R" || bad "lock: isLocked=$(/rig/eval.sh "Main.sessionMode.isLocked") state=$(state)"
set +e; printf '%s' "LOCKED" | W keyboard type --agent tester --stdin >/root/lk.out 2>&1; rc=$?; set -e
[ $rc = 4 ] && grep -q disabled /root/lk.out && ok "input while locked: exit 4 disabled" || bad "while locked rc=$rc $(cat /root/lk.out)"
/rig/eval.sh 'Main.sessionMode.popMode("unlock-dialog"); 1' >/dev/null
until_ '[ "$(/rig/eval.sh "$I._statusItem.label.text" 2>/dev/null)" = "Status: Disabled" ]' 50 || bad "extension did not come back after unlock"
enable_ && [ $DIALOG = 0 ] && ok "unlocked, re-enabled by the human (no dialog)" || bad "re-enable after unlock: $(state) dialog=$DIALOG"

echo "== interlock: org.gnome.ScreenSaver.ActiveChanged(true)"
gdbus emit --session --object-path /org/gnome/ScreenSaver --signal org.gnome.ScreenSaver.ActiveChanged true >/dev/null
until_ '[ "$(state)" = disabled ]' 25 && tail -n 5 /root/w3bs/state/audit/* | grep -q '"reason":"session locked"' && ok "ActiveChanged(true) -> broker disabled (session locked)" || bad "ActiveChanged: $(state)"
enable_ && [ $DIALOG = 0 ] || bad "re-enable after ActiveChanged: $(state)"
EOF

# ---------------------------------------------------------------- extension: top-bar native control
cat > "$RIG/panel.py" <<'EOF'
# panel.py URL ACTION: sign in to the control app with a one-time link the way a browser does
# (303 + session cookie), read its state (CSRF token), then POST one action with Origin + CSRF.
# Prints "<http status> <state after>". Also checks that the link is single use and that an
# agent bearer token is refused.
import http.client, json, sys, urllib.parse
url, action = sys.argv[1], sys.argv[2]
u = urllib.parse.urlsplit(url)
def req(method, path, headers={}, body=None):
    c = http.client.HTTPConnection(u.hostname, u.port, timeout=30)
    c.request(method, path, body=body, headers=headers)
    r = c.getresponse(); data = r.read(); c.close()
    return r.status, r.getheader("Set-Cookie"), data
st, cookie, _ = req("GET", u.path + "?" + u.query)
assert st == 303 and cookie and "HttpOnly" in cookie and "SameSite=Strict" in cookie, (st, cookie)
st2, again, _ = req("GET", u.path + "?" + u.query)
assert again is None, "login link worked twice"
cookie = cookie.split(";")[0]
st, _, data = req("GET", "/control/api/state", {"Cookie": cookie})
csrf = json.loads(data)["csrf"]
st, _, data = req("GET", "/control/api/state", {"Authorization": "Bearer agent-token"})
assert st == 403, ("agent bearer", st)
origin = "http://%s:%d" % (u.hostname, u.port)
st, _, data = req("POST", "/control/api/action", {"Cookie": cookie, "Origin": origin, "X-W3BS-CSRF": csrf, "Content-Type": "application/json"}, json.dumps({"action": action}))
_, _, after = req("GET", "/control/api/state", {"Cookie": cookie})
print(st, json.loads(after)["status"]["state"])
EOF
cat > "$RIG/ext-panel.sh" <<'EOF'
echo "== GNOME Shell extension: top-bar native control (the only native control from here on)"
P() { /rig/eval.sh "$I.$1"; }
w3bs-desktop disable >/dev/null 2>&1 || true
pkill -9 -f '^w3bs-desktop control' || true; pkill -f '^sleep infinity' || true; sleep 1
mark() { MARK=$(wc -c < /root/t1.txt); }
since() { tail -c +$((MARK + 1)) /root/t1.txt; }
# panel_enable: activate the menu's Enable item; answer a portal dialog if one appears (as enable_)
panel_enable() {
  [ "$(P _toggleItem.label.text)" = "Enable agent access" ] || return 3
  P '_toggleItem.activate(null), 1' >/dev/null
  for i in $(seq 1 40); do
    case "$(state)" in enabled) return 0 ;; blocked) return 2 ;; esac
    g=$(/rig/geom.sh "Remote Desktop")
    if [ -n "$g" ]; then
      sleep 0.5; set -- $g
      /rig/human-click.sh $(( $1 + $3 - 80 )) $(( $2 + 156 ))
      /rig/human-click.sh $(( $1 + $3 - 43 )) $(( $2 + 22 ))
    fi
    sleep 0.5
  done
  return 1
}
[ "$(state)" = disabled ] && [ "$(P _statusItem.label.text)" = "Status: Disabled" ] && [ "$(P _label.text)" = "Agents: Off" ] && [ "$(P _toggleItem.label.text)" = "Enable agent access" ] \
  && P accessible_name | grep -q "agent access Disabled" && ok "indicator: label 'Agents: Off', status 'Disabled', item 'Enable agent access', a11y '$(P accessible_name)'" \
  || bad "indicator view: $(P _label.text) / $(P _statusItem.label.text) / $(P _toggleItem.label.text) / $(P accessible_name)"
P '_stopItem.label.text' | grep -qx "Stop all agent activity" && P '_quitItem.label.text' | grep -qx "Quit and disable" && ok "menu has Stop all agent activity / Quit and disable" || bad "menu items"
panel_enable && ok "menu Enable -> broker enabled" || bad "panel enable: $(state) $(P _noteItem.label.text)"
until_ '[ "$(P _statusItem.label.text)" = "Status: Enabled" ]' && [ "$(P _label.text)" = "Agents: On" ] && [ "$(P _toggleItem.label.text)" = "Disable agent access" ] \
  && ok "status line 'Status: Enabled', label 'Agents: On', item 'Disable agent access'" || bad "enabled view: $(P _label.text) / $(P _statusItem.label.text)"

echo "== top bar: approval"
set -- $(/rig/geom.sh T1)
W mouse click --agent tester --x $(( $1 + 15 )) --y $(( $2 + $4 / 2 )) >/dev/null; sleep 0.3
w3bs-desktop agents add ap --preset broad --approve-for type >/dev/null 2>&1 || true
mark; python3 /rig/appr.py ap "approved text" > /root/appr.out 2>&1 &
APID=$!
until_ 'grep -q approval_required /root/appr.out' && ok "agent got approval_required" || bad "appr: $(cat /root/appr.out)"
AL='Array.from('"$I"'._approvalItems.values())[0]'
AT() { /rig/eval.sh "$AL?.label.label.text ?? 'none'"; }
until_ 'AT | grep -q "^Approval: ap wants: "' \
  && [ "$(/rig/eval.sh "$AL.approve.label.text")" = "    Approve" ] && [ "$(/rig/eval.sh "$AL.deny.label.text")" = "    Deny" ] \
  && ok "menu shows the approval: $(/rig/eval.sh "$AL.label.label.text"); label '$(P _label.text)'" || bad "approval menu: $(AT)"
[ -z "$(since)" ] && ok "nothing typed before approval" || bad "typed before approval: $(since)"
/rig/eval.sh "$AL.approve.activate(null), 1" >/dev/null
wait $APID && rc=0 || rc=$?
sleep 0.5
[ $rc = 0 ] && [ "$(since)" = "approved text" ] && ok "Approve activated -> identical retry typed: $(since)" || bad "after approve rc=$rc $(cat /root/appr.out) / $(since)"
until_ '[ "$(P _approvalItems.size)" = 0 ]' && ok "approval gone from the menu" || bad "approval still listed"

echo "== top bar: Open control panel…, Agents, Recent activity"
[ "$(P _openItem.label.text)" = "Open control panel…" ] && [ "$(/rig/eval.sh "$I.menu._getMenuItems()[0] === $I._openItem")" = true ] \
  && ok "first menu item: Open control panel…" || bad "open item: $(P _openItem.label.text)"
P 'menu.open(), 1' >/dev/null; sleep 1
AGS=$(/rig/eval.sh "$I._agentsMenu.menu._getMenuItems().map(i => i.label.text).join('|')")
echo "$AGS" | grep -q "tester (broad desktop)|    Revoke tester" && ok "Agents submenu lists the rig agent: $(echo "$AGS" | cut -c1-120)" || bad "agents submenu: $AGS"
ACT=$(/rig/eval.sh "$I._activityMenu.menu._getMenuItems().map(i => i.label.text).join('|')")
echo "$ACT" | grep -q "approvals.decide" && ok "Recent activity shows the approval: $(echo "$ACT" | cut -c1-120)" || bad "activity submenu: $ACT"
P 'menu.close(), 1' >/dev/null
L0=$(cat /root/w3bs/state/audit/* | grep -c '"via":"GNOME panel"' || true)
P '_openItem.activate(null), 1' >/dev/null
until_ '[ "$(cat /root/w3bs/state/audit/* | grep -c "\"via\":\"GNOME panel\"")" -gt "$L0" ]' && ok "Open control panel… minted a one-time link over the panel's native-control connection (audited control.login via GNOME panel)" || bad "panel open: $(P _noteItem.label.text)"
# The same app, signed in from a person's terminal (a pty), disables access.
command -v script >/dev/null || dnf -y install --setopt=install_weak_deps=False /usr/bin/script >$L/dnf-script.log 2>&1 || true
U=$(script -qc "w3bs-desktop control --open --no-browser" /dev/null | grep -o 'http://[^ ]*login?t=[a-z0-9_]*' | tr -d '\r')
R=$(python3 /rig/panel.py "$U" disable 2>&1)
[ "$R" = "200 disabled" ] && until_ '[ "$(P _statusItem.label.text)" = "Status: Disabled" ]' \
  && ok "control app (link from control --open in a terminal): single use, agent bearer refused, Disable -> broker disabled, panel 'Status: Disabled'" || bad "control app disable: $R / $(state)"
set +e; U2=$(w3bs-desktop control --open --no-browser </dev/null 2>&1); rc=$?; set -e
[ $rc = 4 ] && ok "control --open without a terminal is refused (exit 4)" || bad "non-interactive control --open rc=$rc $U2"
panel_enable || bad "re-enable from the panel: $(state)"

echo "== top bar: Disable, Quit and disable"
P '_toggleItem.activate(null), 1' >/dev/null
until_ '[ "$(state)" = disabled ]' && until_ '[ "$(P _statusItem.label.text)" = "Status: Disabled" ]' && ok "menu Disable -> broker disabled, status 'Disabled'" || bad "panel disable: $(state)"
panel_enable || bad "re-enable from the panel: $(state)"
P '_quitItem.activate(null), 1' >/dev/null
until_ '[ "$(state)" = disabled ]' && [ "$(/rig/eval.sh "!!$I")" = true ] && ok "Quit and disable -> broker disabled, indicator stays" || bad "quit: $(state)"

echo "== top bar: disabling the extension while enabled"
panel_enable || bad "re-enable from the panel: $(state)"
gnome-extensions disable $UUID
until_ '[ "$(state)" = disabled ]' && [ "$(/rig/eval.sh "!!$I")" = false ] && tail -n 3 /root/w3bs/state/audit/* | grep -q -E "native control lost|extension went away" \
  && ok "gnome-extensions disable while enabled -> indicator gone, broker disabled ($(tail -n 3 /root/w3bs/state/audit/* | grep -o -E 'native control lost|extension went away' | head -1))" || bad "after extension disable: $(state) indicator=$(/rig/eval.sh "!!$I")"
set +e; printf hi | W keyboard type --agent tester --stdin >/root/noext.out 2>&1; rc=$?; set -e
[ $rc = 4 ] && ok "agent input after the extension went away: exit 4 ($(grep -o '"code": *"[a-z_]*"' /root/noext.out | head -1))" || bad "after extension disable rc=$rc $(cat /root/noext.out)"
gnome-extensions enable $UUID
until_ '[ "$(P _statusItem.label.text 2>/dev/null)" = "Status: Disabled" ]' 50 && ok "re-enabled extension reconnects: $(P _statusItem.label.text)" || bad "reconnect: $(P _statusItem.label.text)"

echo "== top bar: broker gone"
pkill -f '^w3bs-desktop daemon' || true
until_ '[ "$(P _statusItem.label.text)" = "Status: Broker not running" ]' 50 && [ "$(P _label.text)" = "Agents: No broker" ] && [ "$(P _toggleItem.visible)" = false ] \
  && ok "broker stopped -> 'Broker not running', label 'Agents: No broker', no Enable item" || bad "broker gone view: $(P _label.text) / $(P _statusItem.label.text)"
! grep -q "$UUID" $L/shell.log && ok "no errors or warnings from the extension in the shell log" || bad "shell.log: $(grep "$UUID" $L/shell.log | head -5)"
EOF

chmod +x "$RIG"/*.sh
docker exec "$NAME" /rig/inner.sh
