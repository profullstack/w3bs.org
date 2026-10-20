#!/bin/sh
# End to end on real Hyprland: the Agentic Desktop Layer's linux-hyprland backend in the rig.
#
#   cargo build -p w3bs-desktop && desktop/tests/rig/hyprland.sh && desktop/tests/rig/hyprland-e2e.sh
#
# Copies the built w3bs-desktop into the running rig, starts a foot window and a broker with
# --adapter auto, and checks: backend chosen, windows and layout, a 1920x1080 capture, Unicode
# typing landing exactly, absolute pointer motion, a click.
set -eu
NAME=${NAME:-w3bs-hyprland-rig}
BIN=${W3BS_DESKTOP_BIN:-${CARGO_TARGET_DIR:-$(dirname "$0")/../../target}/debug/w3bs-desktop}
[ -x "$BIN" ] || { echo "build first: cargo build -p w3bs-desktop (or set W3BS_DESKTOP_BIN)" >&2; exit 2; }
docker cp "$BIN" "$NAME":/usr/local/bin/w3bs-desktop >/dev/null
docker exec -i "$NAME" sh -s <<'IN'
# Runs inside the rig: the Agentic Desktop Layer against real Hyprland.
. /run/user/0/rig.env
export W3BS_DESKTOP_RUNTIME_DIR=/tmp/adl/run W3BS_DESKTOP_CONFIG_DIR=/tmp/adl/cfg XDG_STATE_HOME=/tmp/adl/state
rm -rf /tmp/adl; mkdir -p /tmp/adl/run /tmp/adl/cfg /tmp/adl/state; chmod 700 /tmp/adl/run
pkill foot; rm -f /tmp/typed.txt
foot -T T1 sh -c 'stty -echo; cat > /tmp/typed.txt' >/dev/null 2>&1 &
for i in $(seq 50); do hyprctl -j clients | grep -q '"T1"' && break; sleep 0.2; done
w3bs-desktop daemon --adapter auto --enabled-by-default > /tmp/adl/daemon.log 2>&1 &
sleep 1.5
fail=0; check() { if [ "$1" = ok ]; then echo "PASS $2"; else echo "FAIL $2"; fail=1; fi; }
w3bs-desktop agents add bot --preset broad >/dev/null 2>&1
w3bs-desktop capabilities 2>&1 | grep -q '"linux-hyprland"' && check ok "backend is linux-hyprland" || { check no "backend"; w3bs-desktop capabilities | head -20; cat /tmp/adl/daemon.log; }
W=$(w3bs-desktop windows list --agent bot 2>&1)
echo "$W" | grep -q '"app_id": "foot"' && check ok "windows: foot listed" || { check no "windows"; echo "$W"; }
WID=$(echo "$W" | sed -n 's/.*"id": "\(0x[0-9a-f]*\)".*/\1/p' | head -1)
w3bs-desktop layout --agent bot 2>&1 | grep -q '"HEADLESS-1"' && check ok "layout: HEADLESS-1" || check no "layout"
w3bs-desktop screen capture --agent bot --display HEADLESS-1 --output /tmp/adl/shot.png >/dev/null 2>&1 && file /tmp/adl/shot.png | grep -q "1920 x 1080" && check ok "capture 1920x1080 PNG" || check no "capture"
# Every US key (both levels), accents, and more distinct non-US symbols (20) than a US keymap has
# empty keycodes (14), so the typing-in-parts path runs. With W3BS DE's media-key bindings loaded,
# a letter on a media keycode would vanish.
cat > /tmp/adl/text.txt <<'TXT'
hello W3BS: héllo wörld ✓ 日本
abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ 0123456789
!@#$%^&*()_+-=[]{};':",./<>?\|`~
一二三四五六七八九十百千万円年月日時分秒
TXT
w3bs-desktop keyboard type --agent bot --window "$WID" --from-file /tmp/adl/text.txt > /tmp/adl/type.out 2>&1 || cat /tmp/adl/type.out
sleep 1
if [ "$(sha256sum < /tmp/typed.txt)" = "$(sha256sum < /tmp/adl/text.txt)" ]; then check ok "typed 4 lines exactly (ASCII, shifted, accents, 20 CJK)"; else check no "typed text differs"; od -c /tmp/typed.txt | head; fi
w3bs-desktop mouse move --agent bot --x 700 --y 400 >/dev/null 2>&1
pos=$(hyprctl -j cursorpos)
echo "$pos" | grep -q '"x": 700' && echo "$pos" | grep -q '"y": 400' && check ok "absolute move -> $pos" || check no "move: $pos"
w3bs-desktop mouse click --agent bot --window "$WID" --x 300 --y 300 > /tmp/adl/click.out 2>&1 && check ok "click delivered" || { check no click; cat /tmp/adl/click.out; }
echo "--- daemon log"; tail -5 /tmp/adl/daemon.log
exit $fail
IN
