#!/bin/sh
# Build w3bs-training release archives: one self-contained binary per platform (bun build
# --compile; no Bun or Node needed to run it), packaged the way install.sh expects.
#
#   training/packaging/build.sh VERSION OUTDIR [TARGET...]     (from the repository root)
#
# TARGET is a Rust-style triple, as the desktop release uses; default: all five.
# Linux/macOS: OUTDIR/w3bs-training-VERSION-TARGET.tar.gz with bin/w3bs-training, LICENSE, README.md
# Windows:     OUTDIR/w3bs-training-VERSION-TARGET.zip with bin\w3bs-training.exe, LICENSE, README.md
set -eu
[ $# -ge 2 ] || { echo "usage: $0 VERSION OUTDIR [TARGET...]" >&2; exit 2; }
version=$1 out=$2
shift 2
[ $# -gt 0 ] || set -- x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-apple-darwin aarch64-apple-darwin x86_64-pc-windows-msvc
code=$(sed -n "s/^export const VERSION = '\(.*\)';/\1/p" training/core/service.mjs)
[ "$code" = "$version" ] || { echo "training/core/service.mjs says VERSION $code, not $version" >&2; exit 1; }
bun training/ui/build-assets.mjs --check
mkdir -p "$out"
for target in "$@"; do
  case "$target" in
    # The baseline build runs on x86_64 CPUs without AVX2 too.
    x86_64-unknown-linux-gnu) bt=bun-linux-x64-baseline exe= ;;
    aarch64-unknown-linux-gnu) bt=bun-linux-arm64 exe= ;;
    x86_64-apple-darwin) bt=bun-darwin-x64 exe= ;;
    aarch64-apple-darwin) bt=bun-darwin-arm64 exe= ;;
    x86_64-pc-windows-msvc) bt=bun-windows-x64-baseline exe=.exe ;;
    *) echo "unknown target $target" >&2; exit 2 ;;
  esac
  stage="$out/stage-$target"
  rm -rf "$stage"
  mkdir -p "$stage/bin"
  bun build training/bin/w3bs-training.mjs --compile --minify --target="$bt" --outfile "$stage/bin/w3bs-training$exe" >/dev/null
  cp LICENSE "$stage/LICENSE"
  cp docs/training/README.md "$stage/README.md"
  name="w3bs-training-$version-$target"
  if [ -n "$exe" ]; then
    rm -f "$out/$name.zip"
    if command -v zip >/dev/null 2>&1; then
      (cd "$stage" && zip -qr "../$name.zip" bin LICENSE README.md)
    else
      python3 -c 'import sys, zipfile, os
s, out = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    for f in ("bin/w3bs-training.exe", "LICENSE", "README.md"):
        z.write(os.path.join(s, f), f)' "$stage" "$out/$name.zip"
    fi
  else
    tar -czf "$out/$name.tar.gz" -C "$stage" .
  fi
  rm -rf "$stage"
  echo "$out/$name"
done
