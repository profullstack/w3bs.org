#!/bin/sh
# Browser channel end to end: a real headless Chromium with a DevTools port, the broker on the
# mock desktop, and an agent driving a local form by uid through `w3bs-desktop browser call`.
# Then the browser goes away and every browser_* call must fail with browser_unavailable.
#
#   W3BS_DESKTOP_BIN=target/debug/w3bs-desktop CHROME=google-chrome sh tests/rig/browser-cdp.sh
set -u
BIN=${W3BS_DESKTOP_BIN:-w3bs-desktop}
CHROME=${CHROME:-$(command -v google-chrome || command -v chromium || command -v chromium-browser)}
T=$(mktemp -d "${TMPDIR:-/tmp}/browser-cdp-rig.XXXXXX")
pass=0 fail=0
ok() { pass=$((pass + 1)); echo "ok   $1"; }
no() { fail=$((fail + 1)); echo "FAIL $1"; [ -n "${2:-}" ] && printf '%s\n' "$2" | head -20; }
cleanup() { kill "$DPID" "$CPID" "$HPID" 2>/dev/null; wait 2>/dev/null; rm -rf "$T"; }
trap cleanup EXIT INT TERM

mkdir -p "$T/www" "$T/run" "$T/cfg" "$T/state" "$T/chrome"
cat > "$T/www/index.html" <<'HTML'
<!doctype html><title>Rig form</title>
<h1>Sign up</h1>
<label>Name <input id="name"></label>
<label>Plan <select id="plan"><option value="free">Free</option><option value="pro">Pro</option></select></label>
<textarea aria-label="Bio"></textarea>
<button onclick="document.getElementById('out').textContent='Hello '+document.getElementById('name').value+' on '+document.getElementById('plan').value">Submit</button>
<p id="out"></p>
<a href="/two.html">Next page</a>
<shadow-field></shadow-field>
<script>customElements.define('shadow-field', class extends HTMLElement { connectedCallback() { this.attachShadow({mode: 'open'}).innerHTML = '<label>Handle <input id="h"></label>'; } });</script>
HTML
echo '<!doctype html><title>Two</title><h1>Second page</h1>' > "$T/www/two.html"

