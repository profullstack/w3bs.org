#!/bin/sh
# End-to-end check of `w3bs os status|update|upgrade` on a real Alpine machine in Docker:
# start alpine:3.23 as a W3BS OS docker-server, dist-upgrade it to v3.24 over the network,
# check nothing is left to upgrade, run `w3bs os update`, then roll the repositories back.
#
#   package-manager/tests/os-upgrade-e2e.sh path/to/w3bs-musl
#
# The binary must be a musl build (Alpine has no glibc). Needs docker, jq and network access
# to dl-cdn.alpinelinux.org. FROM/TO pick the branches (default v3.23 -> v3.24).
set -eu

BIN=${1:-${W3BS_BIN:-}}
FROM=${FROM:-3.23}
TO=${TO:-3.24}
[ -n "$BIN" ] && [ -x "$BIN" ] || { echo "usage: $0 path/to/w3bs-musl" >&2; exit 2; }
command -v jq >/dev/null || { echo "needs jq" >&2; exit 2; }

C="w3bs-os-e2e-$$"
trap 'docker rm -f "$C" >/dev/null 2>&1 || true' EXIT INT TERM
ok() { echo "ok   - $*"; }
die() { echo "FAIL - $*" >&2; exit 1; }
run() { docker exec "$C" "$@"; }

docker run -d --name "$C" "alpine:$FROM" sleep 3600 >/dev/null
docker cp "$BIN" "$C:/usr/local/bin/w3bs"
# Alpine's rustc links libgcc_s dynamically; the W3BS OS images have it (nodejs pulls it in).
run apk add -q --no-cache libgcc
run apk update -q  # an index cache, as on any installed machine (the image ships none)
run sh -c 'printf "ID=w3bs-os\nNAME=\"W3BS OS\"\nVERSION=0.1.0\nEDITION=docker-server\n" > /etc/w3bs-os-release'
run sh -c 'cp /etc/apk/repositories /tmp/repositories.orig'

# status
s=$(run w3bs os status --json)
echo "$s" | jq -e '.result == "ok" and .data.w3bs_os == true and .data.edition == "docker-server"' >/dev/null || die "status: $s"
echo "$s" | jq -e --arg b "v$FROM" '.data.alpine_branch == $b' >/dev/null || die "status branch: $s"
echo "$s" | jq -e '.data.newest_branch | test("^v[0-9]+\\.[0-9]+$")' >/dev/null || die "status newest_branch: $s"
echo "$s" | jq -e '(.data.upgrade_available | type) == "boolean" and (.data.pending_updates | type) == "number"' >/dev/null || die "status fields: $s"
ok "status: $(echo "$s" | jq -c '{w3bs_os:.data.w3bs_os, edition:.data.edition, alpine:.data.alpine.release, branch:.data.alpine_branch, newest:.data.newest_branch, upgrade_available:.data.upgrade_available, pending:.data.pending_updates, reboot_needed:.data.reboot_needed}')"

# refusals: edge needs --yes, skipping a branch needs --allow-skip, an unreleased branch
set +e
run w3bs os upgrade --to edge --json >/dev/null; [ $? -eq 3 ] || die "--to edge without --yes should exit 3"
run w3bs os upgrade --to v3.99 --yes --json >/dev/null; [ $? -ne 0 ] || die "--to v3.99 should fail"
run w3bs os upgrade --to "v$TO" --json >/dev/null; [ $? -eq 3 ] || die "upgrade without --yes should exit 3 (approval)"
set -e
ok "refuses edge without --yes, unreleased branches, and unapproved upgrades"
run cmp -s /etc/apk/repositories /tmp/repositories.orig || die "a refused upgrade changed the repositories"

# dry run changes nothing
d=$(run w3bs os upgrade --to "v$TO" --dry-run --json)
echo "$d" | jq -e '.result == "ok" and .data.dry_run == true and (.data.plan | length) > 3' >/dev/null || die "dry run: $d"
run cmp -s /etc/apk/repositories /tmp/repositories.orig || die "dry run changed the repositories"
ok "dry run plans $(echo "$d" | jq '.data.plan | length') steps and changes nothing"

