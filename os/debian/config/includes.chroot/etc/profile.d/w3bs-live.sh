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
	printf '  Install W3BS OS on this machine now? (it erases a disk you choose, after a review)\n'
	printf '    1  Yes, start the installer  [default]\n    2  No, give me a shell\n\n  Choose 1 or 2 [1]: '
	read -r _pick
	case "$_pick" in 2*) printf '\n  Run w3bs-os-install when you are ready.\n\n' ;; *) /usr/local/sbin/w3bs-os-install ;; esac
	unset _pw _addrs _a _pick
fi
