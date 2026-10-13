#!/bin/bash
# Installer end to end on macOS (GitHub's macos-latest runner, a logged-in GUI session): runs
# packaging/install.sh against a locally built release archive, then checks what a person gets.
#
#   tests/installer/macos.sh ARCHIVE VERSION      (from desktop/)
#
# Prints PASS/FAIL per check; exits 1 if anything failed. Logs land in $OUT.
set -u
cd "$(dirname "$0")/../.." || exit 1
ARCHIVE=$1 VERSION=$2
OUT=${OUT:-${RUNNER_TEMP:-$HOME/tmp}/installer-mac}
rm -rf "$OUT"; mkdir -p "$OUT"
PASS=0 FAIL=0
ok() { echo "PASS: $*"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $*"; FAIL=$((FAIL + 1)); }
check() { local msg=$1; shift; if "$@"; then ok "$msg"; else bad "$msg"; fi; }
wait_for() { # wait_for SECONDS COMMAND...
    local t=$1; shift
    for _ in $(seq 1 $((t * 4))); do "$@" >/dev/null 2>&1 && return 0; sleep 0.25; done
    return 1
}
B=$HOME/.local/bin/w3bs-desktop
U=$(id -u)
LA=$HOME/Library/LaunchAgents
LOGS=$HOME/Library/Logs/w3bs-desktop
SHA=$(shasum -a 256 "$ARCHIVE" | cut -d' ' -f1)
running() { launchctl print "gui/$U/$1" 2>/dev/null | grep -q 'state = running'; }
job_pid() { launchctl print "gui/$U/$1" 2>/dev/null | awk '$1 == "pid" && $2 == "=" { print $3; exit }'; }
state() { "$B" status --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["state"])'; }
is_disabled() { [ "$(state)" = disabled ]; }
do_install() { W3BS_DESKTOP_ARCHIVE=$ARCHIVE W3BS_DESKTOP_SHA256=$SHA sh packaging/install.sh; }
save_logs() { cp -R "$LOGS" "$OUT/logs-$1" 2>/dev/null; launchctl print "gui/$U/org.w3bs.desktop.broker" >"$OUT/broker-$1.launchctl.txt" 2>&1; launchctl print "gui/$U/org.w3bs.desktop.menubar" >"$OUT/menubar-$1.launchctl.txt" 2>&1; }

echo "== environment"; sw_vers; uname -m; id -un; launchctl print "gui/$U" >/dev/null 2>&1 && echo "gui/$U domain present"

echo "== refusals"
W3BS_DESKTOP_ARCHIVE=$ARCHIVE W3BS_DESKTOP_SHA256=0000 sh packaging/install.sh >"$OUT/mismatch.txt" 2>&1
check "checksum mismatch is refused" [ $? -ne 0 ]
check "nothing installed after a refused archive" [ ! -e "$B" ]
grep -q 'checksum mismatch' "$OUT/mismatch.txt" && ok "mismatch message names the checksum" || bad "mismatch message: $(cat "$OUT/mismatch.txt")"

echo "== install"
do_install >"$OUT/install.txt" 2>&1
rc=$?; cat "$OUT/install.txt"
check "installer exits 0" [ "$rc" -eq 0 ]
check "binary installed at ~/.local/bin/w3bs-desktop" [ -x "$B" ]
check "w3bs-desktop --version is $VERSION" sh -c "'$B' --version | grep -q ' $VERSION\$'"
for l in broker menubar; do
    check "LaunchAgent org.w3bs.desktop.$l.plist written and valid" plutil -lint "$LA/org.w3bs.desktop.$l.plist"
done
check "broker plist: daemon --adapter auto, RunAtLoad, KeepAlive on crash" sh -c "plutil -convert json -o - '$LA/org.w3bs.desktop.broker.plist' | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d[\"ProgramArguments\"][1:]==[\"daemon\",\"--adapter\",\"auto\",\"--mcp-http\",\"127.0.0.1:7801\"] and d[\"RunAtLoad\"] and d[\"KeepAlive\"]=={\"SuccessfulExit\":False}'"
check "menubar plist: tray, RunAtLoad, no KeepAlive" sh -c "plutil -convert json -o - '$LA/org.w3bs.desktop.menubar.plist' | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d[\"ProgramArguments\"][1]==\"tray\" and d[\"RunAtLoad\"] and \"KeepAlive\" not in d'"
check "installer printed the Accessibility + Screen Recording onboarding" sh -c "grep -q 'Accessibility' '$OUT/install.txt' && grep -q 'Screen Recording' '$OUT/install.txt' && grep -q 'doctor --prompt' '$OUT/install.txt'"
check "installer never enables access" sh -c "! grep -qi 'enable.confirm' packaging/install.sh"
check "launchctl: org.w3bs.desktop.broker is running" wait_for 20 running org.w3bs.desktop.broker
BPID=$(job_pid org.w3bs.desktop.broker)
check "broker (pid $BPID) is a launchd child running ~/.local/bin/w3bs-desktop daemon" sh -c "[ \"\$(ps -o ppid= -p '$BPID' | tr -d ' ')\" = 1 ] && ps -o command= -p '$BPID' | grep -q '^$B daemon --adapter auto --mcp-http 127.0.0.1:7801'"
check "w3bs-desktop status answers Disabled through the launchd broker" wait_for 20 is_disabled
"$B" status --json >"$OUT/status.json" 2>&1; cat "$OUT/status.json"
check "launchctl: org.w3bs.desktop.menubar is running" wait_for 20 running org.w3bs.desktop.menubar
check "menu-bar process (w3bs-desktop tray) is running" wait_for 10 pgrep -f "$B tray"
check "menu bar attached to the broker as the native control" wait_for 30 grep -q 'menu bar: ready' "$LOGS/menubar.log"
save_logs install

echo "== crash restart"
kill -9 "$BPID" 2>/dev/null
check "broker killed (SIGKILL) is restarted by launchd (KeepAlive on crash)" wait_for 30 sh -c "p=\$(launchctl print gui/$U/org.w3bs.desktop.broker 2>/dev/null | awk '\$1 == \"pid\" { print \$3; exit }'); [ -n \"\$p\" ] && [ \"\$p\" != '$BPID' ]"
check "restarted broker answers Disabled" wait_for 20 is_disabled
check "menu bar lost its broker and stayed quit (no KeepAlive)" wait_for 15 sh -c "! pgrep -f '$B tray'"

echo "== re-install over a running install"
do_install >"$OUT/reinstall.txt" 2>&1
check "re-install exits 0" [ $? -eq 0 ]
check "broker running again after re-install" wait_for 20 running org.w3bs.desktop.broker
check "status answers Disabled after re-install" wait_for 20 is_disabled
check "menu bar running again after re-install" wait_for 20 pgrep -f "$B tray"
save_logs reinstall

echo "== uninstall"
sh packaging/install.sh --uninstall >"$OUT/uninstall.txt" 2>&1
rc=$?; cat "$OUT/uninstall.txt"
check "uninstall exits 0" [ "$rc" -eq 0 ]
check "broker job unloaded" sh -c "for i in \$(seq 1 50); do launchctl print gui/$U/org.w3bs.desktop.broker >/dev/null 2>&1 || exit 0; sleep 0.2; done; exit 1"
check "menubar job unloaded" sh -c "! launchctl print gui/$U/org.w3bs.desktop.menubar >/dev/null 2>&1"
check "LaunchAgent plists removed" sh -c "[ ! -e '$LA/org.w3bs.desktop.broker.plist' ] && [ ! -e '$LA/org.w3bs.desktop.menubar.plist' ]"
check "binary removed" [ ! -e "$B" ]
check "no w3bs-desktop processes left" wait_for 10 sh -c "! pgrep -f '$B'"
check "status now fails (no broker)" sh -c "! '$HOME/.local/bin/w3bs-desktop' status >/dev/null 2>&1"

echo "== $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
