#!/bin/sh
# Runs inside an alpine:3.24 container as root (see build.sh and .github/workflows/os-iso.yml).
#   /src     the repository (read-only is fine)
#   /bin-in  the musl w3bs and w3bs-desktop binaries (from Dockerfile.bins)
#   /out     where w3bs-os-<version>-x86_64.iso and SHA256SUMS are written
# mkimage.sh refuses to run as root, so the image is built by an unprivileged "build" user.
# Its abuild signing key is generated for every build and never leaves the container.
set -eu
VERSION=${1:?usage: build-in-container.sh VERSION}
ALPINE_BRANCH=${ALPINE_BRANCH:-3.24}
MIRROR=${MIRROR:-https://dl-cdn.alpinelinux.org/alpine}
SRC=${SRC:-/src}
BIN_IN=${BIN_IN:-/bin-in}
OUT=${OUT:-/out}
work=/work

echo "==> Installing the image tools"
apk add --no-cache alpine-sdk abuild alpine-conf xorriso squashfs-tools grub grub-efi \
	mtools syslinux dosfstools fakeroot git apk-tools >/dev/null
id build >/dev/null 2>&1 || adduser -D -G abuild build
rm -rf "$work" && mkdir -p "$work" "$OUT"
chown build:abuild "$work"

echo "==> Generating a throwaway signing key"
su build -s /bin/sh -c 'PACKAGER="W3BS OS build <os-build@w3bs.org>" abuild-keygen -a -n' >/dev/null 2>&1
pub=$(ls /home/build/.abuild/*.rsa.pub | head -n 1)
cp "$pub" /etc/apk/keys/

echo "==> Staging aports $ALPINE_BRANCH-stable, the profile and the overlay"
su build -s /bin/sh -c "git clone -q --depth 1 -b '${ALPINE_BRANCH}-stable' https://gitlab.alpinelinux.org/alpine/aports.git '$work/aports'"
mkdir -p /home/build/.mkimage
cp "$SRC/os/iso/mkimg.w3bs.sh" "$SRC/os/iso/genapkovl-w3bs.sh" /home/build/.mkimage/
chmod 0755 /home/build/.mkimage/genapkovl-w3bs.sh
# mkimage checksums the apkovl script by a path relative to its working directory.
cp /home/build/.mkimage/genapkovl-w3bs.sh "$work/"
overlay="$work/overlay"
cp -a "$SRC/os/iso/overlay" "$overlay"
install -D -m 0755 "$BIN_IN/w3bs" "$overlay/usr/local/bin/w3bs"
install -D -m 0755 "$BIN_IN/w3bs-desktop" "$overlay/usr/local/bin/w3bs-desktop"
install -D -m 0644 "$SRC/registry/FINGERPRINT" "$overlay/usr/share/w3bs-os/FINGERPRINT"
echo "$VERSION" > "$overlay/usr/share/w3bs-os/VERSION"
cp "$SRC/os/iso/VERSION" "$overlay/usr/share/w3bs-os/BASE_VERSION"
chmod 0755 "$overlay/usr/local/sbin/"* "$overlay/usr/share/w3bs-os/first-login.sh"
"$overlay/usr/local/bin/w3bs" --version
chown -R build:abuild /home/build/.mkimage "$overlay"

echo "==> Building the ISO (mkimage.sh --profile w3bs)"
su build -s /bin/sh -c "cd '$work' && W3BS_OVERLAY='$overlay' sh '$work/aports/scripts/mkimage.sh' \
	--tag '$VERSION' --outdir '$work/iso' --workdir '$work/cache' --arch x86_64 --profile w3bs --hostkeys \
	--repository '$MIRROR/v$ALPINE_BRANCH/main' --repository '$MIRROR/v$ALPINE_BRANCH/community'"

iso="w3bs-os-$VERSION-x86_64.iso"
[ -f "$work/iso/$iso" ] || { echo "mkimage did not produce $iso" >&2; ls -la "$work/iso" >&2; exit 1; }
mv "$work/iso/$iso" "$OUT/$iso"
(cd "$OUT" && sha256sum "$iso" > SHA256SUMS)
rm -rf "$work"
ls -la "$OUT"
cat "$OUT/SHA256SUMS"
