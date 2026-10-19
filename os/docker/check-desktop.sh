#!/bin/sh
# The §20 acceptance task for the desktop image (docs/os-prd.md), with the same calls as
# desktop/tests/rig/headless-kde.sh: look, move, click Konsole, type a command that writes a
# file, screenshot at 1920x1080, then disable and confirm the agent is refused. On a Linux host
# with vkms loaded:
#   docker run -d --name w3bs-os-check --device /dev/dri/cardN --group-add video --shm-size 512m \
#     -e W3BS_DESKTOP_ENABLED=1 ghcr.io/profullstack/w3bs-os:desktop
#   docker cp os/docker/check-desktop.sh w3bs-os-check:/tmp/ && docker exec w3bs-os-check sh /tmp/check-desktop.sh
# Without W3BS_DESKTOP_ENABLED=1 every agent step must fail: access starts off.
pass=0; fail=0
ok()  { pass=$((pass+1)); echo "PASS: $*"; }
bad() { fail=$((fail+1)); echo "FAIL: $*"; }
D() { w3bs-desktop --json "$@"; }
mcp() {
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"check","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":${2:-{\}}}}" \
    | w3bs-desktop mcp serve --stdio --agent ci 2>&1 | tail -1
}
rm -f /tmp/typed.txt
w3bs-desktop enable </dev/null >/dev/null 2>&1
w3bs-desktop agents add ci --preset broad >/dev/null 2>&1 && ok "agent ci registered" || ok "agent ci already registered"
w3bs-desktop drive always-allow on >/dev/null 2>&1 || true
DISP=$(mcp desktop_layout | grep -o 'stream-[0-9]*' | head -1)
[ -n "$DISP" ] && ok "MCP desktop_layout: display $DISP" || bad "layout: $(mcp desktop_layout | head -c 300)"
WIN=$(mcp desktop_windows)
echo "$WIN" | grep -qi konsole && ok "MCP desktop_windows sees Konsole" || bad "windows: $(echo "$WIN" | head -c 300)"
D mouse move --agent ci --x 960 --y 540 --display "$DISP" >/dev/null && ok "mouse move" || bad "mouse move"
D mouse click --agent ci --x 960 --y 540 --display "$DISP" >/dev/null && ok "mouse click" || bad "mouse click"
printf 'echo typed-by-an-agent > /tmp/typed.txt\n' | D keyboard type --agent ci --stdin >/dev/null && ok "keyboard type" || bad "keyboard type"
sleep 1
[ "$(cat /tmp/typed.txt 2>/dev/null)" = typed-by-an-agent ] && ok "the typed command ran in Konsole" || bad "no /tmp/typed.txt"
D screen capture --agent ci --display "$DISP" --output /tmp/shot.png >/dev/null
[ -s /tmp/shot.png ] && ok "screenshot written ($(wc -c </tmp/shot.png) bytes)" || bad "screenshot"
SIZE=$(od -An -tu1 -j16 -N8 /tmp/shot.png | awk '{print ($1*16777216+$2*65536+$3*256+$4) "x" ($5*16777216+$6*65536+$7*256+$8)}')
[ "$SIZE" = 1920x1080 ] && ok "display is 1920x1080" || bad "display is $SIZE"
w3bs-desktop disable >/dev/null
printf x | D keyboard type --agent ci --stdin >/dev/null 2>&1 && bad "typed while disabled" || ok "agent refused after disable"
echo "RESULT: $pass passed, $fail failed"
