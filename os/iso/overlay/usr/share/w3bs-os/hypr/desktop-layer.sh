#!/bin/sh
# Starts the Agentic Desktop Layer in the Hyprland session: the broker (agent access starts
# Disabled) and the tray. The tray needs a StatusNotifier host, which is Waybar here; Waybar
# starts at the same moment, so wait for its watcher on the session bus before the tray.
log=${XDG_STATE_HOME:-$HOME/.local/state}/w3bs-os/desktop-layer.log
mkdir -p "${log%/*}"
exec >>"$log" 2>&1
echo "$(date '+%F %T') starting the Agentic Desktop Layer"
/usr/local/bin/w3bs-desktop daemon --adapter auto --mcp-http 127.0.0.1:7801 &
i=0
until dbus-send --session --print-reply --dest=org.freedesktop.DBus /org/freedesktop/DBus \
	org.freedesktop.DBus.NameHasOwner string:org.kde.StatusNotifierWatcher 2>/dev/null | grep -q 'boolean true'; do
	i=$((i + 1))
	[ "$i" -ge 60 ] && { echo "no StatusNotifier watcher after 30s; starting the tray anyway"; break; }
	sleep 0.5
done
/usr/local/bin/w3bs-desktop tray --wait-for-broker 60 &
wait
