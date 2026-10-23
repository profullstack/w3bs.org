#!/bin/sh
# Build the W3BS OS (Debian 13 "trixie") live installer ISO with live-build, in a privileged
# debian:trixie container:  os/debian/build.sh 1.2.0   ->  os/debian/out/w3bs-os-1.2.0-amd64.iso
set -eu
VERSION=${1:?usage: build.sh VERSION}
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$here/out"
docker run --rm --privileged -v "$here:/src:ro" -v "$here/out:/out" debian:trixie sh -euc '
	apt-get update -qq >/dev/null
	apt-get install -y -qq live-build debootstrap >/dev/null
	mkdir /build && cd /build && cp -a /src/config .
	echo "'"$VERSION"'" > config/includes.chroot/etc/w3bs-os-version
	lb config --distribution trixie --architectures amd64 --binary-images iso-hybrid \
		--archive-areas "main non-free-firmware" --debian-installer none --apt-recommends false \
		--bootloaders "syslinux,grub-efi" --iso-application "W3BS OS" --iso-volume "W3BS_OS '"$VERSION"'" \
		--iso-publisher "w3bs.org" --memtest none --firmware-binary false --firmware-chroot false \
		--bootappend-live "boot=live components quiet hostname=w3bs-os console=tty0 console=ttyS0,115200" >/dev/null
	# live-build stock menus wait forever on "Press ENTER": boot the live system after 5 s,
	# under a W3BS OS title, in the BIOS (isolinux) and UEFI (grub) menus alike.
	cp -a /usr/share/live/build/bootloaders config/
	for f in config/bootloaders/isolinux/isolinux.cfg; do
		sed -i "/^timeout /d" "$f"; echo "timeout 50" >>"$f"
	done
	grep -rl "Debian GNU/Linux" config/bootloaders | xargs -r sed -i "s|Debian GNU/Linux|W3BS OS '"$VERSION"' · Debian|g"
	for f in config/bootloaders/grub-pc/config.cfg config/bootloaders/grub-pc/grub.cfg; do
		[ -f "$f" ] && { sed -i "/^set timeout/d" "$f"; echo "set timeout=5" >>"$f"; }
	done
	lb build >/build/build.log 2>&1 || { tail -40 /build/build.log; exit 1; }
	cp live-image-amd64.hybrid.iso /out/w3bs-os-'"$VERSION"'-amd64.iso
	cd /out && sha256sum w3bs-os-'"$VERSION"'-amd64.iso > SHA256SUMS
'
ls -la "$here/out"
