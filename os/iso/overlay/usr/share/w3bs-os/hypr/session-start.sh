#!/bin/sh
# W3BS OS Hyprland session start (exec-once): VM tweaks, then a first-login welcome.
if [ -n "${W3BS_VM:-}" ]; then
	hyprctl keyword cursor:no_hardware_cursors true >/dev/null
	# VMs report a 0x0 mm screen, and Hyprland's auto scale then picks 2x.
	hyprctl keyword monitor ,preferred,auto,1 >/dev/null
fi
f=${XDG_STATE_HOME:-$HOME/.local/state}/w3bs-os/welcomed
if [ ! -e "$f" ]; then
	sleep 4
	notify-send -t 30000 "Welcome to W3BS OS" \
		"Super+D apps · Super+Return terminal · Super+B Firefox · Super+Shift+B TronBrowser · Super+Shift+E log out"
	mkdir -p "${f%/*}" && touch "$f"
fi
