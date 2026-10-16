#!/bin/sh
# W3BS Training installer for Linux and macOS. https://w3bs.org/training
#
#   curl --proto '=https' --proto-redir '=https' -fsSL https://w3bs.org/training/install.sh | sh
#
# Inspect first if you like:  curl -fsSL https://w3bs.org/training/install.sh -o install.sh && less install.sh && sh install.sh
# On Windows use PowerShell instead:  irm https://w3bs.org/training/install.ps1 | iex
#
# What this does:
#   1. Downloads the w3bs-training release archive for this machine from GitHub releases and
#      checks it against the release's SHA256SUMS.
#   2. Installs the single self-contained binary to ~/.local/bin (override with W3BS_BIN_DIR).
#      No Bun or Node is needed. `w3bs training …` runs it through the w3bs CLI.
#   3. Checks that a key store is available for encrypting recordings at rest (libsecret's
#      secret-tool on Linux, the login keychain on macOS). It never stores anything in plaintext.
#   It starts nothing and changes no settings. The Agentic Desktop Layer's recorder hands its
#   recordings to this binary when you finish a task.
#
# Environment: W3BS_TRAINING_VERSION (default below), W3BS_BIN_DIR,
#              W3BS_TRAINING_ARCHIVE=PATH with W3BS_TRAINING_SHA256=HEX (install that local
#              archive instead of downloading, still checksum-verified; for testing).
# Uninstall:   rm ~/.local/bin/w3bs-training   (your data stays in ~/.local/share/w3bs/training)
#
# Nothing runs until the last line, so a truncated download does nothing.

wt_main() {
  set -u
  VERSION="${W3BS_TRAINING_VERSION:-0.2.2}"
  BASE="https://github.com/profullstack/w3bs.org/releases/download/training-v$VERSION"

  say() { printf '%s\n' "$*" >&2; }
  die() { say "w3bs-training installer: $1"; exit "${2:-1}"; }

  [ $# -eq 0 ] || die "this installer takes no options" 2
  case "$(uname -s)" in
    Linux) os=unknown-linux-gnu ;;
    Darwin) os=apple-darwin ;;
    *) die "unsupported system $(uname -s); on Windows run in PowerShell:  irm https://w3bs.org/training/install.ps1 | iex" ;;
  esac
  case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) die "unsupported CPU $(uname -m); x86_64 and aarch64 are supported" ;;
  esac
  if [ "$os" = apple-darwin ] && [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = 1 ]; then
    arch=aarch64
  fi
  triple="$arch-$os"
  [ -n "${HOME:-}" ] || die "HOME is not set"
  bin="${W3BS_BIN_DIR:-$HOME/.local/bin}"

  if command -v curl >/dev/null 2>&1; then
    fetch() { curl --proto '=https' --proto-redir '=https' -fsSL "$1" -o "$2"; }
  elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -q --https-only -O "$2" "$1"; }
  else
    die "need curl or wget"
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    sha() { sha256sum "$1" | cut -d' ' -f1; }
  elif command -v shasum >/dev/null 2>&1; then
    sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
  else
    die "need sha256sum or shasum"
  fi

  tmp="$(mktemp -d)" || die "cannot create a temporary directory"
  trap 'rm -rf "$tmp"' EXIT INT TERM
  archive="w3bs-training-$VERSION-$triple.tar.gz"
  if [ -n "${W3BS_TRAINING_ARCHIVE:-}" ]; then
    want="${W3BS_TRAINING_SHA256:-}"
    [ -n "$want" ] || die "W3BS_TRAINING_ARCHIVE needs W3BS_TRAINING_SHA256 (an archive is never installed unchecked)"
    say "Installing W3BS Training from $W3BS_TRAINING_ARCHIVE"
    cp "$W3BS_TRAINING_ARCHIVE" "$tmp/$archive" || die "cannot read $W3BS_TRAINING_ARCHIVE"
  else
    say "Downloading W3BS Training $VERSION for $triple"
    fetch "$BASE/$archive" "$tmp/$archive" || die "download failed: $BASE/$archive"
    fetch "$BASE/SHA256SUMS" "$tmp/SHA256SUMS" || die "download failed: $BASE/SHA256SUMS"
    want="$(grep " $archive\$" "$tmp/SHA256SUMS" | cut -d' ' -f1)"
    [ -n "$want" ] || die "$archive is not listed in SHA256SUMS"
  fi
  got="$(sha "$tmp/$archive")"
  [ "$want" = "$got" ] || die "checksum mismatch for $archive (expected $want, got $got)"
  mkdir -p "$tmp/x" && tar -xzf "$tmp/$archive" -C "$tmp/x" || die "cannot unpack $archive"

  mkdir -p "$bin" || die "cannot create $bin"
  rm -f "$bin/w3bs-training"
  cp "$tmp/x/bin/w3bs-training" "$bin/w3bs-training" && chmod 0755 "$bin/w3bs-training" || die "cannot install to $bin"
  # Release binaries are not code-signed; Apple silicon refuses to run an unsigned binary, so
  # sign it ad hoc for this machine (what Xcode does for local builds).
  if [ "$os" = apple-darwin ] && command -v codesign >/dev/null 2>&1; then
    codesign --sign - --force "$bin/w3bs-training" >/dev/null 2>&1 || say "warning: ad-hoc signing failed; macOS may refuse to run $bin/w3bs-training"
  fi
  "$bin/w3bs-training" version >/dev/null 2>&1 || die "$bin/w3bs-training does not run on this machine"
  if command -v w3bs >/dev/null 2>&1; then cmd="w3bs training"; else cmd="w3bs-training"; fi
  say "Installed $bin/w3bs-training $VERSION"

  case ":${PATH:-}:" in
    *":$bin:"*) ;;
    *) say "Note: $bin is not on your PATH. Add it, e.g.  echo 'export PATH=\"$bin:\$PATH\"' >> ~/.profile" ;;
  esac
  if [ "$os" = unknown-linux-gnu ] && ! command -v secret-tool >/dev/null 2>&1; then
    say ""
    say "No key store found: W3BS Training encrypts recordings with a key kept in your desktop's"
    say "keyring through libsecret's secret-tool, and refuses to store them unencrypted."
    say "  Debian/Ubuntu: sudo apt install libsecret-tools     Fedora: sudo dnf install libsecret"
    say "  Arch: sudo pacman -S libsecret                      (or set W3BS_TRAINING_PASSPHRASE)"
    say "Until then, finished recordings wait in the desktop layer's memory."
  fi
  say ""
  say "Next:"
  say "  $cmd open           the review app in your browser (one-time link)"
  say "  $cmd --help         everything else"
  say "Record a task from the Agentic Desktop Layer tray: Start training…"
}

wt_main "$@"
