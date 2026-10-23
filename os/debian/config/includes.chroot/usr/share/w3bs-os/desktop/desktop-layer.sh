#!/bin/sh
# The Sway fallback session's Agentic Desktop Layer (W3BS DE starts its own: w3bs-de autostart).
# Not installed: nothing to start.
[ -x /usr/local/bin/w3bs-desktop ] || exit 0
# Starts the Agentic Desktop Layer in the session: the broker (agent access starts
# Disabled) and the tray. The tray needs a StatusNotifier host, which is Waybar here; Waybar
# starts at the same moment, so wait for its watcher on the session bus before the tray.
log=${XDG_STATE_HOME:-$HOME/.local/state}/w3bs-os/desktop-layer.log
mkdir -p "${log%/*}"
exec >>"$log" 2>&1
echo "$(date '+%F %T') starting the Agentic Desktop Layer"
/usr/local/bin/w3bs-desktop daemon --adapter auto --mcp-http 127.0.0.1:7801 &
# Waybar owns the StatusNotifierWatcher name a moment before it registers itself as the
# host, and the tray exits at once ("no StatusNotifierItem host") if it starts in between:
# wait for the host, and restart the tray if it still exits early.
host_ready() {
	dbus-send --session --print-reply --dest=org.kde.StatusNotifierWatcher /StatusNotifierWatcher \
		org.freedesktop.DBus.Properties.Get string:org.kde.StatusNotifierWatcher \
		string:IsStatusNotifierHostRegistered 2>/dev/null | grep -q 'boolean true'
}
i=0
until host_ready; do
	i=$((i + 1))
	[ "$i" -ge 120 ] && { echo "no StatusNotifier host after 60s; starting the tray anyway"; break; }
	sleep 0.5
done
n=0
while [ "$n" -lt 5 ]; do
	n=$((n + 1))
	t0=$(date +%s)
	/usr/local/bin/w3bs-desktop tray --wait-for-broker 60
	[ $(( $(date +%s) - t0 )) -ge 30 ] && break
	echo "the tray exited early (try $n of 5); starting it again"
	sleep 3
done &
wait
