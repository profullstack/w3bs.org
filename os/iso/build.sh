#!/bin/sh
# Build the W3BS OS network installer ISO with Docker (any x86_64 Linux host).
#   os/iso/build.sh [VERSION]      -> os/iso/out/w3bs-os-VERSION-x86_64.iso + SHA256SUMS
#   PRESET=DIR os/iso/build.sh   bakes DIR/users (one name per line) and DIR/keys/NAME (SSH
#                                public keys; NAME may be root) in as the installer defaults
# Needs about 6 GB of free disk and a network connection (Alpine mirrors, crates.io).
set -eu
VERSION=${1:-$(cat "$(dirname "$0")/VERSION")}
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
out="$here/out"
mkdir -p "$out/bin"

echo "==> Building w3bs and w3bs-desktop for musl (alpine:3.24)"
DOCKER_BUILDKIT=1 docker build -f "$here/Dockerfile.bins" --output "type=local,dest=$out/bin" "$root"

echo "==> Building the ISO with aports mkimage.sh (alpine:3.24)"
docker run --rm \
	-v "$root:/src:ro" \
	-v "$out/bin:/bin-in:ro" \
	-v "$out:/out" \
	${PRESET:+-v "$PRESET:/preset-in:ro"} \
	alpine:3.24 sh /src/os/iso/build-in-container.sh "$VERSION"

ls -la "$out"/w3bs-os-"$VERSION"-x86_64.iso
