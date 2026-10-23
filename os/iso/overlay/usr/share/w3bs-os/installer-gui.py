#!/usr/bin/env python3
"""W3BS OS graphical installer (GTK 4), run full screen under cage by w3bs-os-install-gui.

A front end only: every choice becomes a w3bs-os-install flag, and that one installer does
the work, so the text and graphical installs are the same install. Nothing is written to a
disk until the review page's "Erase and install" is pressed.

Result for the launcher, in $W3BS_GUI_RESULT: "installed", "text" (use the text installer)
or nothing (closed). Test hooks: W3BS_GUI_AUTOWALK=SECONDS clicks through with defaults,
W3BS_GUI_TEST_PASSWORD fills the passwords, W3BS_GUI_TEST_NOENCRYPT=1 turns encryption off.
"""
import os, re, subprocess, sys, tempfile, threading

import gi
gi.require_version("Gtk", "4.0")
from gi.repository import Gtk, GLib, Gdk, Gio  # noqa: E402

INSTALLER = os.environ.get("W3BS_INSTALLER", "/usr/local/sbin/w3bs-os-install")
RESULT = os.environ.get("W3BS_GUI_RESULT", "/run/w3bs-gui/result")
VERSION = open("/usr/share/w3bs-os/VERSION").read().strip() if os.path.exists("/usr/share/w3bs-os/VERSION") else ""
AUTOWALK = float(os.environ.get("W3BS_GUI_AUTOWALK", "0") or 0)
TEST_PW = os.environ.get("W3BS_GUI_TEST_PASSWORD", "")

DESKTOPS = [
    ("hyprland", "Hyprland", "Tiling compositor with Waybar, greetd login. The W3BS default."),
    ("plasma", "KDE Plasma", "Full desktop with SDDM login."),
    ("gnome", "GNOME", "Full desktop with GDM login."),
    ("xfce", "Xfce", "Light desktop with LightDM login."),
]

CSS = b"""
window, .page { background: #f6f4ed; color: #1f2a2a; }
.title { font-size: 30px; font-weight: 700; }
.lead { font-size: 15px; color: #4b5656; }
.eyebrow { font-size: 12px; letter-spacing: 2px; color: #d9532c; font-weight: 700; }
.card { background: #ffffff; border: 1px solid #ddd8cb; border-radius: 10px; padding: 16px; }
.card:checked { border-color: #d9532c; }
.card-title { font-size: 17px; font-weight: 700; }
.muted { color: #6b7474; }
.error { color: #b3261e; }
.footer { border-top: 1px solid #ddd8cb; padding: 14px 28px; background: #efece2; }
button.suggested-action { background: #d9532c; color: #ffffff; }
button.destructive-action { background: #b3261e; color: #ffffff; }
textview, textview text { font-family: monospace; font-size: 12px; background: #1b2222; color: #e6e6e6; }
"""


def keymaps():
    base = "/usr/share/bkeymaps"
    out = {}
    try:
        for layout in sorted(os.listdir(base)):
            d = os.path.join(base, layout)
            if os.path.isdir(d):
                out[layout] = sorted(f[:-8] for f in os.listdir(d) if f.endswith(".bmap.gz"))
    except OSError:
        pass
    return out or {"us": ["us"]}


def timezones():
    base = "/usr/share/zoneinfo"
    zones = []
    for root, dirs, files in os.walk(base):
        rel = os.path.relpath(root, base)
        if rel.split(os.sep)[0] in ("posix", "right", "Etc", "SystemV", "US") and rel != ".":
            continue
        for f in files:
            if f[0].isupper():
                z = f if rel == "." else os.path.join(rel, f)
                if "/" in z or z == "UTC":
                    zones.append(z)
    return sorted(set(zones)) or ["UTC"]


def disks():
    try:
        out = subprocess.run([INSTALLER, "--list-disks"], capture_output=True, text=True, timeout=30).stdout
    except Exception:
        out = ""
    rows = []
    for line in out.splitlines():
        parts = line.split("\t")
        if len(parts) >= 2 and parts[1].isdigit():
            rows.append((parts[0], int(parts[1]), parts[2] if len(parts) > 2 else ""))
    return rows


def human(n):
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if n < 1000 or unit == "TB":
            return f"{n:.0f} {unit}" if unit == "B" else f"{n:.1f} {unit}"
        n /= 1000


def label(text, css=None, wrap=True, xalign=0.0):
    w = Gtk.Label(label=text, xalign=xalign, wrap=wrap)
    w.set_wrap_mode(2)  # WORD_CHAR
    if css:
        for c in css.split():
            w.add_css_class(c)
    return w


