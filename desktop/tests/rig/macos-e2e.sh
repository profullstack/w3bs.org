#!/bin/bash
# End-to-end rig for the macOS backend, run on GitHub's macos-latest runner (a logged-in GUI
# session) by .github/workflows/desktop.yml. Every check prints PASS, FAIL or SKIP (with the
# reason, e.g. a TCC permission this process was not given). Exit status: number of FAILs > 0.
#
#   desktop/tests/rig/macos-e2e.sh            (from desktop/, after `cargo build --release`)
set -u
cd "$(dirname "$0")/../.."
B=${B:-$PWD/target/release/w3bs-desktop}
OUT=${OUT:-${RUNNER_TEMP:-/tmp}/adl-mac}
rm -rf "$OUT"; mkdir -p "$OUT/app"
export W3BS_DESKTOP_RUNTIME_DIR=$OUT/run W3BS_DESKTOP_CONFIG_DIR=$OUT/config W3BS_DESKTOP_STATE_DIR=$OUT/state
PASS=0 FAIL=0 SKIP=0
ok() { echo "PASS: $*"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $*"; FAIL=$((FAIL + 1)); }
skp() { echo "SKIP: $*"; SKIP=$((SKIP + 1)); }
note() { echo "  .. $*"; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print($1)"; }
wait_for() { # wait_for SECONDS COMMAND...
    local t=$1; shift
    for _ in $(seq 1 $((t * 10))); do "$@" >/dev/null 2>&1 && return 0; sleep 0.1; done
    return 1
}
state() { "$B" status --json 2>/dev/null | j 'd["state"]'; }
is_state() { [ "$(state)" = "$1" ]; }
PIDS=()
cleanup() {
    for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done
    exec 3>&- 2>/dev/null
}
trap cleanup EXIT

echo "== environment"
sw_vers; uname -m; id -un
swiftc -O -o "$OUT/probe" tests/rig/macos/Probe.swift || { bad "build probe"; exit 1; }
swiftc -O -o "$OUT/TargetApp" tests/rig/macos/TargetApp.swift || { bad "build TargetApp"; exit 1; }
note "probe (compiled Swift, launched from the job shell): $("$OUT/probe" tcc)"
for db in "$HOME/Library/Application Support/com.apple.TCC/TCC.db" "/Library/Application Support/com.apple.TCC/TCC.db"; do
    note "TCC grants in $db (services this layer uses):"
    sudo sqlite3 "$db" "select service, client, auth_value from access where service in ('kTCCServiceAccessibility','kTCCServicePostEvent','kTCCServiceScreenCapture','kTCCServiceListenEvent','kTCCServiceAppleEvents')" 2>&1 | sed 's/^/     /' | head -40
done
note "process tree of this shell:"; ps -o pid,ppid,comm -p $$ -p $PPID | sed 's/^/     /'

echo "== broker"
"$B" daemon --adapter native >"$OUT/daemon.log" 2>&1 &
PIDS+=($!)
wait_for 10 test -S "$OUT/run/control.sock" || { bad "daemon did not start"; cat "$OUT/daemon.log"; exit 1; }
"$B" doctor --json >"$OUT/doctor.json"
python3 - "$OUT/doctor.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
for c in d["checks"]:
    print(f'  doctor: {c["check"]:24} ok={c["ok"]!s:5} {c["detail"]}')
for c in d["capabilities"]:
    if c["availability"] != "unsupported":
        print(f'  capability: {c["capability"]:15} {c["availability"]:20} {c.get("reason", "")}')
PY
perm() { python3 -c "import json; d=json.load(open('$OUT/doctor.json')); print([c['ok'] for c in d['checks'] if c['check']=='$1'][0])"; }
AX=$(perm macos_accessibility) POST=$(perm macos_post_events) SCREEN=$(perm macos_screen_recording)
if [ -n "$AX" ] && [ -n "$POST" ] && [ -n "$SCREEN" ]; then
    ok "doctor reports macOS permissions for the broker: accessibility=$AX post_events=$POST screen_recording=$SCREEN"
else
    bad "doctor has no macOS permission checks"
fi
"$B" agents add ci --preset broad >/dev/null 2>&1 || bad "agents add"
A=(--agent ci)

echo "== menu-bar control"
osa() { osascript -e "$1" 2>&1; }
"$B" tray >"$OUT/tray.log" 2>&1 &
TRAY=$!; PIDS+=($TRAY)
sleep 3
# Find the menu-bar item through Accessibility (System Events). Before macOS 26 a status item is
# `menu bar 2` of its own process; on macOS 26 Control Center draws status items, so look there
# too. Prints the item's title/description, or the errors.
# The status item is an AXMenuExtra in one of the control's own menu bars (menu bar 2 before
# macOS 26, menu bar 1 on macOS 26, where Control Center draws it).
find_item() {
    osa "tell application \"System Events\"
        tell (first process whose unix id is $TRAY)
            repeat with mb in menu bars
                repeat with mbi in menu bar items of mb
                    try
                        if subrole of mbi is \"AXMenuExtra\" then return (name of mbi as text)
                    end try
                end repeat
            end repeat
        end tell
        return \"none\"
    end tell"
}
open_item() { # open_item NAME: click the status item, then the named menu item
    osa "tell application \"System Events\"
        tell (first process whose unix id is $TRAY)
            repeat with mb in menu bars
                repeat with mbi in menu bar items of mb
                    if subrole of mbi is \"AXMenuExtra\" then
                        click mbi
                        delay 0.7
                        set names to name of every menu item of menu 1 of mbi
                        click menu item \"$1\" of menu 1 of mbi
                        return \"clicked '$1' among \" & (names as text)
                    end if
                end repeat
            end repeat
        end tell
        return \"item not found\"
    end tell"
}
if ! kill -0 $TRAY 2>/dev/null; then
    bad "menu-bar control exited: $(cat "$OUT/tray.log")"
else
    note "our process (System Events): $(osa "tell application \"System Events\" to tell (first process whose unix id is $TRAY) to get {count of menu bars, role of every UI element}")"
    note "our menu bar 1 items: $(osa "tell application \"System Events\" to tell (first process whose unix id is $TRAY) to get {name, role, subrole} of every menu bar item of menu bar 1")"
    note "tray log: $(cat "$OUT/tray.log")"
    ITEM=$(find_item)
    note "status item: $ITEM"
    if echo "$ITEM" | grep -q "Agents: Disabled"; then
        ok "menu-bar item exists and says 'Agents: Disabled' ($ITEM)"
        R=$(open_item "Enable agent access")
        note "menu: $R"
        sleep 1
        S=$(state)
        if [ "$S" = enabled ]; then
            ok "Enable from the menu-bar menu enabled access"
            ITEM2=$(find_item)
            echo "$ITEM2" | grep -q "Agents: Enabled" && ok "menu-bar title follows the state ($ITEM2)" || bad "menu-bar title after enable: $ITEM2"
            R=$(open_item "Disable agent access")
            sleep 1
            [ "$(state)" = disabled ] && ok "Disable from the menu-bar menu disabled access" || bad "menu Disable: $R state $(state)"
        else
            bad "menu Enable did not enable (state $S): $R; $(cat "$OUT/tray.log")"
        fi
        "$B" disable >/dev/null 2>&1
    else
        bad "menu-bar item not found through Accessibility ($ITEM) and no position logged"
    fi
