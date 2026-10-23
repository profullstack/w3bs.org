#!/usr/bin/env python3
"""QEMU tests for the W3BS OS installer ISO (run inside os/iso/test/Dockerfile.qemu).

  vmtest.py boot      --iso X --firmware uefi|bios          boots to a login, screenshots tty1
  vmtest.py install   --iso X --disk D.qcow2 [--encrypt]    unattended install over serial
  vmtest.py inspect   --iso X --disk D.qcow2                mounts the installed root from the
                                                           live system and checks it
  vmtest.py installed --disk D.qcow2 [--login USER:PASS]    boots the installed disk (UEFI),
                                                           screenshots the greeter (and desktop)

Uses KVM when /dev/kvm exists, otherwise TCG (slow: allow an hour for a Plasma install).
Every command exits non-zero on failure.
"""
import argparse, os, re, socket, subprocess, sys, time

OVMF_CODE = next((p for p in ["/usr/share/OVMF/OVMF_CODE_4M.fd", "/usr/share/OVMF/OVMF_CODE.fd"] if os.path.exists(p)), None)
OVMF_VARS = next((p for p in ["/usr/share/OVMF/OVMF_VARS_4M.fd", "/usr/share/OVMF/OVMF_VARS.fd"] if os.path.exists(p)), None)


def log(*a):
    print(time.strftime("%H:%M:%S"), *a, flush=True)


