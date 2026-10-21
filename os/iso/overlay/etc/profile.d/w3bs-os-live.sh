# W3BS OS live installer: on tty1 the first question is graphical or text installer.
# w3bs.installer=gui|text on the kernel command line answers it in advance.
if [ "$(id -u)" = 0 ] && [ "$(tty 2>/dev/null)" = /dev/tty1 ] && [ -z "${W3BS_OS_OFFERED:-}" ] \
	&& [ -e /etc/w3bs-os/live ] && [ -x /usr/local/sbin/w3bs-os-install ]; then
	export W3BS_OS_OFFERED=1
	_w3bs_pick=
	for _w3bs_a in $(cat /proc/cmdline); do
		case "$_w3bs_a" in w3bs.installer=gui) _w3bs_pick=1 ;; w3bs.installer=text) _w3bs_pick=2 ;; esac
	done
	_w3bs_def=1
	# A display: a DRM card, or a PCI display controller whose driver loads later.
	if ! ls /sys/class/drm/card[0-9]*/device >/dev/null 2>&1 \
		&& ! grep -qs '^0x03' /sys/bus/pci/devices/*/class; then _w3bs_def=2; fi
	clear 2>/dev/null
	printf '\n  W3BS OS %s installer\n' "$(cat /usr/share/w3bs-os/VERSION 2>/dev/null)"
	printf '  It erases a disk you choose, after a review. A wired network is required.\n\n'
	if [ -z "$_w3bs_pick" ]; then
		printf '  How do you want to install?\n'
		printf '    1  Graphical installer (mouse and windows)%s\n' "$( [ $_w3bs_def = 1 ] && echo '  [default]' || echo '  (no display found)')"
		printf '    2  Text installer (command line)%s\n' "$( [ $_w3bs_def = 2 ] && echo '  [default]')"
		printf '    3  Shell: do not install now (forgot a password? w3bs-os-install --reset-password USER)\n\n'
		printf '  Choose 1, 2 or 3 [%s]: ' "$_w3bs_def"
		read -r _w3bs_pick
		[ -n "$_w3bs_pick" ] || _w3bs_pick=$_w3bs_def
	fi
	case "$_w3bs_pick" in
		1*|g*|G*)
			/usr/local/sbin/w3bs-os-install-gui
			case $? in
				0) ;;
				10) /usr/local/sbin/w3bs-os-install ;;
				*) printf '\n  Starting the text installer instead.\n'; /usr/local/sbin/w3bs-os-install ;;
			esac ;;
		2*|t*|T*) /usr/local/sbin/w3bs-os-install ;;
		*) printf '\n  Run w3bs-os-install-gui (graphical) or w3bs-os-install (text) when you are ready.\n\n' ;;
	esac
	unset _w3bs_pick _w3bs_def _w3bs_a
fi
