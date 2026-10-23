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
	# SSH is on: show how to reach this machine (and the one-time root password).
	_w3bs_ssh_info() {
		_pw=$(cat /run/w3bs-live-ssh-password 2>/dev/null)
		_addrs=$(ip addr show scope global 2>/dev/null | awk '$1 == "inet" || $1 == "inet6" { sub(/\/.*/, "", $2); print $2 }')
		if [ -n "$_pw" ] && [ -n "$_addrs" ]; then
			printf '  SSH is on: install from another computer (copy and paste work there):\n'
			for _a in $_addrs; do printf '    ssh root@%s\n' "$_a"; done
			printf '  One-time root password: %s   then run: w3bs-os-install\n\n' "$_pw"
		elif [ -n "$_pw" ]; then
			printf '  SSH is on, but this machine has no network address yet: choose 4 to set a static IP.\n\n'
		fi
	}
	_w3bs_ssh_info
	if [ -z "$_w3bs_pick" ]; then
		printf '  How do you want to install?\n'
		printf '    1  Graphical installer (mouse and windows)%s\n' "$( [ $_w3bs_def = 1 ] && echo '  [default]' || echo '  (no display found)')"
		printf '    2  Text installer (command line)%s\n' "$( [ $_w3bs_def = 2 ] && echo '  [default]')"
		printf '    3  Shell: do not install now (forgot a password? w3bs-os-install --reset-password USER)\n'
		printf '    4  Set a static IP (servers without DHCP), then come back here\n\n'
		printf '  Choose 1, 2, 3 or 4 [%s]: ' "$_w3bs_def"
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
		4*)
			_if=$(for p in /sys/class/net/*; do i=${p##*/}; [ "$i" != lo ] && [ -e "$p/device" ] && echo "$i"; done | head -n 1)
			printf '\n  Interface [%s]: ' "$_if"; read -r _x; [ -n "$_x" ] && _if=$_x
			printf '  IPv4 address with prefix (e.g. 203.0.113.5/32): '; read -r _ip
			printf '  Gateway: '; read -r _gw
			printf '  Nameservers [1.1.1.1 9.9.9.9]: '; read -r _dns; [ -n "$_dns" ] || _dns="1.1.1.1 9.9.9.9"
			ip link set "$_if" up; ip addr add "$_ip" dev "$_if" && ip route replace default via "$_gw" dev "$_if" onlink
			: >/etc/resolv.conf; for _d in $_dns; do echo "nameserver $_d" >>/etc/resolv.conf; done
			printf '\n  Pass the same to the installer: w3bs-os-install --ip %s --gateway %s --dns "%s"\n\n' "$_ip" "$_gw" "$_dns"
			_w3bs_ssh_info
			unset W3BS_OS_OFFERED; . /etc/profile.d/w3bs-os-live.sh ;;
		*) printf '\n  Run w3bs-os-install-gui (graphical) or w3bs-os-install (text) when you are ready.\n\n' ;;
	esac
	unset _w3bs_pick _w3bs_def _w3bs_a
fi
