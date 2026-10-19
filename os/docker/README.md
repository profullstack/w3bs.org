# W3BS OS Docker editions

The first W3BS OS release (docs/os-prd.md §20): Alpine Linux images with zsh, the `w3bs`
CLI and, on `desktop`, KDE Plasma with the Agentic Desktop Layer installed.

## server

```sh
docker run -it --rm w3bs.org/os:server
w3bs install hqtui          # any package on w3bs.org/packages
```

Alpine stable, zsh with a git-aware prompt, the `w3bs` CLI (built for musl), git, curl,
OpenSSH client and Node. The official W3BS repository is added on first start, pinned to its
root fingerprint. Runs as user `w3bs` (uid 1000).

## desktop

KDE Plasma on Wayland, rendered on the CPU, on a virtual display the host provides. Needs a
Linux host with the `vkms` module (no GPU required):

```sh
sudo modprobe vkms
card=$(for c in /sys/class/drm/card*; do readlink -f $c/device | grep -q vkms && basename $c; done | head -1)
docker run -d --name w3bs-desktop --device /dev/dri/$card --group-add video --shm-size 512m \
  -e W3BS_VNC_PASSWORD=change-me -p 5900:5900 w3bs.org/os:desktop
```

Then connect any VNC viewer to port 5900. Or, with `--network host` and no password, share
it through w3bs.org/fleet: the session serves VNC on the owner-only socket `/vnc/vnc.sock`.

| Variable | Default | Meaning |
| --- | --- | --- |
| `W3BS_DESKTOP_ENABLED` | `0` | `1` lets agents observe and drive from the start. Otherwise access is off until a person turns it on from the tray, as on any W3BS machine |
| `W3BS_VNC_PASSWORD` | unset | Also serve VNC on TCP 5900 with this password |
| `W3BS_GEOMETRY` | `1920x1080` | Display size |
| `W3BS_MCP` | `127.0.0.1:7801` | The desktop layer's MCP endpoint (bearer token required) |
| `W3BS_DRM` | the vkms card | KMS device KWin drives |

Agents register with `docker exec w3bs-desktop w3bs-desktop agents add <name> --preset broad`
and connect over MCP. `os/docker/check-desktop.sh` is the acceptance check: an agent looks,
clicks Konsole, types a command that runs, takes a 1920x1080 screenshot, and is refused once
access is disabled.

## Build

```sh
docker buildx build -f os/docker/Dockerfile --target server  -t w3bs-os:server  --load .
docker buildx build -f os/docker/Dockerfile --target desktop -t w3bs-os:desktop --load .
```

The `w3bs` CLI and `w3bs-desktop` are compiled from this checkout for musl (their release
binaries are glibc builds). `os/docker/Dockerfile.dockerignore` lists what the build reads.

## Web server

Both images include **Caddy**, the W3BS OS web server: virtual hosts, reverse proxies and
static sites, with automatic HTTPS and IPv6 by default. `/etc/caddy/Caddyfile` only imports
`/etc/caddy/sites/*`, so a site is one file:

```sh
docker run -d --name web -p 80:80 -p 443:443 -v w3bs-caddy:/data --user root \
  w3bs.org/os:server sh -c "echo 'example.com { reverse_proxy app:3000 }' > /etc/caddy/sites/example && caddy run --config /etc/caddy/Caddyfile"
```
