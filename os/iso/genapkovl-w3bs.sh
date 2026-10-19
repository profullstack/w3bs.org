#!/bin/sh -e
# Generates the W3BS OS live overlay (HOSTNAME.apkovl.tar.gz), unpacked over the live root
# at boot. Based on aports' genapkovl-dhcp.sh, plus the staged tree in $W3BS_OVERLAY:
# the installer, the w3bs/w3bs-desktop musl binaries, branding and the repo fingerprint.

HOSTNAME="$1"
[ -n "$HOSTNAME" ] || { echo "usage: $0 hostname"; exit 1; }
[ -d "${W3BS_OVERLAY:-}" ] || { echo "$0: W3BS_OVERLAY is not a directory"; exit 1; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

rc_add() {
	mkdir -p "$tmp"/etc/runlevels/"$2"
	ln -sf /etc/init.d/"$1" "$tmp"/etc/runlevels/"$2"/"$1"
}

cp -a "$W3BS_OVERLAY"/. "$tmp"/

mkdir -p "$tmp"/etc/network "$tmp"/etc/apk
echo "$HOSTNAME" > "$tmp"/etc/hostname
cat > "$tmp"/etc/network/interfaces <<EOF
auto lo
iface lo inet loopback

auto eth0
iface eth0 inet dhcp
EOF

# Installed into the live system from the ISO's own repository at boot; setup-disk also
# carries this list into the installed system.
cat > "$tmp"/etc/apk/world <<EOF
alpine-base
zsh
zsh-vcs
tzdata
kbd-bkeymaps
lsblk
blkid
findmnt
cryptsetup
lvm2
e2fsprogs
dosfstools
sfdisk
EOF

rc_add devfs sysinit
rc_add dmesg sysinit
rc_add mdev sysinit
rc_add hwdrivers sysinit
rc_add modloop sysinit

rc_add hwclock boot
rc_add modules boot
rc_add sysctl boot
rc_add hostname boot
rc_add bootmisc boot
rc_add syslog boot
rc_add networking boot

rc_add mount-ro shutdown
rc_add killprocs shutdown
rc_add savecache shutdown

chown -R 0:0 "$tmp"
chmod 0700 "$tmp"/usr/local/sbin/w3bs-os-install 2>/dev/null || true
tar -c -C "$tmp" etc usr | gzip -9n > "$HOSTNAME".apkovl.tar.gz
