#!/usr/bin/env python3
"""QEMU test for the W3BS OS 1.2 (Debian) ISO: live boot (network, DNS, SSH password), an
unattended install, then the installed system (login, sudo, tools, no failed user units).

  dtest.py --iso X --disk D.qcow2 --firmware uefi|bios [--encrypt] --out DIR
Uses os/iso/test/vmtest.py's VM class (KVM when available). Exits non-zero on any failure.
"""
import argparse, os, re, sys, time
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "iso", "test"))
from vmtest import VM, log  # noqa: E402

p = argparse.ArgumentParser()
p.add_argument("--iso", required=True); p.add_argument("--disk", required=True)
p.add_argument("--firmware", default="uefi"); p.add_argument("--encrypt", action="store_true")
p.add_argument("--desktop", action="store_true", help="install the Hyprland desktop and log in at the greeter")
p.add_argument("--gui", action="store_true", help="install through the graphical installer (implies --desktop)")
p.add_argument("--cidata", action="store_true", help="a VPS without DHCP: the static address comes on a cloud-init NoCloud CD")
p.add_argument("--out", default="out"); p.add_argument("--timeout", type=int, default=3000)
a = p.parse_args()
if a.gui:
    a.desktop = True
os.makedirs(a.out, exist_ok=True)
fails = []

def check(name, ok, detail=""):
    log(f"{name}: {'PASS' if ok else 'FAIL'} {detail.strip()[-200:]}")
    if not ok:
        fails.append(name)

# --cidata: like Luxvps and other Proxmox hosts, the address is only on a "cidata" CD (network-config
# v1, matched by MAC). QEMU's user network still offers DHCP (.15): the checks want the seed's .77.
extra, CIDATA_IP = [], "10.0.2.77"
if a.cidata:
    import subprocess
    seed = os.path.join(a.out, "seed"); os.makedirs(seed, exist_ok=True)
    open(os.path.join(seed, "meta-data"), "w").write("instance-id: dtest\n")
    open(os.path.join(seed, "user-data"), "w").write("#cloud-config\n")
    open(os.path.join(seed, "network-config"), "w").write(
        "version: 1\nconfig:\n  - type: physical\n    name: eth0\n    mac_address: '52:54:00:12:34:56'\n    subnets:\n"
        f"    - type: static\n      address: '{CIDATA_IP}'\n      netmask: '255.255.255.0'\n      gateway: '10.0.2.2'\n"
        "  - type: nameserver\n    address:\n    - '10.0.2.3'\n")
    subprocess.check_call(["genisoimage", "-quiet", "-output", os.path.join(a.out, "seed.iso"), "-volid", "cidata", "-joliet", "-rock",
                           os.path.join(seed, "meta-data"), os.path.join(seed, "user-data"), os.path.join(seed, "network-config")])
    extra = ["-drive", f"file={os.path.join(a.out, 'seed.iso')},media=cdrom,index=3"]

