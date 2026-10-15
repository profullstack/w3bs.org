#!/bin/sh
# Open Google Chrome in the agent desktop: `w3bs-browser [URL]`.
# --no-sandbox: Chrome's sandbox needs user namespaces, which this container is not given; the
# container is the boundary here. A separate profile keeps agent sign-ins out of anything else.
exec google-chrome --no-sandbox --ozone-platform=wayland --no-first-run --no-default-browser-check \
  --password-store=basic --hide-crash-restore-bubble --start-maximized \
  --user-data-dir="${W3BS_BROWSER_PROFILE:-$HOME/.config/w3bs-browser}" "$@"