# the upgrade, interrupted after the repositories are rewritten (test hook), then resumed
set +e
f=$(docker exec -e W3BS_OS_TEST_FAIL_AT=index "$C" w3bs os upgrade --to "v$TO" --yes --json); rc=$?
set -e
[ $rc -eq 1 ] || die "a failed step should exit 1 (got $rc): $f"
echo "$f" | jq -e '.result == "failure" and .error.code == "TEST_FAILURE" and ([.data.steps[] | select(.status == "failed")][0].step == "index")' >/dev/null || die "failed upgrade: $f"
run grep -q '"state": "failed"' /var/lib/w3bs-os/upgrade.json || die "journal not marked failed"
set +e
run w3bs os upgrade --yes --json >/dev/null; rc=$?
set -e
[ $rc -eq 4 ] || die "a new upgrade over an unfinished one should exit 4 (got $rc)"
ok "a failed step exits 1, is journaled, and blocks a fresh upgrade until --resume/--rollback"
u=$(run w3bs os upgrade --resume --yes --json) || die "resume failed: $u"
echo "$u" | jq -e '.result == "ok" and ([.data.steps[] | select(.status == "failed")] | length) == 0' >/dev/null || die "upgrade: $u"
rel=$(run cat /etc/alpine-release)
case "$rel" in "$TO".*|"$TO") ;; *) die "alpine-release is $rel after upgrading to $TO" ;; esac
left=$(run apk upgrade --available --simulate | grep -c '^(' || true)
[ "$left" -eq 0 ] || die "$left packages still to upgrade after the dist-upgrade"
run grep -q "^ALPINE_BRANCH=v$TO$" /etc/w3bs-os-release || die "w3bs-os-release not updated"
run grep -q "^ALPINE=$rel$" /etc/w3bs-os-release || die "w3bs-os-release ALPINE not updated"
run jq -e '.state == "done"' /var/lib/w3bs-os/upgrade.json >/dev/null 2>&1 || run grep -q '"state": "done"' /var/lib/w3bs-os/upgrade.json || die "journal not done"
run test -f "/etc/apk/repositories.bak-v$FROM" || die "no repositories backup"
ok "upgrade v$FROM -> v$TO: alpine-release $rel, nothing left to upgrade, release file and journal written ($(echo "$u" | jq -r '[.data.steps[] | "\(.step)=\(.status)"] | join(" ")'))"

s=$(run w3bs os status --json)
echo "$s" | jq -e --arg b "v$TO" '.data.alpine_branch == $b' >/dev/null || die "status after upgrade: $s"
u2=$(run w3bs os upgrade --json)
echo "$u2" | jq -e '.result == "ok" and .data.up_to_date == true' >/dev/null || die "second upgrade should be up to date: $u2"
ok "status reports v$TO; a second upgrade is a no-op ($(echo "$u2" | jq -r .data.message))"

# update on the new branch
p=$(run w3bs os update --yes --json) || die "update failed: $p"
echo "$p" | jq -e '.result == "ok"' >/dev/null || die "update: $p"
ok "update: $(echo "$p" | jq -r '[.data.steps[] | "\(.step)=\(.status)"] | join(" ")')"

# rollback restores the repositories
r=$(run w3bs os upgrade --rollback --yes --json) || die "rollback failed: $r"
echo "$r" | jq -e '.result == "ok"' >/dev/null || die "rollback: $r"
run cmp -s /etc/apk/repositories /tmp/repositories.orig || die "rollback did not restore the repositories"
run grep -q '"state": "rolled_back"' /var/lib/w3bs-os/upgrade.json || die "journal not marked rolled_back"
set +e
run w3bs os upgrade --rollback --yes --json >/dev/null; [ $? -ne 0 ] || die "a second rollback should fail"
set -e
ok "rollback restored /etc/apk/repositories to v$FROM"

echo "PASS: w3bs os e2e (alpine $FROM -> $TO)"
