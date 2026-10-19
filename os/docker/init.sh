#!/bin/sh
# First start of a W3BS OS container: add the official W3BS repository, pinned to its root
# fingerprint (the same value registry/install.sh pins), for whichever home this user has,
# then run the command. Nothing here is secret; machine identity is created on first use.
W3BS_REPO_FINGERPRINT=$(cat /usr/share/w3bs-os/FINGERPRINT)
if ! w3bs repo info w3bs >/dev/null 2>&1; then
  w3bs repo add w3bs https://w3bs.org/repo/ --root-fingerprint "$W3BS_REPO_FINGERPRINT" --yes >/dev/null 2>&1 \
    || echo "w3bs-os: could not add the W3BS repository (offline?); run: w3bs repo add w3bs https://w3bs.org/repo/ --root-fingerprint $W3BS_REPO_FINGERPRINT" >&2
fi
exec "$@"
