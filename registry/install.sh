#!/bin/sh
# w3bs installer: the W3BS command line. https://w3bs.org/packages
#
#   curl --proto '=https' --proto-redir '=https' -fsSL <this url> | sh
#
# Inspect first if you like:  curl -fsSL <this url> -o install.sh && less install.sh && sh install.sh
#
# What this does:
#   1. If the w3bs CLI is not installed, download the prebuilt binary for
#      this machine, check its SHA-256, and let it install itself from its signed repository.
#   2. If this installer is for an application, register the publisher's signed repository
#      (first-use trust: you are shown its fingerprint) and run `w3bs install <package>`.
#   3. Afterwards, `w3bs update` keeps every installed application current.
#
# Trust boundary: this script runs as you before W3BS can verify anything, and the
# checksums and fingerprints below arrive in the same response as the script. They catch
# corruption and mix-ups, not a malicious server. Everything after the bootstrap is
# verified by W3BS against the signed repository metadata (TUF).
#
# Options are passed straight to `w3bs install`, e.g.  ... | sh -s -- --version 1.2.3 --pin
# Environment: W3BS_YES=1 approves without prompting (requires the fingerprints below);
#              W3BS_HOME, W3BS_BIN_DIR relocate the install.
#
# Nothing runs until the last line, so a truncated download does nothing.

w3bs_main() {
  set -u
  PACKAGE=''
  REPO_ALIAS='w3bs'
  REPO_URL='https://w3bs.org/repo/'
  REPO_FINGERPRINT='sha256:48d521f7610117c95cc782982e5c05b641b18bdfa8e48b36a634186133bc766f'
  MANAGER_REPO_URL='https://w3bs.org/repo/'
  MANAGER_FINGERPRINT='sha256:48d521f7610117c95cc782982e5c05b641b18bdfa8e48b36a634186133bc766f'
  MANAGER_DOWNLOAD_BASE='https://github.com/profullstack/w3bs.org/releases/download'
  MANAGER_VERSION='0.1.9'
  MANAGER_SHA256S='x86_64-unknown-linux-gnu=bd4473d5a7b1cbb7180748199d2099c4f559581fb0c0137fc6dc44c02f040998 aarch64-unknown-linux-gnu=e61477ca40581f2e10c30c4fd4eb9a8fc6536f70994799427e02460ecd0a3d81 x86_64-apple-darwin=78d58c8901d2d30f277309c78b5944c2ae39a9465f6173827de499961077b17b aarch64-apple-darwin=4a629e34d07d1112d00873e891aff438ad5a048b8cd8289c4a08adc8560ade08'

  say() { printf '%s\n' "$*" >&2; }
  die() { say "w3bs installer: $*"; exit "${2:-4}"; }

  case "$(uname -s)" in
    Linux) os=unknown-linux-gnu ;;
    Darwin) os=apple-darwin ;;
    *) die "unsupported operating system $(uname -s); W3BS supports Linux and macOS" ;;
  esac
  case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) die "unsupported CPU $(uname -m); W3BS supports x86_64 and aarch64" ;;
  esac
  triple="$arch-$os"

  if [ -n "${W3BS_HOME:-}" ]; then
    data="$W3BS_HOME/data"
    bin="${W3BS_BIN_DIR:-$W3BS_HOME/bin}"
  else
    [ -n "${HOME:-}" ] || die "HOME is not set"
    data="${XDG_DATA_HOME:-$HOME/.local/share}/w3bs"
    bin="${W3BS_BIN_DIR:-$HOME/.local/bin}"
  fi
  # The managed manager lives at a fixed path. A random `w3bs` on PATH is never trusted.
  manager="$data/packages/w3bs-manager/current/bin/w3bs"

  yes=""
  if [ "${W3BS_YES:-}" = 1 ]; then
    yes="--yes"
  elif ! (: </dev/tty) 2>/dev/null; then
    die "no terminal to confirm with; re-run with W3BS_YES=1 to approve the fingerprints shown in this script" 3
  fi

  fetch() { # url dest
    if command -v curl >/dev/null 2>&1; then
      curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
      wget --https-only -q -O "$2" "$1"
    else
      die "need curl or wget"
    fi
  }
  sha256() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
    elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d' ' -f1
    else die "need sha256sum or shasum"; fi
  }

  if [ ! -x "$manager" ]; then
    expected=""
    for pair in $MANAGER_SHA256S; do
      [ "${pair%%=*}" = "$triple" ] && expected="${pair#*=}"
    done
    [ -n "$expected" ] || die "no w3bs build for $triple yet"
    say "The w3bs CLI $MANAGER_VERSION will be installed. It manages this and future packages, and much more."
    say "  It keeps everything it installs updatable with one command: w3bs update"
    tmp="$(mktemp -d 2>/dev/null || mktemp -d -t w3bs)" || die "cannot create a temporary directory"
    trap 'rm -rf "$tmp"' EXIT INT TERM
    archive="$tmp/w3bs.tar.gz"
    fetch "$MANAGER_DOWNLOAD_BASE/w3bs-v$MANAGER_VERSION/w3bs-$MANAGER_VERSION-$triple.tar.gz" "$archive" || die "download failed"
    actual="$(sha256 "$archive")"
    [ "$actual" = "$expected" ] || die "checksum mismatch for the downloaded manager (got $actual, expected $expected)"
    mkdir "$tmp/x" && tar -xzf "$archive" -C "$tmp/x" || die "cannot unpack the manager"
    [ -x "$tmp/x/bin/w3bs" ] || die "the manager archive has no bin/w3bs"
    # The temporary binary installs the real manager through the verified engine.
    "$tmp/x/bin/w3bs" self install --repository-url "$MANAGER_REPO_URL" --root-fingerprint "$MANAGER_FINGERPRINT" $yes || exit $?
  fi

  if [ -n "$PACKAGE" ]; then
    if ! "$manager" repo info "$REPO_ALIAS" >/dev/null 2>&1; then
      "$manager" repo add "$REPO_ALIAS" "$REPO_URL" --root-fingerprint "$REPO_FINGERPRINT" $yes || exit $?
    fi
    "$manager" install "$PACKAGE" --repository "$REPO_ALIAS" --origin curl $yes "$@" || exit $?
  fi

  say ""
  say "w3bs: $manager   (try: w3bs packages, w3bs --help)"
  case ":${PATH:-}:" in
    *":$bin:"*) say "Update everything later with:  w3bs update" ;;
    *)
      say "$bin is not on your PATH. Add it with:"
      case "${SHELL:-}" in
        */fish) say "  fish_add_path $bin" ;;
        */zsh) say "  echo 'export PATH=\"$bin:\$PATH\"' >> ~/.zshrc" ;;
        *) say "  echo 'export PATH=\"$bin:\$PATH\"' >> ~/.profile" ;;
      esac
      say "Until then, run it as:  $bin/w3bs update"
      ;;
  esac
}

w3bs_main "$@"