fi

echo "== enable from the terminal control"
mkfifo "$OUT/ctl"
"$B" control <"$OUT/ctl" >"$OUT/control.log" 2>&1 &
PIDS+=($!)
exec 3>"$OUT/ctl"
sleep 1
echo enable >&3
wait_for 5 is_state enabled
S=$(state)
ENABLED=no
if [ "$S" = enabled ]; then
    ok "enabled from the terminal control"
    ENABLED=yes
elif [ "$POST" != True ]; then
    ok "enable is refused while macOS denies posting events: $("$B" status 2>&1 | head -1)"
else
    bad "enable failed (state $S): $(tail -5 "$OUT/control.log")"
fi

echo "== target application"
"$OUT/TargetApp" "$OUT/app" >"$OUT/app.log" 2>&1 &
APP=$!; PIDS+=($APP)
wait_for 15 test -f "$OUT/app/ready" || bad "TargetApp did not start: $(cat "$OUT/app.log")"
sleep 1
"$OUT/probe" windows | sed 's/^/  window: /' | head -40
note "geometry: $(cat "$OUT/app/geometry.json")"
geo() { python3 -c "import json; g=json.load(open('$OUT/app/geometry.json')); r=g['$1']; print($2)"; }

"$B" windows list "${A[@]}" --json >"$OUT/windows.json" 2>"$OUT/windows.err"
python3 - "$OUT/windows.json" "$OUT/app/geometry.json" "$APP" >"$OUT/windows.check" <<'PY'
import json, sys
ws = json.load(open(sys.argv[1]))["data"]
g = json.load(open(sys.argv[2]))
mine = [w for w in ws if w["app_id"] in ("TargetApp",) or w["title"].startswith("ADL ")]
def near(w, r):
    b = w["bounds"]
    # Window bounds include the title bar; the content rect sits at its bottom.
    return abs(b["x"] - r["x"]) <= 2 and abs(b["x"] + b["width"] - r["x"] - r["width"]) <= 2 and abs(b["y"] + b["height"] - r["y"] - r["height"]) <= 2 and 0 <= r["y"] - b["y"] <= 60