class VM:
    def __init__(self, name, firmware="uefi", iso=None, disks=(), mem=4096, smp=4, outdir=".", vga="std"):
        self.name, self.outdir = name, outdir
        self.serial_path = os.path.join(outdir, f"{name}.serial.sock")
        self.mon_path = os.path.join(outdir, f"{name}.mon.sock")
        self.serial_log = open(os.path.join(outdir, f"{name}.serial.log"), "wb")
        for p in (self.serial_path, self.mon_path):
            if os.path.exists(p):
                os.unlink(p)
        kvm = os.path.exists("/dev/kvm") and os.access("/dev/kvm", os.W_OK)
        cmd = ["qemu-system-x86_64", "-machine", "q35", "-m", str(mem), "-smp", str(smp),
               "-display", "none", "-vga", vga,
               "-serial", f"unix:{self.serial_path},server=on,wait=off",
               "-monitor", f"unix:{self.mon_path},server=on,wait=off",
               "-netdev", "user,id=n0" + os.environ.get("VMTEST_NETDEV_EXTRA", ""), "-device", "virtio-net-pci,netdev=n0",
               "-device", "virtio-rng-pci"] + ([] if os.environ.get("VMTEST_ALLOW_REBOOT") else ["-no-reboot"])
        cmd += ["-accel", "kvm", "-cpu", "host"] if kvm else ["-accel", "tcg,thread=multi", "-cpu", "max"]
        log(f"[{name}] accel={'kvm' if kvm else 'tcg'} firmware={firmware}")
        if firmware == "uefi":
            vars_copy = os.path.join(outdir, f"{name}.OVMF_VARS.fd")
            if not os.path.exists(vars_copy):
                subprocess.check_call(["cp", OVMF_VARS, vars_copy])
            cmd += ["-drive", f"if=pflash,format=raw,readonly=on,file={OVMF_CODE}",
                    "-drive", f"if=pflash,format=raw,file={vars_copy}"]
        for d in disks:
            cmd += ["-drive", f"file={d},if=virtio,format=qcow2,cache=unsafe"]
        if disks and not iso:
            cmd += ["-boot", "order=c"]
        if iso:
            # Like a VPS "virtual DVD": a CD-ROM drive, booted first; the disk stays virtio.
            cmd += ["-cdrom", iso, "-boot", "order=dc"]
        self.proc = subprocess.Popen(cmd)
        self.buf = b""
        self.serial = self._connect(self.serial_path)
        self.serial.setblocking(False)

    def _connect(self, path):
        for _ in range(100):
            try:
                s = socket.socket(socket.AF_UNIX)
                s.connect(path)
                return s
            except OSError:
                time.sleep(0.2)
        raise SystemExit(f"cannot connect to {path}")

    def _pump(self, wait=0.5):
        end = time.time() + wait
        while time.time() < end:
            try:
                data = self.serial.recv(65536)
                if not data:
                    break
                self.buf += data
                self.serial_log.write(data)
                self.serial_log.flush()
            except BlockingIOError:
                time.sleep(0.1)
            if self.proc.poll() is not None:
                break

    def expect(self, pattern, timeout, since=None):
        rx = re.compile(pattern.encode() if isinstance(pattern, str) else pattern)
        start = len(self.buf) if since is None else since
        end = time.time() + timeout
        while time.time() < end:
            self._pump(1)
            m = rx.search(self.buf, start)
            if m:
                return m
            if self.proc.poll() is not None:
                raise SystemExit(f"[{self.name}] qemu exited ({self.proc.returncode}) while waiting for {pattern!r}")
        tail = self.buf[-2000:].decode(errors="replace")
        raise SystemExit(f"[{self.name}] timeout ({timeout}s) waiting for {pattern!r}; serial tail:\n{tail}")

    def send(self, text):
        self.serial.setblocking(True)
        self.serial.sendall(text.encode())
        self.serial.setblocking(False)

    def sh(self, command, timeout=600):
        """Run a shell command on the serial root shell and return (rc, output)."""
        tag = f"W3BSDONE{int(time.time()*1000) % 100000000}"
        mark = len(self.buf)
        self.send(f"{command}; echo {tag}=$?\n")
        m = self.expect(rf"{tag}=(\d+)", timeout, since=mark)
        out = self.buf[mark:m.start()].decode(errors="replace")
        return int(m.group(1)), out

    def monitor(self, command):
        s = self._connect(self.mon_path)
        s.settimeout(10)
        try:
            s.recv(4096)
        except socket.timeout:
            pass
        s.sendall((command + "\n").encode())
        time.sleep(1.5)
        try:
            out = s.recv(65536).decode(errors="replace")
        except socket.timeout:
            out = ""
        s.close()
        return out

    def screenshot(self, png):
        ppm = png[:-4] + ".ppm"
        self.monitor(f"screendump {ppm}")
        for _ in range(20):
            if os.path.exists(ppm) and os.path.getsize(ppm) > 0:
                break
            time.sleep(0.5)
        time.sleep(1)
        from PIL import Image
        Image.open(ppm).save(png, optimize=True)
        os.unlink(ppm)
        log(f"[{self.name}] screenshot {png} ({os.path.getsize(png)} bytes)")

    def sendkeys(self, text):
        names = {" ": "spc", "-": "minus", ".": "dot", "/": "slash", "_": "shift-minus", "\n": "ret",
                 ":": "shift-semicolon", "=": "equal"}
        for ch in text:
            if ch in names:
                k = names[ch]
            elif ch.isupper():
                k = "shift-" + ch.lower()
            else:
                k = ch
            self.monitor(f"sendkey {k}")

    def stop(self):
        if self.proc.poll() is None:
            self.monitor("quit")
            try:
                self.proc.wait(30)
            except subprocess.TimeoutExpired:
                self.proc.kill()


def serial_login(vm, timeout):
    vm.expect(r"login: ", timeout)
    time.sleep(2)
    vm.send("root\n")
    vm.expect(r"# ", 60)
    vm.sh("export PS1='# '; stty cols 200 2>/dev/null", 60)


def push_file(vm, src, dest):
    """Copy a local file into the live system over the serial shell (test iterations only)."""
    import base64
    data = base64.encodebytes(open(src, "rb").read()).decode()
    # cat reads the tty in canonical mode, so the shell's line editor never sees the data.
    vm.send(f"mkdir -p $(dirname {dest}); stty -echo; cat > {dest}.b64; stty echo\n")
    time.sleep(2)
    for line in data.splitlines():
        vm.send(line + "\n")
        time.sleep(0.01)
    time.sleep(1)
    vm.send("\x04")
    time.sleep(2)
    rc, _ = vm.sh(f"base64 -d {dest}.b64 > {dest} && chmod 755 {dest} && rm {dest}.b64 && if head -n 1 {dest} | grep -q '^#!/bin/sh'; then sh -n {dest}; fi", 60)
    if rc != 0:
        raise SystemExit(f"could not push {src}")
    log(f"[{vm.name}] pushed {src} -> {dest}")


