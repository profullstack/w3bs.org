# W3BS OS network installer profile for Alpine's aports scripts/mkimage.sh.
# Loaded from ~/.mkimage/ (after aports' own mkimg.*.sh), so the functions below override
# the stock boot menu generators for this build only.
#
#   sh aports/scripts/mkimage.sh --tag 0.1.0 --profile w3bs --arch x86_64 \
#       --repository https://dl-cdn.alpinelinux.org/alpine/v3.24/main \
#       --repository https://dl-cdn.alpinelinux.org/alpine/v3.24/community --outdir out
#
# W3BS_OVERLAY must point at the staged overlay tree (see os/iso/build-in-container.sh).
#
# Based on Alpine's "standard" profile (lean: everything else downloads during the install)
# plus the tools the installer needs offline in the live system and CPU microcode.

profile_w3bs() {
	profile_standard
	title="W3BS OS"
	desc="W3BS OS network installer (Alpine Linux based).
		Boots to a live system that runs from RAM and offers w3bs-os-install.
		Packages are downloaded during the install."
	profile_abbrev="w3bs"
	image_name="w3bs-os"
	arch="x86_64"
	# Default entry: the screen (VGA/framebuffer, e.g. a VPS VNC console) is the console.
	# The serial entry (below) adds ttyS0 as the console; a serial login runs either way.
	kernel_cmdline="console=tty0"
	syslinux_serial="0 115200"
	syslinux_timeout=100
	grub_mod="$grub_mod serial terminal"
	boot_addons="amd-ucode intel-ucode"
	initrd_ucode="/boot/amd-ucode.img /boot/intel-ucode.img"
	apks="$apks libgcc zsh zsh-vcs tzdata kbd-bkeymaps alpine-conf lsblk blkid findmnt
		cryptsetup lvm2 e2fsprogs dosfstools sfdisk mtools efibootmgr
		grub grub-efi grub-bios syslinux mkinitfs doas dhcpcd dhcpcd-openrc
		cage seatd-launch py3-gobject3 gtk4.0 font-dejavu adwaita-icon-theme eudev mesa-dri-gallium"
	# ^ the graphical installer (w3bs-os-install-gui): loaded from the ISO only when chosen.
	apkovl="genapkovl-w3bs.sh"
	hostname="w3bs-os"
}

# aports' build_syslinux plus chain.c32, so the "disk" entry can chain-load the first hard
# disk (LOCALBOOT fails on many server BIOSes, SeaBIOS/Proxmox among them).
build_syslinux() {
	local _fn
	mkdir -p "$DESTDIR"/boot/syslinux
	apk fetch --root "$APKROOT" --stdout syslinux | tar -C "$DESTDIR" -xz usr/share/syslinux
	for _fn in isohdpfx.bin isolinux.bin ldlinux.c32 libutil.c32 libcom32.c32 mboot.c32 chain.c32; do
		mv "$DESTDIR"/usr/share/syslinux/$_fn "$DESTDIR"/boot/syslinux/$_fn
	done
	rm -rf "$DESTDIR"/usr
}

# Volume label (also used by the EFI grub to find the ISO).
gen_volid() {
	printf "%s" "W3BS_OS ${RELEASE%_rc*} $ARCH" | cut -c1-32
}

syslinux_gen_config() {
	[ -z "$syslinux_serial" ] || echo "SERIAL $syslinux_serial"
	echo "SAY W3BS OS ${RELEASE} network installer (Alpine Linux based)"
	echo "SAY Enter or wait 10 s: the installer.   disk + Enter: boot the installed system on the first disk."
	echo "SAY serial + Enter: the installer on serial console ttyS0."
	echo "TIMEOUT ${syslinux_timeout:-10}"
	echo "PROMPT ${syslinux_prompt:-1}"
	echo "DEFAULT w3bs"
	local _f _p _initrd
	for _f in $kernel_flavors; do
		_initrd="/boot/initramfs-$_f"
		for _p in $initrd_ucode; do
			_initrd="$_p,$_initrd"
		done
		cat <<- EOF

		LABEL w3bs
			MENU LABEL W3BS OS ${RELEASE} installer
			KERNEL /boot/vmlinuz-$_f
			INITRD $_initrd
			APPEND $initfs_cmdline $kernel_cmdline

		LABEL disk
			MENU LABEL Boot the installed system (first hard disk)
			COM32 chain.c32
			APPEND hd0

		LABEL serial
			MENU LABEL W3BS OS ${RELEASE} installer (serial console ttyS0)
			KERNEL /boot/vmlinuz-$_f
			INITRD $_initrd
			APPEND $initfs_cmdline console=tty0 console=ttyS0,115200
		EOF
	done
}

grub_gen_config() {
	local _f _p _initrd
	cat <<- EOF
	serial --unit=0 --speed=115200
	terminal_input console serial
	terminal_output console serial
	set timeout=5
	set default=0
	EOF
	for _f in $kernel_flavors; do
		_initrd="/boot/initramfs-$_f"
		for _p in $initrd_ucode; do
			_initrd="$_p $_initrd"
		done
		cat <<- EOF

		menuentry "W3BS OS ${RELEASE} installer" {
			linux	/boot/vmlinuz-$_f $initfs_cmdline $kernel_cmdline
			initrd	$_initrd
		}

		menuentry "W3BS OS ${RELEASE} installer (serial console ttyS0)" {
			linux	/boot/vmlinuz-$_f $initfs_cmdline console=tty0 console=ttyS0,115200
			initrd	$_initrd
		}

		menuentry "Boot the installed system (next boot device)" {
			exit
		}

		menuentry "W3BS OS ${RELEASE} installer (safe graphics: nomodeset)" {
			linux	/boot/vmlinuz-$_f $initfs_cmdline $kernel_cmdline nomodeset
			initrd	$_initrd
		}
		EOF
	done
}
