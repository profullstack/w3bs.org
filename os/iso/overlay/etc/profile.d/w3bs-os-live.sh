# W3BS OS live installer: offer the installer once on tty1.
if [ "$(id -u)" = 0 ] && [ "$(tty 2>/dev/null)" = /dev/tty1 ] && [ -z "${W3BS_OS_OFFERED:-}" ] \
	&& [ -e /etc/w3bs-os/live ] && [ -x /usr/local/sbin/w3bs-os-install ]; then
	export W3BS_OS_OFFERED=1
	printf '\n  W3BS OS %s network installer\n' "$(cat /usr/share/w3bs-os/VERSION 2>/dev/null)"
	printf '  It erases a disk you choose, after a review. A wired network is required.\n\n'
	printf '  Start the installer now? [Y/n] '
	read -r _w3bs_ans
	case "$_w3bs_ans" in
		n*|N*) printf '\n  Run w3bs-os-install when you are ready.\n\n' ;;
		*) /usr/local/sbin/w3bs-os-install ;;
	esac
	unset _w3bs_ans
fi