def cmd_boot(a):
    vm = VM(f"boot-{a.firmware}", a.firmware, iso=a.iso, outdir=a.out)
    try:
        t0 = time.time()
        serial_login(vm, a.timeout)
        log(f"[{vm.name}] serial login after {time.time()-t0:.0f}s")
        rc, out = vm.sh("cat /etc/alpine-release; cat /usr/share/w3bs-os/VERSION; w3bs --version; w3bs-desktop --version; "
                        "ls /sys/firmware/efi >/dev/null 2>&1 && echo BOOT=UEFI || echo BOOT=BIOS; command -v w3bs-os-install", 120)
        print(out)
        if rc != 0:
            raise SystemExit("live system checks failed")
        want = "BOOT=UEFI" if a.firmware == "uefi" else "BOOT=BIOS"
        if want not in out:
            raise SystemExit(f"expected {want}")
        time.sleep(5)
        vm.screenshot(os.path.join(a.out, f"live-{a.firmware}-tty1.png"))
        # The VGA console: the first question is graphical or text installer; Enter takes
        # the default, the graphical installer (QEMU's VGA is a display), which must come up.
        vm.sendkeys("\n")
        rc, out = vm.sh("for i in $(seq 1 90); do pidof cage >/dev/null && break; sleep 2; done; "
                        "echo GUI_UP=$(pidof cage); tail -n 5 /var/log/w3bs-os-install-gui.log", 240)
        time.sleep(8)
        vm.screenshot(os.path.join(a.out, f"live-{a.firmware}-installer-gui.png"))
        if not re.search(r"GUI_UP=\d", out):
            raise SystemExit(f"the graphical installer did not start: {out[-600:]}")
        log(f"[{vm.name}] PASS")
    finally:
        vm.stop()


def vga_review(vm, a):
    """On tty1 (VGA), answer the interactive installer with its defaults up to the review
    screen, screenshot it, then cancel at the typed confirmation (nothing is written)."""
    vm.sendkeys("2\n")                      # first question: 2 = the text installer
    time.sleep(8)
    for _ in range(9):                      # install type, desktop, keymap, timezone, hostname, domain, user, full name, disk
        vm.sendkeys("\n")
        time.sleep(2)
    vm.sendkeys("\n")                       # encryption: yes
    time.sleep(2)
    # root password (Enter: none, root stays locked), user password twice, disk passphrase twice
    for secret in ("", "vgatestpw", "vgatestpw", "vgatestluks", "vgatestluks"):
        vm.sendkeys(secret + "\n")
        time.sleep(2)
    # The internet check (IPv6 then IPv4 probe, then the CDN) runs before the review.
    time.sleep(30)
    vm.screenshot(os.path.join(a.out, f"install-{a.firmware}-review-vga.png"))
    vm.sendkeys("no\n")                     # anything but the disk name cancels
    time.sleep(5)
    vm.screenshot(os.path.join(a.out, f"install-{a.firmware}-cancelled-vga.png"))
    rc, out = vm.sh("tail -n 3 /var/log/w3bs-os-install.log; blkid /dev/vda* || echo DISK_UNTOUCHED", 60)
    if "cancelled at review" not in out or "DISK_UNTOUCHED" not in out:
        raise SystemExit(f"VGA review/cancel did not behave: {out}")
    log(f"[{vm.name}] VGA interactive review shown and cancelled; disk untouched")


