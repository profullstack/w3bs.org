#!/bin/sh
# W3BS OS Hyprland session start (exec-once): the wallpaper, then a first-login welcome.
# Wallpaper (the keys to open things). Started from exec-once directly it died at once:
# start it from here and restart it if it exits.
(
	n=0
	while [ "$n" -lt 20 ]; do
		n=$((n + 1))
		swaybg -m fill -i /usr/share/w3bs-os/wallpaper.png -c '#101418' >/dev/null 2>&1
		sleep 2
	done
) &
f=${XDG_STATE_HOME:-$HOME/.local/state}/w3bs-os/welcomed
if [ ! -e "$f" ]; then
	sleep 4
	notify-send -t 30000 "Welcome to W3BS OS" \
		"Super+D apps · Super+Return terminal · Super+B Firefox · Super+Shift+B TronBrowser · Super+Shift+E log out"
	mkdir -p "${f%/*}" && touch "$f"
fi