t = [w for w in mine if near(w, g["target_window"])]
o = [w for w in mine if near(w, g["other_window"])]
focused = [w for w in ws if w["focused"]]
print(t[0]["id"] if t else "", o[0]["id"] if o else "", len(focused), focused[0]["id"] if focused else "", json.dumps(mine))
PY
read -r TID OID NFOC FID MINE <"$OUT/windows.check"
note "windows: target=$TID other=$OID focused=$FID ($NFOC focused) mine=$MINE"
if [ -n "$TID" ] && [ -n "$OID" ]; then ok "windows list has both target windows with bounds matching AppKit's frames (target $TID, other $OID)"; else bad "windows list: target/other not found ($(cat "$OUT/windows.err") $MINE)"; fi
[ "$NFOC" = 1 ] && ok "exactly one focused window ($FID)" || bad "focused windows: $NFOC"
[ "$FID" = "$TID" ] && ok "the focused window is the target" || bad "focused $FID, expected target $TID"
if [ "$SCREEN" = True ]; then
    echo "$MINE" | grep -q '"ADL Target"' && ok "window titles visible (Screen Recording allowed)" || bad "titles missing although Screen Recording is allowed"
else
    echo "$MINE" | grep -q '"ADL Target"' && note "titles visible without Screen Recording" || ok "window titles are empty without Screen Recording, as documented"
fi

if [ "$ENABLED" != yes ]; then
    for c in "typing lands exactly" "unfocused window refused" "mouse move read-back" "click focuses another window" "double click count" "disable during a long type" "input after disable exits 4" "emergency-stop hot key"; do skp "$c: access could not be enabled (posting events not allowed)"; done
