#!/usr/bin/env bash
# Daily: re-sign the official W3BS repository (https://w3bs.org/repo/) so its TUF
# metadata never expires (timestamp lasts 60 days), raise catalog latest_version values to
# each tool's newest GitHub release (`w3bs update` skips installers that are already
# current, so a stale value hides releases), and pick up any catalog/catalog.json changes
# merged to main. Opens a PR and merges it when checks are green.
#
# Installed on dev2 as ~/.local/bin/w3bs-registry-refresh, run from crontab:
#   40 4 * * *  ~/.local/bin/w3bs-registry-refresh >> ~/.local/share/w3bs-publish/refresh.log 2>&1
# Needs: the signing workspace (~/.local/share/w3bs-publish/w3bs-org), gh auth, and
# w3bs-publish at ~/.local/share/w3bs-publish/bin/w3bs-publish.
set -euo pipefail
export PATH="$HOME/.local/bin:$HOME/.local/share/mise/shims:/usr/local/bin:/usr/bin:/bin"

repo="$HOME/src/profullstack/w3bs.org"
wt="$HOME/.local/share/w3bs-publish/refresh-worktree"
branch="chore/registry-refresh-$(date -u +%Y%m%d-%H%M)"
export W3BS_PUBLISH_BIN="$HOME/.local/share/w3bs-publish/bin/w3bs-publish"

echo "== $(date -u +%FT%TZ) registry refresh"
exec 9>"$HOME/.local/share/w3bs-publish/refresh.lock"
flock -n 9 || { echo "another refresh is running"; exit 0; }

git -C "$repo" fetch -q origin
git -C "$repo" worktree remove --force "$wt" 2>/dev/null || true
git -C "$repo" branch -D "$branch" 2>/dev/null || true
git -C "$repo" worktree add -q -b "$branch" "$wt" origin/main
trap 'git -C "$repo" worktree remove --force "$wt" 2>/dev/null || true' EXIT

cd "$wt"
bun install --frozen-lockfile >/dev/null && bun scripts/catalog-versions.mjs --scan || echo "catalog version refresh failed; signing the catalog as it is"
# Publisher-signed releases from w3bs.org/packages/publish (W3BS-SIGN-1), each re-verified.
bun scripts/catalog-signed.mjs || echo "signed release merge failed; signing the catalog without new ones"
package-manager/scripts/publish-manager.sh --refresh
git update-index -q --really-refresh || true
if git diff --quiet -- registry catalog && [ -z "$(git ls-files --others --exclude-standard registry)" ]; then
  echo "registry unchanged"
  exit 0
fi
git add registry catalog
git commit -q -m "chore(registry): re-sign the W3BS repository ($(date -u +%F))

Automated daily refresh: renews TUF timestamp/snapshot/targets expiry, raises catalog
latest_version to each tool's newest GitHub release, and signs any catalog changes from main."
git push -q -u origin "$branch"
pr="$(gh pr create --repo profullstack/w3bs.org --base main --head "$branch" \
  --title "chore(registry): daily re-sign ($(date -u +%F))" \
  --body "Automated by \`w3bs-registry-refresh\` on dev2: renews TUF metadata expiry, refreshes catalog latest_version from GitHub releases, and signs catalog changes merged to main.")"
echo "opened $pr"
sleep 20
gh pr checks "$pr" --watch --interval 30 >/dev/null 2>&1 || true
if gh pr checks "$pr" 2>/dev/null | awk '{print $2}' | grep -qE '^(fail|cancel)'; then
  echo "checks failed on $pr; leaving it open"
  exit 1
fi
gh pr merge "$pr" --squash --delete-branch
echo "merged $pr"