def cmd_install(a):
    vm = VM(f"install-{a.firmware}" + ("-crypt" if a.encrypt else ""), a.firmware, iso=a.iso, disks=[a.disk], mem=a.mem, outdir=a.out)
    try:
        serial_login(vm, a.timeout)
        vm.sh("printf '%s\\n' 'w3bs-test-pass' > /tmp/pw; printf '%s\\n' 'w3bs-test-luks-pass' > /tmp/luks; chmod 600 /tmp/pw /tmp/luks", 30)
        if a.push_installer:
            push_file(vm, a.push_installer, "/usr/local/sbin/w3bs-os-install")
        for spec in a.push:
            src, dest = spec.split("=", 1)
            push_file(vm, src, dest)
        if a.vga_review:
            vga_review(vm, a)
        disk = "" if a.auto_disk else "--disk /dev/vda "
        flags = f"--defaults {disk}--yes --no-reboot --serial-console --password-file /tmp/pw --desktop {a.desktop}"
        flags += " --passphrase-file /tmp/luks" if a.encrypt else " --no-encrypt"
        t0 = time.time()
        rc, out = vm.sh(f"w3bs-os-install {flags}", a.install_timeout)
        log(f"[{vm.name}] installer exited {rc} after {time.time()-t0:.0f}s")
        if rc != 0:
            vm.sh("tail -n 60 /var/log/w3bs-os-install.log", 60)
            raise SystemExit(f"installer failed with {rc}")
        if "W3BS OS" not in out or "is installed" not in out:
            raise SystemExit("installer exited 0 without the success message")
        if not re.search(r"Target disk\s+/dev/vda\s", out):
            raise SystemExit("the review did not show /dev/vda as the target disk")
        for leak in ("w3bs-test-pass", "w3bs-test-luks-pass"):
            r2, o2 = vm.sh(f"grep -c '{leak}' /var/log/w3bs-os-install.log || true", 30)
            if o2.strip().splitlines()[-1].strip() != "0":
                raise SystemExit(f"password leaked into the install log: {leak}")
        if a.encrypt:
            r, o = vm.sh("for p in /dev/vda?; do printf '%s ' $p; blkid -o value -s TYPE $p; done; "
                         "for p in /dev/vda?; do [ \"$(blkid -o value -s TYPE $p)\" = crypto_LUKS ] && cryptsetup luksDump $p | head -n 8; done; "
                         "grep -h cryptroot /etc/w3bs-os/* 2>/dev/null; true", 60)
            print(o)
            if "crypto_LUKS" not in o or not re.search(r"Version:\s+2", o):
                raise SystemExit("no LUKS2 volume found")
        log(f"[{vm.name}] PASS")
    finally:
        vm.stop()