vm = VM(f"live-{a.firmware}", a.firmware, iso=a.iso, disks=[a.disk], mem=4096, outdir=a.out, extra=extra)
try:
    vm.expect(r"root@w3bs-os:~# |# ", 600); time.sleep(20)
    rc, o = vm.sh("cat /etc/w3bs-os-version; curl -s -o /dev/null -w 'MIRROR=%{http_code}\\n' http://deb.debian.org/debian/dists/trixie/Release; "
                  "systemctl is-active systemd-networkd ssh; cat /run/w3bs-live-ssh-password; "
                  "who | grep -c '^root *tty1' | sed 's/^/TTY1ROOT=/'", 120)
    check("live network + DNS", "MIRROR=200" in o, o)
    check("live tty1: root, the installer menu", "TTY1ROOT=1" in o, o)
    if a.cidata:
        _, n = vm.sh("ip -4 -br addr | grep -c ' 10.0.2.77/24' | sed 's/^/CIDATA_LIVE=/'; cat /run/w3bs-net.env", 60)
        check("live static network from the cloud-init seed", "CIDATA_LIVE=1" in n, n)
    _, n = vm.sh("systemctl is-enabled ifup@.service networking.service 2>&1 | grep -c masked | sed 's/^/IFUPDOWN_MASKED=/'; pgrep -c 'dhclient|dhcpcd' | sed 's/^/DHCPCLIENTS=/'", 60)
    check("live network is systemd-networkd only (no ifupdown DHCP client)", "IFUPDOWN_MASKED=2" in n and "DHCPCLIENTS=0" in n, n)
    check("live ssh one-time password", re.search(r"\b[a-z0-9]{4}-[a-z0-9]{4}-[a-z0-9]{4}\b", o) is not None, o)
    vm.screenshot(os.path.join(a.out, f"live-{a.firmware}.png"))
    if a.gui:
        # The real first screen on tty1: 1 = the graphical installer; it clicks through by itself
        # (W3BS_GUI_AUTOWALK, read by w3bs-os-install-gui) with the test password.
        env = "W3BS_GUI_AUTOWALK=7 W3BS_GUI_TEST_PASSWORD=w3bs-test-pass" + (" W3BS_GUI_TEST_PASSPHRASE=w3bs-luks-pass" if a.encrypt else " W3BS_GUI_TEST_NOENCRYPT=1")
        # The checks after the reboot read ttyS0: the test asks for that console like the text path does.
        vm.sh(f"printf 'export {env} W3BS_GUI_TEST_ARGS=\\x27--serial-console --domain ci.example.test\\x27\\n' > /run/w3bs-gui.env", 30)
        vm.sendkeys("1\n")
        rc, o = vm.sh("for i in $(seq 1 90); do pidof cage >/dev/null && break; sleep 2; done; echo GUI=$(pidof cage)", 240)
        check("graphical installer starts", re.search(r"GUI=\d", o) is not None, o)
        for n in range(1, 6):
            time.sleep(8); vm.screenshot(os.path.join(a.out, f"gui-{n}.png"))
        rc, o = vm.sh("for i in $(seq 1 400); do grep -q -E 'W3BS_GUI_INSTALLED|cage exited' /var/log/w3bs-os-install-gui.log && break; sleep 5; done; "
                      "grep -c W3BS_GUI_INSTALLED /var/log/w3bs-os-install-gui.log | sed 's/^/GUI_OK=/'; tail -3 /var/log/w3bs-os-install.log", a.timeout)
        time.sleep(3); vm.screenshot(os.path.join(a.out, "gui-done.png"))
        check("install", "GUI_OK=1" in o, o)
        if "GUI_OK=1" not in o:
            _, t = vm.sh("tail -40 /var/log/w3bs-os-install.log; tail -20 /var/log/w3bs-os-install-gui.log", 60); print(t)
    else:
        vm.sh("printf 'w3bs-test-pass\\n' > /tmp/pw; printf 'w3bs-luks-pass\\n' > /tmp/lk", 30)
        flags = "--passphrase-file /tmp/lk" if a.encrypt else "--no-encrypt"
        flags += " --desktop hyprland" if a.desktop else " --server"
        rc, o = vm.sh(f"w3bs-os-install --defaults --disk /dev/vda --yes --no-reboot --serial-console --password-file /tmp/pw "
                      f"--domain ci.example.test {flags} > /tmp/o 2>&1; echo RC=$?; grep -E 'warning|FAILED|installed on' /tmp/o", a.timeout)
        # The console echoes the command (which itself says 'warning'): count the installer's lines only.
        check("install", re.search(r"RC=0\b", o) is not None and re.search(r"warning: ", o) is None, o)
        if "RC=0" not in o:
            _, t = vm.sh("tail -40 /var/log/w3bs-os-install.log", 60); print(t)
finally:
    vm.stop()

