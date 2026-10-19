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
  MANAGER_VERSION='0.1.15'
  MANAGER_SHA256S='x86_64-unknown-linux-gnu=6b2cf1b17191ec6ac2f5784b8a0229263d8a5b7bea903b40c1f4bb101f50703a aarch64-unknown-linux-gnu=184238913fa4d6d01d226b2289f410c622e40d8dfeda92612a328916c9900167 x86_64-unknown-linux-musl=1cb701dee155ab272bf8aac3999708d03e7a5e2597501f592c5398b1f10011e2 aarch64-unknown-linux-musl=1412ea869289004b8a4aa9c4424be773a895b18117a9c019afb6189d18482149 x86_64-apple-darwin=3065f595539e0d74134ce50138502ae217a93019bbcf9de703bd936542c7ddab aarch64-apple-darwin=180aaed69454d285ffa3a6e2661c6768f5038aae4d8803e9254c9eab80e4536f'

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
  # On a desktop, offer (never register silently) the handler for w3bs:// install links.
  if [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ] || { [ "$(uname -s)" = Darwin ] && [ -z "${SSH_CONNECTION:-}" ]; }; then
    say "Open w3bs:// install links from your browser (asks before every install):  w3bs handler install"
  fi
}

w3bs_main "$@"