def cmd_inspect(a):
    vm = VM("inspect", "uefi", iso=a.iso, disks=[a.disk], outdir=a.out)
    try:
        serial_login(vm, a.timeout)
        script = r"""
mkdir -p /mnt/t
root=$(for p in /dev/vda?; do [ "$(blkid -o value -s TYPE $p)" = ext4 ] && echo $p; done | tail -n1)
mount $root /mnt/t
cat /mnt/t/etc/fstab
awk '$2 ~ /^\/boot/ { print $1, $2, $3 }' /mnt/t/etc/fstab | sort -k2 | while read -r spec mp t; do mount -t $t $(blkid -U ${spec#UUID=}) /mnt/t$mp; done
find /mnt/t/boot -maxdepth 4 -iname '*.efi'
for m in dev proc sys; do mount --bind /$m /mnt/t/$m; done
echo ALPINE=$(cat /mnt/t/etc/alpine-release)
cat /mnt/t/etc/w3bs-os/release /mnt/t/etc/w3bs-os/install-profile
echo USERLINE=$(grep '^w3bs:' /mnt/t/etc/passwd)
echo ROOTSHADOW=$(cut -d: -f1,2 /mnt/t/etc/shadow | grep '^root:' | cut -c1-6)
grep -qs 'PermitRootLogin=yes' /mnt/t/etc/conf.d/sshd && echo LIVE_SSH_LEAK || echo NO_LIVE_SSH
[ -e /mnt/t/etc/local.d/w3bs-live-ssh.start ] && echo LIVE_SSH_LEAK || echo NO_LIVE_SSH_SCRIPT
echo GROUPS=$(chroot /mnt/t id -nG w3bs)
echo VERSION=$(chroot /mnt/t /usr/local/bin/w3bs --version)
echo DESKTOPV=$(chroot /mnt/t /usr/local/bin/w3bs-desktop --version)
echo ZSH=$(chroot /mnt/t su -s /bin/sh w3bs -c 'echo $SHELL'; chroot /mnt/t /bin/zsh -c 'echo $ZSH_VERSION')
echo DM=$(ls /mnt/t/etc/runlevels/default/)
ls /mnt/t/etc/xdg/autostart/ | grep w3bs
grep -h Exec= /mnt/t/etc/xdg/autostart/w3bs-desktop*.desktop
ls /mnt/t/boot /mnt/t/boot/EFI/* 2>/dev/null
[ -e /mnt/t/etc/profile.d/w3bs-os-live.sh ] && echo LIVE_LEFTOVER || echo NO_LIVE_LEFTOVER
grep -c tty1 /mnt/t/etc/inittab; grep 'login -f' /mnt/t/etc/inittab || echo NO_AUTOLOGIN
for m in dev proc sys boot/efi boot ""; do umount /mnt/t/$m 2>/dev/null; done; true
"""
        local = os.path.join(a.out, "inspect.sh")
        with open(local, "w") as f:
            f.write(script)
        push_file(vm, local, "/tmp/inspect.sh")
        rc, out = vm.sh("sh /tmp/inspect.sh", 300)
        print(out)
        if rc != 0:
            raise SystemExit("inspection failed")
        for want in ("/bin/zsh", "w3bs ", "NO_LIVE_LEFTOVER", "NO_AUTOLOGIN", "NO_LIVE_SSH", "NO_LIVE_SSH_SCRIPT", "wheel"):
            if want not in out:
                raise SystemExit(f"inspection: missing {want!r}")
        # Root is locked ("!" or "*"): never a password hash, so the live one-time SSH password
        # (or any other) cannot have leaked into the installed system.
        if not re.search(r"ROOTSHADOW=root:[!*](\s|$)", out):
            raise SystemExit("inspection: root is not locked on the installed system")
        log("[inspect] PASS")
    finally:
        vm.stop()