else
    echo "== keyboard"
    TEXT='héllo € 😀 ADL-mac 123'
    printf '%s' "$TEXT" >"$OUT/text.txt"
    "$B" keyboard type --window "$TID" --from-file "$OUT/text.txt" "${A[@]}" --json >"$OUT/type.json" 2>&1
    RC=$?
    sleep 0.7
    GOT=$(cat "$OUT/app/target.txt" 2>/dev/null)
    if [ $RC = 0 ] && [ "$GOT" = "$TEXT" ]; then ok "typed text landed exactly: '$GOT'"; else bad "type rc=$RC got '$GOT' expected '$TEXT' ($(cat "$OUT/type.json"))"; fi
    note "HID idle right after typing: $("$OUT/probe" hid) s (large = synthetic events are not counted as hardware input)"

    "$B" keyboard type --window "$OID" --from-file "$OUT/text.txt" "${A[@]}" --json >"$OUT/stale.json" 2>&1
    RC=$?
    if [ $RC != 0 ] && grep -q target_stale "$OUT/stale.json" && [ ! -s "$OUT/app/other.txt" ]; then ok "typing into the unfocused window refused target_stale (rc $RC), nothing landed"; else bad "unfocused type rc=$RC $(cat "$OUT/stale.json") other='$(cat "$OUT/app/other.txt" 2>/dev/null)'"; fi

    "$B" keyboard press --window "$TID" --key Backspace "${A[@]}" >/dev/null 2>&1
    sleep 0.3
    GOT=$(cat "$OUT/app/target.txt")
    [ "$GOT" = "${TEXT%?}" ] && ok "named key (Backspace) deleted one character" || bad "Backspace: got '$GOT'"
    "$B" keyboard shortcut --window "$TID" --keys primary+a "${A[@]}" >/dev/null 2>&1
    printf 'x' >"$OUT/x.txt"
    "$B" keyboard type --window "$TID" --from-file "$OUT/x.txt" "${A[@]}" >/dev/null 2>&1
    sleep 0.3
    GOT=$(cat "$OUT/app/target.txt")
    [ "$GOT" = x ] && ok "shortcut primary+a (Command+A) selected all, typing replaced it" || bad "primary+a then x: got '$GOT'"

    echo "== pointer"
    MX=$(geo other_window 'int(r["x"] + 50)'); MY=$(geo other_window 'int(r["y"] + 100)')
    "$B" mouse move --x "$MX" --y "$MY" "${A[@]}" >/dev/null 2>"$OUT/move.err"
    sleep 0.2
    read -r PX PY <<<"$("$OUT/probe" mouse)"
    if python3 -c "import sys; sys.exit(0 if abs($PX-$MX)<=1 and abs($PY-$MY)<=1 else 1)"; then ok "mouse move read back at ($PX, $PY), asked ($MX, $MY)"; else bad "mouse move: at ($PX, $PY), asked ($MX, $MY) $(cat "$OUT/move.err")"; fi

    "$B" mouse click --window "$OID" --x 200 --y 150 "${A[@]}" --json >"$OUT/click.json" 2>&1
    RC=$?
    sleep 0.7
    KEY=$(cat "$OUT/app/key.txt" 2>/dev/null)
    F2=$("$B" windows list "${A[@]}" --json | j '[w["id"] for w in d["data"] if w["focused"]]')
    if [ $RC = 0 ] && [ "$KEY" = "ADL Other" ] && echo "$F2" | grep -q "'$OID'"; then ok "click focused the other window (AppKit key window '$KEY', broker focused $F2)"; else bad "click rc=$RC key='$KEY' focused=$F2 $(cat "$OUT/click.json")"; fi

    "$B" mouse click --window "$OID" --x 200 --y 150 --count 2 "${A[@]}" >/dev/null 2>&1
    sleep 0.5
    # The other window's content view records clickCount too (same ClickView class).
    CC=$(cat "$OUT/app/clicks.txt" 2>/dev/null)
    [ "$CC" = 2 ] && ok "double click arrives with clickCount 2" || bad "double click: clickCount '$CC'"

    # Back to the target field for the next checks.
    read -r FX FY <<<"$("$B" windows list "${A[@]}" --json | python3 -c "
import json, sys
w = [w for w in json.load(sys.stdin)['data'] if w['id'] == '$TID'][0]['bounds']
f = json.load(open('$OUT/app/geometry.json'))['target_field']
print(int(f['x'] - w['x'] + 30), int(f['y'] - w['y'] + 10))")"
    "$B" mouse click --window "$TID" --x "$FX" --y "$FY" "${A[@]}" >"$OUT/refocus.txt" 2>&1 || note "refocus click: $(cat "$OUT/refocus.txt")"
    sleep 0.5
    note "key window now: $(cat "$OUT/app/key.txt")"
fi

echo "== screen capture"
D=$("$B" layout "${A[@]}" --json | j '[x for x in d["data"]["displays"] if x["primary"]][0]["id"]')
WANT=$("$B" layout "${A[@]}" --json | j "[str(round(x['logical']['width']*x['scale']))+'x'+str(round(x['logical']['height']*x['scale'])) for x in d['data']['displays'] if x['primary']][0]")
note "layout: $("$B" layout "${A[@]}" --json | j 'd["data"]')"
"$B" screen capture --display "$D" --output "$OUT/shot.png" "${A[@]}" --json >"$OUT/shot.json" 2>&1
RC=$?
if [ $RC = 0 ]; then
    GOTSZ=$(python3 -c "import struct; b=open('$OUT/shot.png','rb').read(24); assert b[:8]==b'\x89PNG\r\n\x1a\n'; print('%dx%d' % struct.unpack('>II', b[16:24]))")
    [ "$GOTSZ" = "$WANT" ] && ok "screen capture PNG is the display's pixel size ($GOTSZ)" || bad "capture is $GOTSZ, display pixels $WANT"
elif [ "$SCREEN" != True ] && grep -q "Screen Recording" "$OUT/shot.json"; then
    skp "screen capture: Screen Recording not allowed; refused with: $(j 'd["error"]["message"]' <"$OUT/shot.json")"
else
    bad "screen capture rc=$RC $(cat "$OUT/shot.json")"
