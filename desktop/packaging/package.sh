#!/bin/sh
# Package a release build of w3bs-desktop the way the installers expect it. Used by the release
# workflow and by the installer CI jobs, so what CI installs is what a release ships.
#
#   packaging/package.sh TARGET VERSION OUTDIR      (from desktop/, after
#                                                    cargo build --release --target TARGET)
#
# Linux/macOS: OUTDIR/w3bs-desktop-VERSION-TARGET.tar.gz with bin/w3bs-desktop, LICENSE,
#              README.md and share/packaging/.
# Windows:     OUTDIR/w3bs-desktop-VERSION-TARGET.zip with bin\w3bs-desktop.exe (console CLI),
#              bin\w3bs-desktopw.exe (GUI subsystem, for the broker and tray at logon), LICENSE
#              and README.md.
set -eu
[ $# -eq 3 ] || { echo "usage: $0 TARGET VERSION OUTDIR" >&2; exit 2; }
target=$1 version=$2 out=$3
rel="${CARGO_TARGET_DIR:-target}/$target/release"
stage="$out/stage-$target"
rm -rf "$stage"
mkdir -p "$stage/bin" "$out"
name="w3bs-desktop-$version-$target"
case "$target" in
  *-windows-*)
    cp "$rel/w3bs-desktop.exe" "$rel/w3bs-desktopw.exe" "$stage/bin/"
    cp LICENSE README.md "$stage/"
    rm -f "$out/$name.zip"
    if command -v 7z >/dev/null 2>&1; then
      (cd "$stage" && 7z a -tzip -bd "../$name.zip" bin LICENSE README.md >/dev/null)
    elif command -v zip >/dev/null 2>&1; then
      (cd "$stage" && zip -qr "../$name.zip" bin LICENSE README.md)
    else
      (cd "$stage" && pwsh -NoProfile -Command "Compress-Archive -Path bin, LICENSE, README.md -DestinationPath ../$name.zip")
    fi
    ;;
  *)
    cp "$rel/w3bs-desktop" "$stage/bin/"
    cp LICENSE README.md "$stage/"
    mkdir -p "$stage/share" && cp -r packaging "$stage/share/"
    tar -czf "$out/$name.tar.gz" -C "$stage" .
    ;;
esac
rm -rf "$stage"
ls -l "$out"/w3bs-desktop-*
