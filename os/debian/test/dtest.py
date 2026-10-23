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
p.add_argument("--out", default="out"); p.add_argument("--timeout", type=int, default=3000)
a = p.parse_args()
os.makedirs(a.out, exist_ok=True)
fails = []

def check(name, ok, detail=""):
    log(f"{name}: {'PASS' if ok else 'FAIL'} {detail.strip()[-200:]}")
    if not ok:
        fails.append(name)

vm = VM(f"live-{a.firmware}", a.firmware, iso=a.iso, disks=[a.disk], mem=4096, outdir=a.out)
try:
    vm.expect(r"root@w3bs-os:~# |# ", 600); time.sleep(20)
    rc, o = vm.sh("cat /etc/w3bs-os-version; curl -s -o /dev/null -w 'MIRROR=%{http_code}\\n' http://deb.debian.org/debian/dists/trixie/Release; "
                  "systemctl is-active systemd-networkd ssh; cat /run/w3bs-live-ssh-password", 120)
    check("live network + DNS", "MIRROR=200" in o, o)
    check("live ssh one-time password", re.search(r"\b[a-z0-9]{4}-[a-z0-9]{4}-[a-z0-9]{4}\b", o) is not None, o)
    vm.screenshot(os.path.join(a.out, f"live-{a.firmware}.png"))
    vm.sh("printf 'w3bs-test-pass\\n' > /tmp/pw; printf 'w3bs-luks-pass\\n' > /tmp/lk", 30)
    flags = "--passphrase-file /tmp/lk" if a.encrypt else "--no-encrypt"
    rc, o = vm.sh(f"w3bs-os-install --defaults --disk /dev/vda --yes --no-reboot --serial-console --password-file /tmp/pw "
                  f"--domain ci.example.test {flags} > /tmp/o 2>&1; echo RC=$?; grep -E 'warning|FAILED|installed on' /tmp/o", a.timeout)
    # The console echoes the command (which itself says 'warning'): count the installer's lines only.
    check("install", re.search(r"RC=0\b", o) is not None and re.search(r"warning: ", o) is None, o)
    if "RC=0" not in o:
        _, t = vm.sh("tail -40 /var/log/w3bs-os-install.log", 60); print(t)
finally:
    vm.stop()

if "install" not in fails:
    vm = VM(f"installed-{a.firmware}", a.firmware, disks=[a.disk], mem=4096, outdir=a.out)
    try:
        if a.encrypt:
            vm.expect(r"[Pp]assphrase|unlock disk", 300); time.sleep(2); vm.send("w3bs-luks-pass\n")
        vm.expect(r"\S+ login: ", 600); time.sleep(3)
        vm.send("w3bs\n"); vm.expect(r"[Pp]assword: ", 60); vm.send("w3bs-test-pass\n")
        time.sleep(15); vm.send("PS1='> '; PROMPT='> '\n"); time.sleep(2)
        rc, o = vm.sh("grep -E '^(ID|VERSION)=' /etc/w3bs-os-release; echo SHELL=$SHELL; "
                      "curl -s -o /dev/null -w 'NET=%{http_code}\\n' http://deb.debian.org/debian/dists/trixie/Release; "
                      "echo w3bs-test-pass | sudo -S -p '' id -u; "
                      "for c in w3bs mise node bun pnpm readm3; do command -v $c >/dev/null && echo HAVE_$c; done; "
                      "systemctl --failed --no-legend --no-pager | grep -c 'user@' | sed 's/^/USER_UNITS_FAILED=/'", 300)
        check("os-release", "ID=w3bs-os" in o, o)
        check("zsh", "SHELL=/usr/bin/zsh" in o, o)
        check("network", "NET=200" in o, o)
        check("sudo", re.search(r"^0\s*$", o, re.M) is not None, o)
        for c in ("w3bs", "mise", "node", "bun", "pnpm", "readm3"):
            check(f"tool {c}", f"HAVE_{c}" in o, "")
        check("user session units", "USER_UNITS_FAILED=0" in o, o)
    finally:
        vm.stop()
if fails:
    sys.exit("failed: " + ", ".join(fails))
log("PASS")
