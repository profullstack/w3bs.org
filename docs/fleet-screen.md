# Fleet screens: watch a machine's desktop from w3bs.org/fleet

When an AI agent works on a desktop you are not sitting at (a headless KDE box, a server
running the [Agentic Desktop Layer](/desktop), a machine in another room), open
**w3bs.org/fleet**, pick the machine and press **View screen**. The page shows its desktop live
in the browser (noVNC), view only unless you allow control.

**A server with no desktop at all?** One command gives it one: see
[A headless desktop for AI agents](#a-headless-desktop-for-ai-agents).

## How it works

```
browser (noVNC) --wss--> w3bs.org relay <--wss-- w3bs fleet agent --> local VNC server
                                                                    (TCP or a Unix socket)
```

- **Nothing on the machine listens to the network.** The fleet agent (`w3bs fleet agent`, the
  `w3bs-fleet` service) keeps one outbound WebSocket to w3bs.org while sharing is on. When you
  open the screen page, w3bs.org hands the agent a one-time session id; the agent dials back
  and connects that session to the VNC server on the machine's loopback.
- **Off by default, switched on the machine.** Only someone on the machine can turn sharing on
  (`w3bs fleet screen on`, or `--screen` when joining). The page cannot.
- **Only you.** The viewer needs your w3bs.org session on w3bs.org itself (the relay checks the
  page's origin), and the machine must be in your fleet. A machine's agent key cannot watch
  anything.
- **View only means view only.** In view mode the agent follows the RFB protocol and drops every
  message that could change the machine: keys, pointer, clipboard, screen resize and power
  (xvp) requests. A modified page still cannot type. With `--control` the stream passes through
  and the page's **Control** switch (off at the start of every session) sends your mouse and
  keyboard.
- **Logged.** Every session appears in the machine's recent actions on /fleet: how long, which
  mode, and from which address. The agent's log (`journalctl --user -u w3bs-fleet`) records
  each session and how many input messages it dropped.
- **The VNC password stays in your browser.** VNC authentication is a challenge and response;
  w3bs.org relays the response and never sees the password.

## A headless desktop for AI agents

On a Linux server with Docker and no desktop, after `w3bs fleet join`:

```sh
w3bs fleet desktop start             # view only from /fleet
w3bs fleet desktop start --control   # also mouse and keyboard from /fleet
```

That is all. It runs a **Kubuntu desktop in a Docker container** (`w3bs-desktop`) and shares its
screen on /fleet: press **View screen**. The first start builds the image (a few minutes);
after that it starts in seconds, and Docker restarts it with the machine.

What is in the container:

- **KDE Plasma** (KWin, Plasma shell, Konsole) from Ubuntu 26.04's packages.
- **No GPU needed.** KWin drives the kernel's `vkms` virtual display and Mesa renders OpenGL on
  the CPU. `start` loads `vkms` (asking `sudo` once) and keeps it loaded across reboots.
- **The [Agentic Desktop Layer](/desktop)**, enabled from the start, with its MCP endpoint on
  `http://127.0.0.1:7802/mcp` (not 7801, so it never clashes with a desktop layer on the host).
- **TigerVNC's `w0vncserver`**, built from source in the image (Ubuntu's TigerVNC is too old to
  have it), listening only on a Unix socket in `~/.local/share/w3bs/desktop/vnc/` that only
  your user can open. No TCP port, no VNC password to manage.
- Its home directory is `~/.local/share/w3bs/desktop/home` on the host, kept across restarts.

Give an AI agent the desktop:

```sh
w3bs fleet desktop agent claude      # registers it, prints the `claude mcp add ...` line
w3bs fleet desktop agent watcher --preset observe   # look, never touch
```

Run the printed line on the same machine; the agent then sees and drives the Kubuntu desktop
through MCP, and you watch it on /fleet.

```sh
w3bs fleet desktop status            # running? shared? image current?
w3bs fleet desktop logs              # container log; session logs are under home/.local/state
w3bs fleet desktop start --rebuild   # rebuild the image (after a w3bs update changes it)
w3bs fleet desktop start --geometry 2560x1440
w3bs fleet desktop stop              # remove the container and stop sharing; home is kept
```

## Turn it on (your own VNC server)

On the machine, after it joined your fleet:

```sh
w3bs fleet screen on                 # view only, VNC server on 127.0.0.1:5900
w3bs fleet screen on --control       # also allow mouse and keyboard from the page
w3bs fleet screen on --vnc 127.0.0.1:5901
w3bs fleet screen on --vnc unix:/run/user/1000/vnc.sock   # a Unix socket
w3bs fleet screen                    # show the setting and whether the VNC server answers
w3bs fleet screen off                # stop; open sessions end within one report (30 s)
```

Or join with it: on /fleet pick **Screen: View only** (or **View and control**) before making
the join command, which adds `--screen view` to it. `w3bs fleet join <token> --screen view
--vnc 127.0.0.1:5900` does the same by hand.

The running agent picks the change up on its next report; no restart. From any computer signed
in with `w3bs login`, `w3bs fleet view <machine>` opens the page.

## The VNC server

The agent shares whatever VNC server answers on the address you gave. Bind it to loopback only
(the agent is on the same machine), and give it a password: the page asks for it. View-only
sessions need the server to offer **no authentication or a VNC password** (security types None
or VncAuth); the agent hides other types (TLS, VeNCrypt) from the page so it chooses one of
those two.

### KDE Plasma (Wayland), including headless boxes

TigerVNC 1.16's `w0vncserver` shares the running Wayland session through the desktop portal
(Fedora: `dnf install tigervnc-wayland-server tigervnc-server-common`).

```sh
mkdir -p ~/.config/tigervnc
vncpasswd ~/.config/tigervnc/passwd
w0vncserver -localhost -rfbport 5900 -SecurityTypes VncAuth \
  -PasswordFile ~/.config/tigervnc/passwd -AlwaysShared
```

On a desktop with a person at it, KDE asks once to allow remote control. On a headless box
there is nobody to click that, so pre-grant it (the same grant the Agentic Desktop Layer uses);
`w0vncserver` opens a portal session for each viewer, so keep the grant in place:

```sh
flatpak permission-set kde-authorized remote-desktop "" yes
```

This is the setup our end-to-end rig runs (`desktop/tests/rig/fleet-screen.sh`): KDE Plasma in
Docker, `w0vncserver`, the real /fleet page in Chromium.

KDE's own `krfb` 26.08 does not work for this: it never opens its port unless started as
`krfb-virtualmonitor`, which shares a new, empty monitor instead of the desktop.

### Other desktops (not yet tested by us)

- **GNOME (Wayland):** `w0vncserver` uses the same portal and should work; GNOME shows its
  sharing dialog. We have not tried GNOME's built-in Remote Desktop in VNC mode.
- **Sway, Hyprland and other wlroots compositors:** `wayvnc 127.0.0.1 5900`.
- **X11:** `x11vnc -localhost -rfbport 5900 -shared -forever -rfbauth ~/.vnc/passwd`.

The fleet agent runs on Linux today (it installs as a systemd service).

## When it does not connect

The page shows the reason it got:

| Message                                                 | What to do                                                                    |
| ------------------------------------------------------- | ----------------------------------------------------------------------------- |
| This machine does not share its screen                  | On the machine: `w3bs fleet screen on`                                        |
| The machine's agent is not connected                    | `w3bs fleet status` on the machine; is the `w3bs-fleet` service running?      |
| No VNC server answers on 127.0.0.1:5900                 | Start the VNC server, or point `--vnc` at the right port                      |
| needs a security type a view-only session cannot follow | Allow VNC password auth (`-SecurityTypes VncAuth`), or share with `--control` |
| The machine did not answer                              | The agent missed the session; reload in a few seconds                         |

A session that connects but stays black is usually the desktop waiting on a consent dialog
nobody can see; see the pre-grant above.
