#!/bin/sh
# Installer end to end on Alpine Linux (musl, no systemd), the base of postmarketOS and W3BS OS:
# runs packaging/install.sh against a locally built musl release archive in a fresh HOME, then
# checks what a person gets. Run it inside an Alpine container, e.g. from desktop/:
#
#   docker run --rm -v "$PWD:/w" -w /w alpine:3.24 sh tests/installer/alpine.sh ARCHIVE VERSION
#
# POSIX sh (busybox). Prints PASS/FAIL per check; exits 1 if anything failed.
set -u
cd "$(dirname "$0")/../.." || exit 1
ARCHIVE=$1 VERSION=$2
PASS=0 FAIL=0
ok() { echo "PASS: $*"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $*"; FAIL=$((FAIL + 1)); }
check() { msg=$1; shift; if "$@"; then ok "$msg"; else bad "$msg"; fi; }

T=$(mktemp -d)
export HOME="$T/home" XDG_RUNTIME_DIR="$T/run"
mkdir -p "$HOME" "$XDG_RUNTIME_DIR" && chmod 0700 "$XDG_RUNTIME_DIR"
unset W3BS_LIBC WAYLAND_DISPLAY DISPLAY XDG_CURRENT_DESKTOP
B=$HOME/.local/bin/w3bs-desktop
SHA=$(sha256sum "$ARCHIVE" | cut -d' ' -f1)
arch=$(uname -m); [ "$arch" = arm64 ] && arch=aarch64

echo "== environment"; cat /etc/alpine-release; uname -m; ldd --version 2>&1 | head -1

echo "== refusals"
W3BS_DESKTOP_ARCHIVE=$ARCHIVE W3BS_DESKTOP_SHA256=0000 sh packaging/install.sh >"$T/mismatch.txt" 2>&1
check "checksum mismatch is refused" [ $? -ne 0 ]
check "nothing installed after a refused archive" [ ! -e "$B" ]

echo "== install"
W3BS_DESKTOP_ARCHIVE=$ARCHIVE W3BS_DESKTOP_SHA256=$SHA sh packaging/install.sh >"$T/install.txt" 2>&1
rc=$?; cat "$T/install.txt"
check "installer exits 0" [ "$rc" -eq 0 ]
check "installer picked $arch-unknown-linux-musl" grep -q "for $arch-unknown-linux-musl from" "$T/install.txt"
check "binary installed at ~/.local/bin/w3bs-desktop" [ -x "$B" ]
check "w3bs-desktop --version is $VERSION" sh -c "'$B' --version | grep -q ' $VERSION\$'"
check "binary is static (musl's ldd: not a dynamic program)" sh -c "ldd '$B' 2>&1 | grep -q 'Not a valid dynamic program'"
check "no systemd: broker autostart entry written" grep -q "^Exec=$B daemon --adapter auto --mcp-http 127.0.0.1:7801\$" "$HOME/.config/autostart/w3bs-desktop-broker.desktop"
check "tray autostart entry written" grep -q "^Exec=$B tray --wait-for-broker 60\$" "$HOME/.config/autostart/w3bs-desktop-tray.desktop"
check "installer printed the apk add line for the portal and PipeWire" grep -q 'apk add.*xdg-desktop-portal-kde.*pipewire' "$T/install.txt"
check "installer installs no system packages (it only prints the line)" sh -c "! grep -Eq '^[[:space:]]*(doas |sudo )?apk (add|-)' packaging/install.sh"

echo "== broker runs on musl (mock adapter, starts Disabled)"
"$B" daemon --adapter mock >"$T/daemon.log" 2>&1 &
DPID=$!
n=0; until "$B" status --json >"$T/status.json" 2>/dev/null || [ $n -ge 40 ]; do sleep 0.25; n=$((n + 1)); done
cat "$T/status.json"
check "status answers Disabled" grep -q '"state": *"disabled"' "$T/status.json"
kill "$DPID" 2>/dev/null; wait "$DPID" 2>/dev/null

echo "== glibc override"
W3BS_LIBC=gnu W3BS_DESKTOP_ARCHIVE=$ARCHIVE W3BS_DESKTOP_SHA256=$SHA W3BS_DESKTOP_NO_SERVICE=1 sh packaging/install.sh >"$T/gnu.txt" 2>&1
check "W3BS_LIBC=gnu selects the glibc triple" grep -q "for $arch-unknown-linux-gnu from" "$T/gnu.txt"

rm -rf "$T"
echo "== $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