fi

echo "== accessibility"
"$B" inspect --window "$TID" "${A[@]}" --json >"$OUT/inspect.json" 2>&1
RC=$?
if [ $RC = 0 ]; then
    FIELD=$(python3 - "$OUT/inspect.json" <<'PY'
import json, sys
def walk(n):
    yield n
    for c in n.get("children", []):
        yield from walk(c)
d = json.load(open(sys.argv[1]))["data"]
f = [n for n in walk(d) if n["role"] == "AXTextField"]
print(json.dumps(f[0]) if f else "")
PY
)
    note "text field node: $FIELD"
    WANTV=$(cat "$OUT/app/target.txt" 2>/dev/null)
    if [ -n "$FIELD" ] && python3 -c "import json,sys; n=json.loads(sys.argv[1]); sys.exit(0 if n.get('value','')==sys.argv[2] and n.get('bounds') else 1)" "$FIELD" "$WANTV"; then ok "inspect returns the AXTextField with its value '$WANTV' and bounds"; else bad "inspect: no matching text field (want value '$WANTV')"; fi
elif [ "$AX" != True ]; then
    skp "inspect: Accessibility not allowed; refused with $(head -c 300 "$OUT/inspect.json")"
else
    bad "inspect rc=$RC $(head -c 500 "$OUT/inspect.json")"
fi

if [ "$ENABLED" = yes ]; then
    echo "== disable during a long type"
    python3 -c "print('a' * 3000, end='')" >"$OUT/long.txt"
    : >"$OUT/app/target.txt"
    "$B" keyboard shortcut --window "$TID" --keys primary+a "${A[@]}" >/dev/null 2>&1
    "$B" keyboard type --window "$TID" --from-file "$OUT/long.txt" "${A[@]}" --json >"$OUT/long.json" 2>&1 &
    TP=$!
    sleep 2
    "$B" disable >"$OUT/disable.txt" 2>&1
    T0=$(date +%s)
    wait $TP; RC=$?
    sleep 1
    N1=$(python3 -c "print(len(open('$OUT/app/target.txt').read()))")
    sleep 1
    N2=$(python3 -c "print(len(open('$OUT/app/target.txt').read()))")
    note "disable: $(cat "$OUT/disable.txt")"
    if [ $RC != 0 ] && [ "$N1" -gt 0 ] && [ "$N1" -lt 3000 ] && [ "$N1" = "$N2" ]; then ok "disable stopped a running type: $N1 of 3000 characters landed, nothing after (rc $RC, $(j 'd["error"]["code"]' <"$OUT/long.json"))"; else bad "long type rc=$RC landed $N1 then $N2 of 3000: $(head -c 300 "$OUT/long.json")"; fi
    "$B" keyboard type --window "$TID" --from-file "$OUT/x.txt" "${A[@]}" >/dev/null 2>&1
    RC=$?
    [ $RC = 4 ] && ok "input after disable exits 4" || bad "input after disable exited $RC"

    echo "== emergency stop"
    echo enable >&3
    wait_for 5 is_state enabled
    if [ "$(state)" = enabled ]; then
        HOW="CGEvent chord from the probe"
        "$OUT/probe" hotkey
        if ! wait_for 3 is_state disabled; then
            HOW="System Events key code 53 using control, option, command"
            osa 'tell application "System Events" to key code 53 using {control down, option down, command down}' >/dev/null
            wait_for 3 is_state disabled
        fi
        # The reason of the last disable is in the audit metadata.
        S=$("$B" audit export --json | python3 -c "import json,sys; d=json.load(sys.stdin); r=[e for e in (d if isinstance(d, list) else d.get('events', [])) if e.get('op')=='disable']; print(r[-1]['meta']['reason'] if r else 'no disable recorded')")
        if [ "$(state)" = disabled ] && echo "$S" | grep -qi "emergency"; then ok "Control+Option+Command+Escape ($HOW) disabled access: $S"; else bad "emergency hot key: $S; tray log: $(tail -3 "$OUT/tray.log")"; fi
    else
        bad "could not re-enable for the emergency-stop check"
    fi
fi

echo "== logs"
note "daemon log tail:"; tail -20 "$OUT/daemon.log" | sed 's/^/     /'
note "tray log tail:"; tail -10 "$OUT/tray.log" | sed 's/^/     /'
echo "== summary: $PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" = 0 ]