if "install" not in fails:
    vm = VM(f"installed-{a.firmware}", a.firmware, disks=[a.disk], mem=4096, outdir=a.out, extra=extra)
    try:
        if a.encrypt:
            vm.expect(r"[Pp]assphrase|unlock disk", 300); time.sleep(2); vm.send("w3bs-luks-pass\n")
        vm.expect(r"\S+ login: ", 600); time.sleep(3)
        if a.desktop:
            # The first user is logged in at boot, straight to the lock screen (hyprlock): the
            # password unlocks it.
            time.sleep(90); vm.screenshot(os.path.join(a.out, f"locked-{a.firmware}.png"))
            vm.sendkeys("w3bs-test-pass\n"); time.sleep(15)
            vm.screenshot(os.path.join(a.out, f"desktop-{a.firmware}.png"))
        vm.send("w3bs\n"); vm.expect(r"[Pp]assword: ", 60); vm.send("w3bs-test-pass\n")
        time.sleep(15); vm.send("PS1='> '; PROMPT='> '\n"); time.sleep(2)
        rc, o = vm.sh("grep -E '^(ID|VERSION)=' /etc/w3bs-os-release; echo SHELL=$SHELL; "
                      "curl -s -o /dev/null -w 'NET=%{http_code}\\n' http://deb.debian.org/debian/dists/trixie/Release; "
                      "echo w3bs-test-pass | sudo -S -p '' id -u; "
                      "for c in w3bs mise node bun pnpm readm3 hqterm hqsh threatcrush; do command -v $c >/dev/null && echo HAVE_$c; done; "
                      "systemctl --failed --no-legend --no-pager | grep -c 'user@' | sed 's/^/USER_UNITS_FAILED=/'", 300)
        check("os-release", "ID=w3bs-os" in o, o)
        check("zsh", "SHELL=/usr/bin/zsh" in o, o)
        check("network", "NET=200" in o, o)
        check("sudo", re.search(r"^0\s*$", o, re.M) is not None, o)
        for c in ("w3bs", "mise", "node", "bun", "pnpm", "readm3", "hqterm", "hqsh", "threatcrush"):
            check(f"tool {c}", f"HAVE_{c}" in o, "")
        check("user session units", "USER_UNITS_FAILED=0" in o, o)
        if a.cidata:
            rc, o = vm.sh("ip -4 -br addr | grep -c ' 10.0.2.77/24' | sed 's/^/CIDATA_INST=/'; grep -l 10.0.2.77 /etc/systemd/network/*.network", 60)
            check("installed system keeps the provider's static address", "CIDATA_INST=1" in o, o)
        if a.desktop:
            rc, o = vm.sh("echo HYPR=$(pidof Hyprland) WAYBAR=$(pidof waybar) SWAYNC=$(pidof swaync); "
                          "tr '\\0' ' ' </proc/$(pidof Hyprland | cut -d' ' -f1)/cmdline | grep -c 'w3bs-de/hypr/hyprland.lua' | sed 's/^/DECFG=/'; "
                          "cat /run/user/1000/hypr/*/hyprland.log 2>/dev/null | grep -ciE 'config error|lua.*error|error.*lua' | sed 's/^/CFGERR=/'", 120)
            check("Hyprland session", re.search(r"HYPR=\d", o) is not None, o)
            check("W3BS DE config (Lua)", "DECFG=1" in o, o)
            check("Waybar", re.search(r"WAYBAR=\d", o) is not None, o)
            check("notification center (swaync)", re.search(r"SWAYNC=\d", o) is not None, o)
            check("Hyprland config has no errors", "CFGERR=0" in o, o)
            rc, o = vm.sh("grep -c '^user = \"w3bs\"' /etc/greetd/config.toml | sed 's/^/AUTOLOGIN=/'; "
                          "grep -c 'Unlocked' ~/.local/state/w3bs-os/hyprland-session.log | sed 's/^/UNLOCKED=/'; "
                          "test -S /run/user/1000/w3bs-vnc.sock && echo VNCSOCK=1; stat -c VNCMODE=%a /run/user/1000", 60)
            check("autologin at boot (greetd initial_session)", "AUTOLOGIN=1" in o, o)
            check("boots to the lock screen, unlocked from the keyboard", re.search(r"UNLOCKED=[1-9]", o) is not None, o)
            check("VNC on the session's private socket (fleet screen)", "VNCSOCK=1" in o and "VNCMODE=700" in o, o)
            rc, o = vm.sh("test -x ~/.local/opt/hqterm-desktop/app/hqterm-desktop -o -x ~/.local/opt/hqterm-desktop/hqterm-desktop.AppImage && echo HQDESK=1; grep -c '^Exec' ~/.local/share/applications/hqterm.desktop | sed 's/^/HQENTRY=/'", 60)
            check("hqterm desktop app + launcher entry", "HQDESK=1" in o and "HQENTRY=1" in o, o)
            rc, o = vm.sh("w3bs-de doctor; echo DOCTOR=$?", 120)
            check("w3bs-de doctor", "DOCTOR=0" in o, o)
            rc, o = vm.sh("for i in $(seq 1 30); do [ $(pidof w3bs-desktop | wc -w) -ge 2 ] && break; sleep 2; done; "
                          "echo ADL=$(pidof w3bs-desktop | wc -w); /usr/local/bin/w3bs-desktop status 2>&1 | head -3", 120)
            check("Agentic Desktop Layer (broker + tray)", re.search(r"ADL=([2-9]|\d\d)", o) is not None, o)
            check("agent access off", "DISABLED" in o.upper(), o)
    finally:
        vm.stop()
if fails:
    sys.exit("failed: " + ", ".join(fails))
log("PASS")
