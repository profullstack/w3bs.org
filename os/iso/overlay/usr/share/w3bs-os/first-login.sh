#!/bin/sh
# W3BS OS first login: add the official W3BS repository for this user, pinned to its root
# fingerprint (the value registry/install.sh pins). Runs once per user; retried at the next
# login if the machine was offline. Nothing here is secret.
[ "$(id -u)" = 0 ] && exit 0
[ -n "${HOME:-}" ] && [ -w "$HOME" ] || exit 0
marker="$HOME/.config/w3bs/.os-repo-added"
[ -e "$marker" ] && exit 0
command -v w3bs >/dev/null 2>&1 || exit 0
fp=$(cat /usr/share/w3bs-os/FINGERPRINT 2>/dev/null) || exit 0
if w3bs repo info w3bs >/dev/null 2>&1 \
	|| w3bs repo add w3bs https://w3bs.org/repo/ --root-fingerprint "$fp" --yes >/dev/null 2>&1; then
	mkdir -p "${marker%/*}" && : >"$marker"
else
	echo "w3bs-os: could not add the W3BS repository (offline?); run: w3bs repo add w3bs https://w3bs.org/repo/ --root-fingerprint $fp" >&2
fi
exit 0