def cmd_installed(a):
    """Boot the installed disk (UEFI), log in on ttyS0 as the user and assert the system,
    screenshot the greeter and, with --type-password, the desktop after logging in there."""
    user, pw = a.login.split(":", 1)
    vm = VM(f"installed-{a.firmware}", a.firmware, disks=[a.disk], mem=a.mem, outdir=a.out, vga=a.vga)
    try:
        t0 = time.time()
        vm.expect(r"login: ", a.timeout)
        log(f"[installed] serial login prompt after {time.time()-t0:.0f}s")
        time.sleep(3)
        vm.send(user + "\n")
        vm.expect(r"[Pp]assword: ", 60)
        vm.send(pw + "\n")
        vm.expect(r"[%$#>] ", 180)
        time.sleep(3)
        vm.send("PS1='> '; PROMPT='> '; RPROMPT=''\n")
        time.sleep(2)
        checks = [
            ("shell", "echo SHELL=$SHELL ZSH=$ZSH_VERSION; grep \"^$(id -un):\" /etc/passwd", r":/bin/zsh"),
            ("w3bs", "w3bs --version", r"\d+\.\d+\.\d+"),
            ("w3bs-desktop", "w3bs-desktop --version", r"\d+\.\d+\.\d+"),
            ("groups", "id -nG", r"\bwheel\b"),
            ("os-release", "cat /etc/w3bs-os-release", r"ID=w3bs-os[\s\S]*EDITION=iso[\s\S]*ALPINE=v3\.\d+"),
        ]
        if a.desktop != "none":
            dm = {"plasma": "sddm", "gnome": "gdm", "xfce": "lightdm", "hyprland": "greetd"}[a.desktop]
            checks.append(("display-manager",
                           # busybox pgrep -x misses daemons started by full path; pidof matches the name.
                           f"for i in $(seq 1 60); do pidof {dm} >/dev/null && break; sleep 5; done; "
                           f"p=$(pidof {dm}) && echo \"$p {dm}\" || echo NO_DM",
                           rf"\d+ \S*{dm}"))
        failed = []
        for name, cmd, want in checks:
            rc, out = vm.sh(cmd, 400)
            ok = rc == 0 and re.search(want, out) is not None
            log(f"[installed] {name}: {'PASS' if ok else 'FAIL'}: {out.strip()[-300:]}")
            if not ok:
                failed.append(name)
        rc, out = vm.sh("cat /etc/w3bs-os/release; ls /etc/xdg/autostart | grep w3bs; w3bs repo list 2>&1 | head -n 5", 120)
        print(out)
        time.sleep(5)
        vm.screenshot(os.path.join(a.out, "installed-greeter.png"))
        if a.desktop != "none" and a.type_password:
            if a.desktop == "hyprland":
                # tuigreet asks for the user name first, then the password.
                vm.sendkeys(user + "\n")
                time.sleep(2)
            vm.sendkeys(pw + "\n")
            log(f"[installed] typed the password at the greeter; waiting {a.desktop_wait}s for the desktop")
            time.sleep(a.desktop_wait)
            vm.screenshot(os.path.join(a.out, "installed-desktop.png"))
            session = {
                "hyprland": "echo HYPR_UP=$(pidof Hyprland)",
            }.get(a.desktop, "pidof plasmashell || pgrep -a kwin_wayland || pgrep -a -f startplasma || echo NO_PLASMA")
            rc, out = vm.sh(session, 60)
            log(f"[installed] desktop session: {out.strip()[-300:]}")
            # The console echoes the command, so match the pid, not a marker word.
            if a.desktop == "hyprland" and not re.search(r"HYPR_UP=\d", out):
                failed.append("hyprland-session")
            # The Agentic Desktop Layer starts with the session: broker and tray, agent access off.
            rc, out = vm.sh("for i in $(seq 1 30); do [ $(pidof w3bs-desktop | wc -w) -ge 2 ] && break; sleep 2; done; echo ADL_PROCS=$(pidof w3bs-desktop | wc -w); ps -o args | grep '[w]3bs-desktop '; tail -n 5 ~/.local/state/w3bs-os/desktop-layer.log 2>/dev/null", 90)
            log(f"[installed] desktop layer: {out.strip()[-300:]}")
            if not re.search(r"ADL_PROCS=([2-9]|\d\d)", out):
                failed.append("desktop-layer-running")
        if failed:
            raise SystemExit(f"installed system checks failed: {', '.join(failed)}")
        log("[installed] PASS")
    finally:
        vm.stop()


def main():
    p = argparse.ArgumentParser()
    p.add_argument("cmd", choices=["boot", "install", "inspect", "installed"])
    p.add_argument("--iso")
    p.add_argument("--disk")
    p.add_argument("--firmware", default="uefi", choices=["uefi", "bios"])
    p.add_argument("--encrypt", action="store_true")
    p.add_argument("--desktop", default="hyprland")
    p.add_argument("--mem", type=int, default=6144)
    p.add_argument("--timeout", type=int, default=900)
    p.add_argument("--install-timeout", type=int, default=6 * 3600)
    p.add_argument("--desktop-wait", type=int, default=300)
    p.add_argument("--login", default="w3bs:w3bs-test-pass")
    p.add_argument("--type-password", action="store_true")
    p.add_argument("--push-installer")
    p.add_argument("--push", action="append", default=[], metavar="SRC=DEST",
                   help="also copy SRC into the live system at DEST before installing (repeatable)")
    p.add_argument("--auto-disk", action="store_true", help="let the installer pick the largest disk")
    p.add_argument("--vga-review", action="store_true", help="first walk the interactive installer on tty1 to its review and cancel")
    # vmware = the VMware SVGA II adapter with no 3D: what VirtualBox's default VMSVGA looks like to Linux.
    p.add_argument("--vga", default="std", help="QEMU -vga for the installed boot (std, vmware, virtio)")
    p.add_argument("--out", default=".")
    a = p.parse_args()
    os.makedirs(a.out, exist_ok=True)
    {"boot": cmd_boot, "install": cmd_install, "inspect": cmd_inspect, "installed": cmd_installed}[a.cmd](a)


if __name__ == "__main__":
    main()
