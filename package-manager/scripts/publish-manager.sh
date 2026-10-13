#!/usr/bin/env bash
# Sign a w3bs CLI release (and the catalog) into the official repository
# served at https://w3bs.org/repo/, and regenerate https://w3bs.org/install.sh.
#
#   package-manager/scripts/publish-manager.sh <version>      # after the w3bs-v<version> release exists
#   package-manager/scripts/publish-manager.sh --refresh      # re-sign before metadata expires
#
# Runs on dev2, where the signing workspace lives (keys never enter git):
#   $W3BS_OFFICIAL_WORKSPACE (default ~/.local/share/w3bs-publish/w3bs-org)
#     keys/*.pk8   backed up in the team vault profullstack w3bs-org--signing
# Output lands in registry/ of this checkout; commit it and open a PR.
set -euo pipefail

here="$(cd "$(dirname "$0")/../.." && pwd)"
ws="${W3BS_OFFICIAL_WORKSPACE:-$HOME/.local/share/w3bs-publish/w3bs-org}"
pub="${W3BS_PUBLISH_BIN:-w3bs-publish}"
platforms=(x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-apple-darwin aarch64-apple-darwin)

[ -f "$ws/repo/metadata/1.root.json" ] || { echo "no signing workspace at $ws (run: $pub init $ws)" >&2; exit 1; }

if [ "${1:-}" != "--refresh" ]; then
  v="${1:?usage: publish-manager.sh <version>|--refresh}"
  dl="$ws/downloads/$v"
  mkdir -p "$dl"
  gh release download "w3bs-v$v" --repo profullstack/w3bs.org --dir "$dl" --clobber \
    --pattern "w3bs-$v-*.tar.gz" --pattern SHA256SUMS
  (cd "$dl" && grep " w3bs-$v-" SHA256SUMS | sha256sum -c -)
  args=()
  for p in "${platforms[@]}"; do args+=(--artifact "$p=$dl/w3bs-$v-$p.tar.gz"); done
  "$pub" add "$ws" --id w3bs.org/w3bs --version "$v" "${args[@]}" --command w3bs=bin/w3bs \
    --description "w3bs: the W3BS command line (packages, resources and more)" --license "MIT OR Apache-2.0"
  echo "$v" > "$ws/LATEST"
fi
v="$(cat "$ws/LATEST")"
dl="$ws/downloads/$v"

"$pub" catalog "$ws" "$here/catalog/catalog.json"
"$pub" build "$ws" --timestamp-days 60 --snapshot-days 90

# Signed JSON goes into the site; artifacts stay on GitHub releases (the site redirects).
mkdir -p "$here/registry/repo"
rsync -a --delete --exclude 'targets/artifacts/' "$ws/repo/" "$here/registry/repo/"

fp="$("$pub" fingerprint "$ws")"
sums=()
for p in "${platforms[@]}"; do
  sums+=(--manager-sha256 "$p=$(grep " w3bs-$v-$p.tar.gz" "$dl/SHA256SUMS" | cut -d' ' -f1)")
done
"$pub" installer --repo-alias w3bs --repo-url https://w3bs.org/repo/ --repo-fingerprint "$fp" \
  --manager-repo-url https://w3bs.org/repo/ --manager-fingerprint "$fp" \
  --manager-version "$v" "${sums[@]}" -o "$here/registry/install.sh"
printf '%s\n' "$fp" > "$here/registry/FINGERPRINT"
echo "signed w3bs $v; root $fp; registry/ updated"