HPORT=$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])')
(cd "$T/www" && exec python3 -m http.server "$HPORT" --bind 127.0.0.1 >/dev/null 2>&1) &
HPID=$!
"$CHROME" --headless=new --no-sandbox --disable-gpu --no-first-run --remote-debugging-port=0 --user-data-dir="$T/chrome" about:blank >"$T/chrome.log" 2>&1 &
CPID=$!
i=0; while [ ! -s "$T/chrome/DevToolsActivePort" ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
CDP_PORT=$(head -n1 "$T/chrome/DevToolsActivePort" 2>/dev/null)
[ -n "$CDP_PORT" ] || { no "chrome did not open a DevTools port" "$(tail -5 "$T/chrome.log")"; exit 1; }

export W3BS_DESKTOP_RUNTIME_DIR="$T/run" W3BS_DESKTOP_CONFIG_DIR="$T/cfg" W3BS_DESKTOP_STATE_DIR="$T/state"
"$BIN" daemon --adapter mock --enabled-by-default --browser-cdp "http://127.0.0.1:$CDP_PORT" >"$T/daemon.log" 2>&1 &
DPID=$!
i=0; until "$BIN" status >/dev/null 2>&1 || [ $i -gt 50 ]; do sleep 0.1; i=$((i + 1)); done
"$BIN" agents add rig --preset broad >/dev/null || no "agents add"
"$BIN" agents add looker --preset observe >/dev/null || no "agents add observe"
B() { "$BIN" browser call --agent rig "$@" 2>&1; }

out=$("$BIN" browser status --json 2>&1)
echo "$out" | grep -q '"available": true' && ok "browser status: DevTools reachable" || no "browser status" "$out"
out=$("$BIN" capabilities --json 2>&1); echo "$out" | python3 -c 'import json,sys; sys.exit(0 if any(c["capability"]=="browser" and c["availability"]=="available" for c in json.load(sys.stdin)) else 1)' && ok "capabilities lists browser" || no "capabilities lists browser" "$out"

out=$(B browser_new_page "{\"url\": \"http://127.0.0.1:$HPORT/\"}" --json)
PAGE=$(echo "$out" | sed -n 's/.*"page": "\([^"]*\)".*/\1/p' | head -1)
[ -n "$PAGE" ] && echo "$out" | grep -q '"loaded": true' && ok "new_page loads the form" || no "new_page" "$out"
P="\"page\": \"$PAGE\""

snap=$(B browser_snapshot "{$P}")
uid() { echo "$snap" | grep -m1 "$1" | sed -n 's/.*uid=\([0-9]*\).*/\1/p'; }
NAME=$(uid 'textbox "Name"'); PLAN=$(uid 'combobox "Plan"'); BIO=$(uid 'textbox "Bio"'); BTN=$(uid 'button "Submit"'); LINK=$(uid 'link "Next page"')
[ -n "$NAME" ] && [ -n "$PLAN" ] && [ -n "$BTN" ] && [ -n "$LINK" ] && [ -n "$BIO" ] && ok "snapshot has uids for textbox, combobox, button, link" || no "snapshot uids" "$snap"

out=$(B browser_fill "{$P, \"uid\": $NAME, \"value\": \"Ada\"}" --json); echo "$out" | grep -q '"verified": true' && ok "fill input (verified)" || no "fill input" "$out"
out=$(B browser_fill "{$P, \"uid\": $PLAN, \"value\": \"Pro\"}" --json); echo "$out" | grep -q '"selected": "Pro"' && ok "fill select by label" || no "fill select" "$out"
out=$(B browser_fill "{$P, \"uid\": $BIO, \"value\": \"line one\"}" --json); echo "$out" | grep -q '"verified": true' && ok "fill textarea" || no "fill textarea" "$out"
SH=$(uid 'textbox "Handle"')
out=$(B browser_fill "{$P, \"uid\": ${SH:-0}, \"value\": \"rc#1 x\"}" --json); echo "$out" | grep -q '"verified": true' && ok "fill an input inside a shadow root" || no "fill shadow-DOM input" "$out"
out=$(B browser_click "{$P, \"uid\": $BTN}" --json); echo "$out" | grep -q '"x"' && ok "click by uid" || no "click" "$out"
out=$(B browser_wait_for "{$P, \"text\": \"Hello Ada on pro\"}" --json); echo "$out" | grep -q '"found": true' && ok "click ran the page's handler" || no "wait_for after click" "$out"
out=$(B browser_evaluate "{$P, \"expression\": \"document.querySelector('textarea').value\"}" --json); echo "$out" | grep -q '"result": "line one"' && ok "evaluate" || no "evaluate" "$out"

B browser_click "{$P, \"uid\": $NAME}" >/dev/null
B browser_press_key "{$P, \"key\": \"Control+a\"}" >/dev/null
out=$(B browser_type "{$P, \"text\": \"Grace\"}" --json)
v=$(B browser_evaluate "{$P, \"expression\": \"document.getElementById('name').value\"}" --json)
echo "$v" | grep -q '"result": "Grace"' && ok "press_key Control+a then type replaces the text" || no "press_key/type" "$v $out"

out=$(B browser_click "{$P, \"uid\": $LINK}" --json)
out=$(B browser_wait_for "{$P, \"text\": \"Second page\"}" --json); echo "$out" | grep -q '"found": true' && ok "click a link navigates" || no "link click" "$out"
out=$(B browser_navigate "{$P, \"action\": \"back\"}" --json); echo "$out" | grep -q '"title": "Rig form"' && ok "navigate back" || no "navigate back" "$out"
out=$(B browser_click "{$P, \"uid\": $LINK}" --json); B browser_wait_for "{$P, \"text\": \"Second page\"}" >/dev/null; out=$(B browser_click "{$P, \"uid\": $BTN}" --json)
echo "$out" | grep -q 'target_stale' && ok "a uid from the old page is target_stale" || no "stale uid" "$out"
out=$(B browser_navigate "{$P, \"url\": \"file:///etc/passwd\"}" --json); echo "$out" | grep -q denied && ok "file: urls are refused" || no "file url refused" "$out"
"$BIN" browser call browser_screenshot "{$P}" --agent rig --out "$T/page.png" >/dev/null 2>&1
[ "$(head -c 4 "$T/page.png" 2>/dev/null | tail -c 3)" = "PNG" ] && ok "page screenshot (fallback) writes a PNG" || no "screenshot"
out=$("$BIN" browser call --agent looker browser_snapshot "{$P}" 2>&1); echo "$out" | grep -q 'not granted browser' && ok "an agent without the browser grant is denied" || no "grant check" "$out"
"$BIN" audit export 2>/dev/null | grep -q 'browser.fill' && ok "audit records browser ops" || no "audit"
"$BIN" audit export 2>/dev/null | grep -q 'Ada\|Grace\|line one' && no "audit leaked typed text" || ok "audit keeps no typed text"

kill "$CPID"; wait "$CPID" 2>/dev/null
out=$(B browser_snapshot "{$P}" --json); echo "$out" | grep -q 'browser_unavailable' && ok "browser gone: snapshot fails with browser_unavailable" || no "unavailable error" "$out"
out=$(B browser_click "{$P, \"uid\": 1}" --json); echo "$out" | grep -q 'browser_unavailable' && ok "browser gone: acts fail with browser_unavailable" || no "unavailable on act" "$out"

echo "browser-cdp: $pass passed, $fail failed"
[ "$fail" = 0 ]