def dropdown(items, selected, search=False):
    model = Gtk.StringList.new(items)
    dd = Gtk.DropDown(model=model)
    if search:
        dd.set_enable_search(True)
        dd.set_expression(Gtk.PropertyExpression.new(Gtk.StringObject, None, "string"))
    if selected in items:
        dd.set_selected(items.index(selected))
    return dd


def dd_value(dd):
    item = dd.get_selected_item()
    return item.get_string() if item else ""


class Installer(Gtk.ApplicationWindow):
    def __init__(self, app):
        super().__init__(application=app, title="Install W3BS OS")
        self.set_default_size(1024, 720)
        self.set_decorated(False)
        self.kmaps = keymaps()
        self.zones = timezones()
        self.disk_rows = disks()
        self.proc = None

        self.stack = Gtk.Stack(transition_type=Gtk.StackTransitionType.SLIDE_LEFT_RIGHT, vexpand=True)
        self.pages = []
        self.back = Gtk.Button(label="Back")
        self.next = Gtk.Button(label="Continue")
        self.next.add_css_class("suggested-action")
        self.text_btn = Gtk.Button(label="Use the text installer")
        self.back.connect("clicked", lambda *_: self.go(-1))
        self.next.connect("clicked", lambda *_: self.go(+1))
        self.text_btn.connect("clicked", lambda *_: self.finish("text"))
        self.step_label = label("", "muted", wrap=False)

        footer = Gtk.Box(spacing=12)
        footer.add_css_class("footer")
        footer.append(self.text_btn)
        footer.append(Gtk.Box(hexpand=True))
        footer.append(self.step_label)
        footer.append(self.back)
        footer.append(self.next)

        root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        root.append(self.stack)
        root.append(footer)
        self.set_child(root)

        for build in (self.page_type, self.page_desktop, self.page_locale, self.page_user,
                      self.page_disk, self.page_review, self.page_progress, self.page_done):
            name, widget = build()
            self.stack.add_named(widget, name)
            self.pages.append(name)
        self.index = 0
        self.show_page()
        if AUTOWALK:
            GLib.timeout_add(int(AUTOWALK * 1000), self.autowalk)

    # --- layout helpers ---------------------------------------------------------------
    def page(self, eyebrow, title, lead):
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=14, margin_top=36, margin_bottom=24,
                      margin_start=56, margin_end=56)
        box.add_css_class("page")
        box.append(label(eyebrow, "eyebrow"))
        box.append(label(title, "title"))
        if lead:
            box.append(label(lead, "lead"))
        scroller = Gtk.ScrolledWindow(vexpand=True, hscrollbar_policy=Gtk.PolicyType.NEVER)
        scroller.set_child(box)
        return scroller, box

    def card(self, group, title, text, active=False):
        btn = Gtk.ToggleButton()
        btn.add_css_class("card")
        inner = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
        inner.append(label(title, "card-title"))
        inner.append(label(text, "muted"))
        btn.set_child(inner)
        if group is not None:
            btn.set_group(group)
        btn.set_active(active)
        return btn

    def form(self, rows):
        grid = Gtk.Grid(row_spacing=12, column_spacing=18, margin_top=8)
        for i, (name, widget) in enumerate(rows):
            grid.attach(label(name, xalign=1.0, wrap=False), 0, i, 1, 1)
            widget.set_hexpand(True)
            grid.attach(widget, 1, i, 1, 1)
        return grid

    # --- pages ------------------------------------------------------------------------
    def page_type(self):
        sc, box = self.page(f"W3BS OS {VERSION} · GRAPHICAL INSTALLER", "What are you installing?",
                            "A desktop gives you a graphical session to log in to. A server is command "
                            "line only (zsh, w3bs, sshd, Caddy). Both get the w3bs CLI and an encrypted disk.")
        self.t_desktop = self.card(None, "Desktop", "Graphical desktop with Firefox and TronBrowser. Choose Hyprland, KDE Plasma, GNOME or Xfce next.", True)
        self.t_server = self.card(self.t_desktop, "Server", "Command line only, no desktop. For a VPS or a headless box.")
        row = Gtk.Box(spacing=16, homogeneous=True)
        row.append(self.t_desktop)
        row.append(self.t_server)
        box.append(row)
        box.append(label("Packages download during the install, so a wired network connection is needed.", "muted"))
        return "type", sc

    def page_desktop(self):
        sc, box = self.page("DESKTOP", "Choose a desktop.", "You can install another one later.")
        self.desktop_btns = {}
        first = None
        grid = Gtk.Grid(row_spacing=16, column_spacing=16, column_homogeneous=True)
        for i, (key, title, text) in enumerate(DESKTOPS):
            b = self.card(first, title + ("  (default)" if key == "hyprland" else ""), text, key == "hyprland")
            first = first or b
            self.desktop_btns[key] = b
            grid.attach(b, i % 2, i // 2, 1, 1)
        box.append(grid)
        return "desktop", sc

    def page_locale(self):
        sc, box = self.page("REGION", "Keyboard, time zone and name.", None)
        layouts = sorted(self.kmaps)
        self.kb_layout = dropdown(layouts, "us", search=True)
        self.kb_variant = dropdown(self.kmaps.get("us", ["us"]), "us")
        self.kb_layout.connect("notify::selected", self.on_layout)
        self.tz = dropdown(self.zones, "UTC", search=True)
        self.hostname = Gtk.Entry(text="w3bs")
        self.domain = Gtk.Entry(placeholder_text="optional, e.g. dev1.example.com: its web page gets HTTPS")
        self.test_entry = Gtk.Entry(placeholder_text="Type here to test the keyboard (after install the layout above is used)")
        box.append(self.form([("Keyboard layout", self.kb_layout), ("Variant", self.kb_variant),
                              ("Time zone", self.tz), ("Computer name", self.hostname), ("Web domain", self.domain), ("", self.test_entry)]))
        return "locale", sc

    def on_layout(self, *_):
        variants = self.kmaps.get(dd_value(self.kb_layout), [])
        self.kb_variant.set_model(Gtk.StringList.new(variants))
        lay = dd_value(self.kb_layout)
        self.kb_variant.set_selected(variants.index(lay) if lay in variants else 0)

    def page_user(self):
        sc, box = self.page("YOUR ACCOUNT", "Create your user.",
                            "You log in with this account; it is an administrator (doas). Root login stays locked.")
        self.fullname = Gtk.Entry(text="W3BS User")
        self.username = Gtk.Entry(text="w3bs")
        self.pw1 = Gtk.PasswordEntry(show_peek_icon=True)
        self.pw2 = Gtk.PasswordEntry(show_peek_icon=True)
        self.rpw1 = Gtk.PasswordEntry(show_peek_icon=True)
        self.rpw2 = Gtk.PasswordEntry(show_peek_icon=True)
        box.append(label("Root password (optional): for su and root logins on the console. Leave it empty "
                         "to keep root locked; your account below is an administrator either way (sudo).", "muted"))
        box.append(self.form([("Root password", self.rpw1), ("Root password again", self.rpw2)]))
        box.append(self.form([("Full name", self.fullname), ("User name", self.username),
                              ("Password", self.pw1), ("Password again", self.pw2)]))
        self.user_err = label("", "error")
        box.append(self.user_err)
        return "user", sc

    def page_disk(self):
        sc, box = self.page("DISK", "Where should W3BS OS go?",
                            "The disk you choose is erased completely. The installer media is not listed.")
        self.disk_btns = []
        first = None
        if not self.disk_rows:
            box.append(label("No internal disk was found. Add a disk of at least 8 GB (20 GB for a desktop).", "error"))
        biggest = max(self.disk_rows, key=lambda r: r[1])[0] if self.disk_rows else None
        for dev, size, model in self.disk_rows:
            b = self.card(first, f"{dev}   {human(size)}", model or "disk", dev == biggest)
            first = first or b
            self.disk_btns.append((dev, size, b))
            box.append(b)
        box.append(label("Disk encryption", "card-title"))
        # self.encrypt is the "Encrypt" card: get_active() is the choice.
        self.encrypt = self.card(None, "Encrypt the disk  (recommended)",
                                 "LUKS2. A passphrase is asked at every boot; if you lose it, the data cannot be recovered.", True)
        self.no_encrypt = self.card(self.encrypt, "Do not encrypt",
                                    "Boots straight to the login screen. For VMs, test machines and servers with a web console.")
        row = Gtk.Box(spacing=16, homogeneous=True)
        row.append(self.encrypt)
        row.append(self.no_encrypt)
        box.append(row)
        self.pp1 = Gtk.PasswordEntry(show_peek_icon=True)
        self.pp2 = Gtk.PasswordEntry(show_peek_icon=True)
        self.pp_form = self.form([("Disk passphrase", self.pp1), ("Passphrase again", self.pp2)])
        self.encrypt.connect("toggled", lambda *_: self.pp_form.set_visible(self.encrypt.get_active()))
        box.append(self.pp_form)
        self.disk_err = label("", "error")
        box.append(self.disk_err)
        return "disk", sc

    def page_review(self):
        sc, box = self.page("REVIEW", "Nothing has been written yet.", None)
        self.review = Gtk.Grid(row_spacing=8, column_spacing=24, margin_top=4)
        box.append(self.review)
        self.confirm = Gtk.CheckButton(label="")
        self.confirm.connect("toggled", lambda *_: self.update_buttons())
        box.append(self.confirm)
        return "review", sc

    def page_progress(self):
        sc, box = self.page("INSTALLING", "Installing W3BS OS…", "This takes a few minutes; the desktop downloads about 1 GB.")
        self.bar = Gtk.ProgressBar(show_text=True)
        self.step_now = label("Starting", "card-title")
        self.logbuf = Gtk.TextBuffer()
        view = Gtk.TextView(buffer=self.logbuf, editable=False, cursor_visible=False, monospace=True)
        self.logscroll = Gtk.ScrolledWindow(min_content_height=320, vexpand=True)
        self.logscroll.set_child(view)
        box.append(self.step_now)
        box.append(self.bar)
        box.append(self.logscroll)
        return "progress", sc

    def page_done(self):
        sc, box = self.page("DONE", "", None)
        self.done_title = box.get_first_child().get_next_sibling()
        self.done_text = label("", "lead")
        box.append(self.done_text)
        return "done", sc

    # --- navigation -------------------------------------------------------------------
    def desktop(self):
        if self.t_server.get_active():
            return "none"
        return next((k for k, b in self.desktop_btns.items() if b.get_active()), "hyprland")

    def disk(self):
        return next(((d, s) for d, s, b in self.disk_btns if b.get_active()), (None, 0))

    def show_page(self):
        self.stack.set_visible_child_name(self.pages[self.index])
        name = self.pages[self.index]
        if name == "review":
            self.fill_review()
        self.update_buttons()

    def update_buttons(self):
        name = self.pages[self.index]
        steps = [p for p in self.pages if p not in ("progress", "done")]
        self.step_label.set_text(f"Step {steps.index(name) + 1} of {len(steps)}" if name in steps else "")
        self.back.set_visible(name not in ("type", "progress", "done"))
        self.text_btn.set_visible(name not in ("progress", "done"))
        self.next.set_visible(name != "progress")
        if name == "review":
            self.next.set_label("Erase and install")
            self.next.remove_css_class("suggested-action")
            self.next.add_css_class("destructive-action")
            self.next.set_sensitive(self.confirm.get_active())
        elif name == "done":
            self.next.set_label("Reboot" if self.ok else "Back to the review")
            self.next.set_sensitive(True)
        else:
            self.next.set_label("Continue")
            self.next.remove_css_class("destructive-action")
            self.next.add_css_class("suggested-action")
            self.next.set_sensitive(True)

    def validate(self, name):
        if name == "user":
            user = self.username.get_text().strip()
            err = ""
            if not re.fullmatch(r"[a-z_][a-z0-9_-]{0,30}", user) or user == "root":
                err = "The user name must start with a letter and use only a-z, 0-9, _ and -."
            elif ":" in self.fullname.get_text():
                err = "The full name cannot contain ':'."
            elif len(self.pw1.get_text()) < 6:
                err = "Use a password of at least 6 characters."
            elif self.pw1.get_text() != self.pw2.get_text():
                err = "The two passwords are different."
            elif self.rpw1.get_text() and len(self.rpw1.get_text()) < 6:
                err = "Use a root password of at least 6 characters, or leave it empty."
            elif self.rpw1.get_text() != self.rpw2.get_text():
                err = "The two root passwords are different."
            self.user_err.set_text(err)
            return not err
        if name == "disk":
            dev, size = self.disk()
            need = 20e9 if self.desktop() != "none" else 8e9
            err = ""
            if not dev:
                err = "Choose a disk."
            elif size < need:
                err = f"{dev} is {human(size)}; this install needs at least {human(need)}."
            elif self.encrypt.get_active() and len(self.pp1.get_text()) < 8:
                err = "Use a disk passphrase of at least 8 characters."
            elif self.encrypt.get_active() and self.pp1.get_text() != self.pp2.get_text():
                err = "The two passphrases are different."
            self.disk_err.set_text(err)
            return not err
        if name == "locale":
            d = self.domain.get_text().strip()
            return bool(re.fullmatch(r"[A-Za-z0-9-]{1,63}", self.hostname.get_text().strip())) and \
                (not d or bool(re.fullmatch(r"[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+", d)))
        return True

    def go(self, delta):
        name = self.pages[self.index]
        if name == "done":
            if self.ok:
                subprocess.run(["sync"])
                self.finish("installed", reboot=True)
            else:
                self.index = self.pages.index("review")
                self.show_page()
            return
        if delta > 0 and not self.validate(name):
            return
        if name == "review" and delta > 0:
            self.start_install()
            return
        self.index += delta
        if self.pages[self.index] == "desktop" and self.t_server.get_active():
            self.index += delta
        self.show_page()

    def fill_review(self):
        dev, size = self.disk()
        model = next((m for d, s, m in self.disk_rows if d == dev), "")
        desk = self.desktop()
        dname = dict((k, t) for k, t, _ in DESKTOPS).get(desk, "none (server install, command line only)")
        lines = [
            f"Disk:  {dev}  {human(size)}  {model}  (ALL DATA ON IT WILL BE ERASED)",
            f"Encryption:  {'LUKS2, passphrase asked at boot' if self.encrypt.get_active() else 'off'}",
            f"Desktop:  {dname}",
            f"Browsers:  {'Firefox and TronBrowser' if desk != 'none' else 'none'}",
            f"Keyboard:  {dd_value(self.kb_layout)} {dd_value(self.kb_variant)}",
            f"Time zone:  {dd_value(self.tz)}",
            f"Computer name:  {self.hostname.get_text().strip()}",
            f"Web page:  {'https://' + self.domain.get_text().strip() if self.domain.get_text().strip() else 'http://' + self.hostname.get_text().strip() + ' (no domain: no HTTPS)'}",
            f"User:  {self.username.get_text().strip()} ({self.fullname.get_text().strip()}), administrator via doas",
            f"Root:  {'password set (su, console login)' if self.rpw1.get_text() else 'locked (administrators use doas)'}",
            "W3BS:  w3bs CLI and the Agentic Desktop Layer (agent access OFF until you turn it on)",
            "Dev tools:  curl, wget, mise (bun, pnpm), Node and readm3",
        ]
        while (child := self.review.get_first_child()) is not None:
            self.review.remove(child)
        for i, line in enumerate(lines):
            key, val = line.split(":  ", 1)
            k = label(key, "card-title", wrap=False)
            v = label(val, "error" if i == 0 else None)
            v.set_selectable(True)
            self.review.attach(k, 0, i, 1, 1)
            self.review.attach(v, 1, i, 1, 1)
        self.confirm.set_label(f"I understand that everything on {dev} will be erased.")
        self.confirm.set_active(False)

    # --- install ----------------------------------------------------------------------
    def start_install(self):
        dev, _ = self.disk()
        work = tempfile.mkdtemp(prefix="w3bs-gui-", dir="/run" if os.access("/run", os.W_OK) else None)
        os.chmod(work, 0o700)
        pw = os.path.join(work, "pw")
        with open(os.open(pw, os.O_WRONLY | os.O_CREAT, 0o600), "w") as f:
            f.write(self.pw1.get_text() + "\n")
        args = [INSTALLER, "--defaults", "--yes", "--no-reboot", "--disk", dev, "--desktop", self.desktop(),
                "--hostname", self.hostname.get_text().strip(), "--timezone", dd_value(self.tz),
                *(["--domain", self.domain.get_text().strip()] if self.domain.get_text().strip() else []),
                "--keymap", f"{dd_value(self.kb_layout)} {dd_value(self.kb_variant)}",
                "--user", self.username.get_text().strip(), "--fullname", self.fullname.get_text().strip(),
                "--password-file", pw]
        if self.rpw1.get_text():
            rp = os.path.join(work, "root")
            with open(os.open(rp, os.O_WRONLY | os.O_CREAT, 0o600), "w") as f:
                f.write(self.rpw1.get_text() + "\n")
            args += ["--root-password-file", rp]
        if self.encrypt.get_active():
            pp = os.path.join(work, "luks")
            with open(os.open(pp, os.O_WRONLY | os.O_CREAT, 0o600), "w") as f:
                f.write(self.pp1.get_text() + "\n")
            args += ["--passphrase-file", pp]
        else:
            args += ["--no-encrypt"]
        self.work = work
        self.index = self.pages.index("progress")
        self.show_page()
        self.steps_seen = 0
        self.total_steps = self.count_steps()
        self.proc = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                     stderr=subprocess.STDOUT, text=True, bufsize=1)
        threading.Thread(target=self.reader, daemon=True).start()
        GLib.timeout_add(400, self.pulse)

    def count_steps(self):
        try:
            return max(8, len(re.findall(r'^\s*step "', open(INSTALLER).read(), re.M)) - 2)
        except OSError:
            return 14

    def reader(self):
        for line in self.proc.stdout:
            GLib.idle_add(self.on_line, line.rstrip("\n"))
        rc = self.proc.wait()
        GLib.idle_add(self.on_exit, rc)

    def on_line(self, line):
        # apk and setup-alpine draw progress bars: escape codes, cursor save/restore, \r redraws.
        line = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b[78()][0-9A-B]?", "", line).split("\r")[-1]
        if not line.strip() or re.fullmatch(r"[\s\d%]+", line):
            return False
        end = self.logbuf.get_end_iter()
        self.logbuf.insert(end, line + "\n")
        adj = self.logscroll.get_vadjustment()
        adj.set_value(adj.get_upper())
        if line.startswith("==> "):
            self.steps_seen += 1
            self.step_now.set_text(line[4:])
            self.bar.set_fraction(min(0.97, self.steps_seen / self.total_steps))
            self.bar.set_text(f"{int(self.bar.get_fraction() * 100)}%")
        return False

    def pulse(self):
        return self.proc is not None and self.proc.poll() is None

    def on_exit(self, rc):
        for f in ("pw", "luks", "root"):
            try:
                os.unlink(os.path.join(self.work, f))
            except OSError:
                pass
        self.ok = rc == 0
        self.index = self.pages.index("done")
        if self.ok:
            self.done_title.set_text("W3BS OS is installed.")
            dev, _ = self.disk()
            msg = f"It is on {dev}. Reboot: the installer DVD is ejected first so the machine starts W3BS OS (take out a USB stick yourself)."
            if self.encrypt.get_active():
                msg += " You will be asked for the disk passphrase at boot."
            msg += f" Log in as {self.username.get_text().strip()}."
            self.done_text.set_text(msg)
        else:
            self.done_title.set_text("The install failed.")
            tail = self.logbuf.get_text(self.logbuf.get_start_iter(), self.logbuf.get_end_iter(), False).splitlines()[-8:]
            self.done_text.set_text("Nothing was reported as installed. Last lines of the log "
                                    "(full log: /var/log/w3bs-os-install.log):\n\n" + "\n".join(tail))
        self.show_page()
        # After minutes of log updates the software renderer can leave this page blank in
        # a VM (VirtualBox): force a full redraw once it is shown.
        GLib.timeout_add(300, lambda: (self.queue_draw(), False)[1])
        if AUTOWALK and self.ok:
            print("W3BS_GUI_INSTALLED", flush=True)
        return False

    def finish(self, result, reboot=False):
        try:
            os.makedirs(os.path.dirname(RESULT), exist_ok=True)
            with open(RESULT, "w") as f:
                f.write(result + ("\nreboot" if reboot else "") + "\n")
        except OSError:
            pass
        self.get_application().quit()

    # --- test hook --------------------------------------------------------------------
    def autowalk(self):
        name = self.pages[self.index]
        if name == "user" and TEST_PW:
            for w in (self.pw1, self.pw2):
                w.set_text(TEST_PW)
        if name == "disk":
            if os.environ.get("W3BS_GUI_TEST_NOENCRYPT"):
                self.no_encrypt.set_active(True)
            elif TEST_PW:
                for w in (self.pp1, self.pp2):
                    w.set_text(TEST_PW + "-luks")
        if name == "review":
            self.confirm.set_active(True)
        if name in ("progress", "done"):
            return name == "progress"
        print(f"W3BS_GUI_PAGE {name}", flush=True)
        self.go(+1)
        return True


class App(Gtk.Application):
    def __init__(self):
        super().__init__(application_id="org.w3bs.OsInstaller", flags=Gio.ApplicationFlags.NON_UNIQUE)

    def do_activate(self):
        provider = Gtk.CssProvider()
        provider.load_from_data(CSS)
        Gtk.StyleContext.add_provider_for_display(Gdk.Display.get_default(), provider,
                                                  Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
        win = Installer(self)
        win.ok = False
        win.present()
        win.fullscreen()


if __name__ == "__main__":
    sys.exit(App().run([sys.argv[0]]))
