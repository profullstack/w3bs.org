#!/bin/sh
# Headless Hyprland (0.56, Lua config) in Docker, for driving agent automation against a real
# Hyprland session: grim screenshots, wtype keyboard, wlrctl pointer, hyprctl IPC.
#
#   desktop/tests/rig/hyprland.sh            # build image, (re)start container, wait, print exec env
#   desktop/tests/rig/hyprland.sh stop       # remove the container
#   desktop/tests/rig/hyprland.sh env        # print the -e flags of the running session
#   desktop/tests/rig/hyprland.sh exec CMD.. # docker exec CMD inside the session (any shell)
#   W3BS_RIG_WORK=/path/to/bin desktop/tests/rig/hyprland.sh   # mounted read-only at /work
#
# How it works: Hyprland runs its normal DRM backend on the host's /dev/dri/card* (opened through
# seatd, non-VT-bound) and renders with Mesa on /dev/dri/renderD*. A headless box has every
# connector disconnected, so the entrypoint adds a virtual output (`hyprctl output create headless`).
# Hyprland 0.56 cannot start on the headless backend alone: without a DRM device aquamarine fails
# with "Cannot open backend: no allocator available". Builds context: ./hyprland/.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
NAME=${NAME:-w3bs-hyprland-rig}
IMAGE=${IMAGE:-w3bs-hyprland-rig:latest}
CARD=${CARD:-$(ls /dev/dri/card* 2>/dev/null | head -1)}
RENDER=${RENDER:-$(ls /dev/dri/renderD* 2>/dev/null | head -1)}

print_env() {
  docker exec "$NAME" cat /run/user/0/rig.env | sed -n 's/^export //p' | tr ' ' '\n' | sed 's/^/-e /' | tr '\n' ' '
  echo
}

# shellcheck disable=SC2046
case "${1:-start}" in
  stop) docker rm -f "$NAME" >/dev/null 2>&1 || true; echo "stopped $NAME"; exit 0 ;;
  env)  print_env; exit 0 ;;
  # print_env is a list of -e flags: word splitting is intended.
  exec) shift; exec docker exec $(print_env) "$NAME" "$@" ;;
  start) ;;
  *) echo "usage: $0 [start|stop|env|exec CMD...]" >&2; exit 64 ;;
esac

[ -n "$CARD" ] && [ -n "$RENDER" ] || { echo "need /dev/dri/card* and /dev/dri/renderD* on this host" >&2; exit 2; }
docker build -q -t "$IMAGE" "$HERE/hyprland" >/dev/null
docker rm -f "$NAME" >/dev/null 2>&1 || true   # one rig at a time

set -- --memory 6g --cpus 4 --init --device "$CARD" --device "$RENDER" -e AQ_DRM_DEVICES="$CARD"
[ -n "${W3BS_RIG_WORK:-}" ] && set -- "$@" -v "$(cd "$W3BS_RIG_WORK" && pwd)":/work:ro
docker run -d --name "$NAME" "$@" "$IMAGE" >/dev/null

i=0
until docker exec "$NAME" test -s /run/user/0/rig.env 2>/dev/null; do
  i=$((i+1))
  if [ $i -gt 150 ] || [ "$(docker inspect -f '{{.State.Running}}' "$NAME" 2>/dev/null)" != true ]; then
    echo "Hyprland did not come up; last log lines:" >&2
    docker logs "$NAME" 2>&1 | tail -5 >&2
    docker exec "$NAME" sh -c 'tail -20 /var/log/rig/hyprland.log' >&2 2>/dev/null || true
    exit 1
  fi
  sleep 0.2
done

ENV=$(print_env)
echo "Hyprland $(docker exec -e XDG_RUNTIME_DIR=/run/user/0 "$NAME" Hyprland --version | head -1 | awk '{print $2}') running in $NAME"
docker exec $ENV "$NAME" hyprctl -j monitors | docker exec -i "$NAME" jq -r '.[] | "monitor \(.name) \(.width)x\(.height)"'
echo "exec env: $ENV"
echo "e.g.:     docker exec $ENV $NAME hyprctl -j clients"
echo "logs:     docker exec $NAME cat /var/log/rig/hyprland.log"
