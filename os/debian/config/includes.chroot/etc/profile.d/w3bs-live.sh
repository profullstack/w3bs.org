# W3BS OS live installer: the first screen on tty1.
if [ "$(id -u)" = 0 ] && [ "$(tty 2>/dev/null)" = /dev/tty1 ] && [ -z "${W3BS_OS_OFFERED:-}" ] && [ -d /run/live/medium ]; then
	export W3BS_OS_OFFERED=1
	clear 2>/dev/null
	printf '\n  W3BS OS %s installer (Debian 13)\n\n' "$(cat /etc/w3bs-os-version 2>/dev/null)"
	_pw=$(cat /run/w3bs-live-ssh-password 2>/dev/null)
	_addrs=$(ip -o addr show scope global 2>/dev/null | awk '{ sub(/\/.*/, "", $4); print $4 }')
	if [ -n "$_addrs" ]; then
		printf '  SSH is on: install from another computer (copy and paste work there):\n'
		for _a in $_addrs; do printf '    ssh root@%s\n' "$_a"; done
		printf '  One-time root password: %s\n\n' "$_pw"
	else
		printf '  No network address yet (DHCP and IPv6 keep trying).\n\n'
	fi
	# The first question: the graphical installer (default with a display) or the text one.
	_def=1; ls /sys/class/drm/card[0-9]*/device >/dev/null 2>&1 || _def=2
	printf '  How do you want to install? (it erases a disk you choose, after a review)\n'
	printf '    1  Graphical installer (mouse and windows)%s\n' "$( [ $_def = 1 ] && echo '  [default]' || echo '  (no display found)')"
	printf '    2  Text installer (command line)%s\n' "$( [ $_def = 2 ] && echo '  [default]')"
	printf '    3  Shell: do not install now\n\n  Choose 1, 2 or 3 [%s]: ' "$_def"
	read -r _pick; [ -n "$_pick" ] || _pick=$_def
	case "$_pick" in
		1*) /usr/local/sbin/w3bs-os-install-gui
			case $? in 0) ;; 10) /usr/local/sbin/w3bs-os-install ;; *) printf '\n  Starting the text installer instead.\n'; /usr/local/sbin/w3bs-os-install ;; esac ;;
		2*) /usr/local/sbin/w3bs-os-install ;;
		*) printf '\n  Run w3bs-os-install-gui (graphical) or w3bs-os-install (text) when you are ready.\n\n' ;;
	esac
	unset _pw _addrs _a _pick _def
fi
