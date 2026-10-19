#!/bin/sh
# Regenerate the SSHSIG fixtures used by w3bs-core's sshsig tests (W3BS-SIGN-1).
#
# Every signature here is made by the real `ssh-keygen -Y sign` (OpenSSH 8.2+), so the
# verifier is tested against OpenSSH's output, not against itself. Hardware-key (sk-*)
# signatures need a FIDO authenticator, so those come from OpenSSH's own test vectors
# (regress/unittests/sshsig/testdata, made with its sk-dummy provider) under openssh/.
#
#   sh regen.sh            # new keys, new signatures
#   sh regen.sh --openssh  # also re-download OpenSSH's sk vectors
set -eu
cd "$(dirname "$0")"

msg() { printf '%s' "$1"; }

rm -f ed25519* ecdsa* rsa* release.jcs* key-message*
ssh-keygen -q -t ed25519 -N '' -C fixture-ed25519 -f ed25519
ssh-keygen -q -t ecdsa -b 256 -N '' -C fixture-ecdsa -f ecdsa
ssh-keygen -q -t rsa -b 2048 -N '' -C fixture-rsa -f rsa

# A release statement's JCS bytes (no trailing newline) and the key-registration message.
msg '{"created_at":"2026-10-19T20:00:00Z","description":"fixture","install":"curl -fsSL https://example.com/install.sh | sh","installer":{"sha256":"0000000000000000000000000000000000000000000000000000000000000000","url":"https://example.com/install.sh"},"name":"tool","package":"example.com/tool","type":"w3bs-release/1","version":"1.0.0"}' > release.jcs.json
msg 'w3bs-key:alice:c0ffee' > key-message.txt

for k in ed25519 ecdsa rsa; do
  ssh-keygen -q -Y sign -n w3bs-release@w3bs.org -f "$k" release.jcs.json
  mv release.jcs.json.sig "release.$k.sig"
done
ssh-keygen -q -Y sign -n w3bs-key@w3bs.org -f ed25519 key-message.txt
mv key-message.txt.sig key-message.ed25519.sig
# sha256 instead of the default sha512.
ssh-keygen -q -Y sign -n w3bs-release@w3bs.org -O hashalg=sha256 -f ecdsa release.jcs.json
mv release.jcs.json.sig release.ecdsa-sha256.sig
# A git commit signature: right key, wrong namespace.
ssh-keygen -q -Y sign -n git -f ed25519 release.jcs.json
mv release.jcs.json.sig release.ed25519-git.sig

ssh-keygen -lf ed25519.pub | awk '{print $2}' > ed25519.fp
ssh-keygen -lf ecdsa.pub | awk '{print $2}' > ecdsa.fp
rm -f ed25519 ecdsa rsa # private halves are not needed by the tests

if [ "${1:-}" = "--openssh" ]; then
  base=https://raw.githubusercontent.com/openssh/openssh-portable/master/regress/unittests/sshsig/testdata
  mkdir -p openssh
  for f in ecdsa_sk.pub ecdsa_sk.sig ed25519_sk.pub ed25519_sk.sig namespace signed-data; do
    curl -fsSL -o "openssh/$f" "$base/$f"
  done
fi
