//! GNOME Shell window observer: window list, geometry, stacking, focus, activation and the
//! system-dialog interlock, from the W3BS GNOME Shell extension (`gnome-extension/` in this
//! workspace, installed with `w3bs desktop setup gnome-extension`).
//!
//! Mutter has no public D-Bus interface that lists windows, so the extension exports one from
//! inside gnome-shell: name `org.w3bs.Desktop.Shell`, path `/org/w3bs/Desktop/Shell`, interface
//! `org.w3bs.Desktop.Shell`, methods `GetWindows() -> s` (JSON), `GetFocused() -> s`,
//! `GetInterlock() -> s` and `Activate(s) -> b`. It is read-only apart from activation and never
//! injects input.
//!
//! Trust runs both ways without a shared secret:
//! - the extension answers only a caller whose bus connection has this user's uid AND the pid of
//!   the process at the other end of its control.sock subscription (this broker, by SO_PEERCRED);
//! - this side calls the extension only when `org.w3bs.Desktop.Shell` is owned by the same
//!   process that owns `org.gnome.Shell`, and addresses that unique name directly, so a process
//!   squatting on the well-known name gets no query and cannot feed us window data.
//!
//! Window ids are `Meta.Window.get_id()` as a decimal string: unique for the life of the shell
//! and never reused for another window. The app id is the desktop-file id GNOME Shell's window
//! tracker associates with the window (".desktop" stripped), else the GTK application id, the
//! Flatpak id, or the Wayland app id / WM_CLASS. `windows()` lists normal windows, dialogs and
//! utility windows; `window_at()` walks every visible window actor top to bottom (menus,
//! popups and tooltips included) plus the top bar, because any of them can cover a target.

use adl_broker::{Adapter, InputEvent, SessionEvent};
use adl_schema::*;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const SHELL_NAME: &str = "org.w3bs.Desktop.Shell";
pub const SHELL_PATH: &str = "/org/w3bs/Desktop/Shell";
pub const SHELL_IFACE: &str = "org.w3bs.Desktop.Shell";
pub const EXTENSION_UUID: &str = "agentic-desktop@w3bs.org";
const GNOME_SHELL: &str = "org.gnome.Shell";
/// Snapshots older than this are re-queried. Focus and interlocks always re-query.
const STATE_TTL: Duration = Duration::from_millis(100);
const CALL_TIMEOUT: Duration = Duration::from_secs(2);
/// Snapshot format this observer understands.
const SNAPSHOT_VERSION: u32 = 1;
/// Window types a person works in. Everything else (menus, tooltips, the top bar, docks,
/// notifications, the desktop) only matters as something that can cover a target.
const USER_TYPES: &[&str] = &["normal", "dialog", "modal_dialog", "utility"];

pub const INSTALL_HINT: &str = "install the W3BS GNOME Shell extension (`w3bs desktop setup gnome-extension`, then log out and back in)";

fn yes() -> bool {
    true
}

/// One window (or shell chrome surface) as the extension reports it.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ShellWindow {
    pub id: String,
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub wm_class: String,
    #[serde(default)]
    pub title: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub minimized: bool,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default = "yes")]
    pub on_current_workspace: bool,
    #[serde(default)]
    pub monitor: i32,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub focused: bool,
}

impl ShellWindow {
    pub fn app_id(&self) -> &str {
        if self.app_id.is_empty() { &self.wm_class } else { &self.app_id }
    }

    pub fn is_user_window(&self) -> bool {
        USER_TYPES.contains(&self.kind.as_str())
    }

    /// Drawn on screen right now (it may still be covered by something above it).
    pub fn is_visible(&self) -> bool {
        !self.minimized && !self.hidden && self.on_current_workspace && self.width > 0.0 && self.height > 0.0
    }

    fn rect(&self) -> Rect {
        Rect { x: self.x, y: self.y, width: self.width, height: self.height }
    }
}

/// One `GetWindows` answer. `windows` is topmost first.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ShellSnapshot {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub focused: Option<String>,
    /// Why input must wait ("" when clear): lock screen, overview, a shell modal dialog or menu.
    #[serde(default)]
    pub interlock: String,
    #[serde(default)]
    pub windows: Vec<ShellWindow>,
}

impl ShellSnapshot {
    pub fn parse(json: &str) -> Result<Self> {
        let s: Self = serde_json::from_str(json).map_err(|e| AdlError::internal(format!("unexpected GNOME Shell extension report: {e}")))?;
        if s.version != SNAPSHOT_VERSION {
            return Err(AdlError::new(ErrorCode::Blocked, format!("GNOME Shell extension speaks snapshot version {}, this broker {SNAPSHOT_VERSION}; reinstall it with `w3bs desktop setup gnome-extension`", s.version)));
        }
        Ok(s)
    }

    fn info(&self, w: &ShellWindow) -> WindowInfo {
        WindowInfo {
            id: w.id.clone(),
            app_id: w.app_id().to_string(),
            title: w.title.clone(),
            // Minimized windows and windows on another workspace have no on-screen box.
            bounds: w.is_visible().then(|| w.rect()),
            display: None,
            focused: self.focused.as_deref() == Some(w.id.as_str()),
        }
    }

    /// User windows, topmost first.
    pub fn windows(&self) -> Vec<WindowInfo> {
        self.windows.iter().filter(|w| w.is_user_window()).map(|w| self.info(w)).collect()
    }

    pub fn focused(&self) -> Option<WindowInfo> {
        let id = self.focused.as_deref()?;
        self.windows.iter().find(|w| w.id == id).map(|w| self.info(w))
    }

    /// The topmost visible surface at a desktop logical point, of any type.
    pub fn window_at(&self, x: f64, y: f64) -> Option<WindowInfo> {
        self.windows.iter().find(|w| w.is_visible() && w.rect().contains(x, y)).map(|w| self.info(w))
    }

    pub fn interlock(&self) -> Option<String> {
        (!self.interlock.is_empty()).then(|| self.interlock.clone())
    }
}

/// Where snapshots come from: the extension over D-Bus, or a canned source in tests.
pub trait ShellSource: Send + Sync {
    fn snapshot(&self) -> Result<ShellSnapshot>;
    /// Raise and focus a window by id. `Ok(false)` when the shell has no such window.
    fn activate(&self, id: &str) -> Result<bool>;
}

fn err(e: impl std::fmt::Display) -> AdlError {
    AdlError::new(ErrorCode::Blocked, format!("GNOME Shell extension: {e}"))
}

/// A GNOME session that is not Vela (Vela lists itself first, e.g. `Vela:GNOME`).
pub fn detect() -> bool {
    is_gnome(&std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default())
}

fn is_gnome(desktop: &str) -> bool {
    desktop.split(':').next() != Some("Vela") && desktop.split(':').any(|c| c.eq_ignore_ascii_case("GNOME"))
}

/// Unique bus name of the extension, if it is loaded inside the running gnome-shell.
fn verified_owner(conn: &zbus::blocking::Connection) -> Result<String> {
    let bus = zbus::blocking::fdo::DBusProxy::new(conn).map_err(err)?;
    let owner = bus.get_name_owner(SHELL_NAME.try_into().map_err(err)?).map_err(|_| err(format!("{SHELL_NAME} is not on the session bus; {INSTALL_HINT}")))?;
    let shell = bus.get_name_owner(GNOME_SHELL.try_into().map_err(err)?).map_err(|_| err("GNOME Shell (org.gnome.Shell) is not on the session bus"))?;
    let pid = |n: &zbus::names::OwnedUniqueName| bus.get_connection_unix_process_id(zbus::names::BusName::Unique(n.as_ref())).map_err(err);
    if owner != shell && pid(&owner)? != pid(&shell)? {
        return Err(err(format!("{SHELL_NAME} is owned by a process other than gnome-shell; refusing its window data")));
    }
    Ok(owner.to_string())
}

/// Whether the extension is loaded in the running gnome-shell (for `doctor`).
pub fn extension_present() -> Result<String> {
    let conn = zbus::blocking::Connection::session().map_err(err)?;
    verified_owner(&conn)
}

/// [`ShellSource`] over the session bus.
pub struct ShellBus {
    conn: zbus::blocking::Connection,
}

impl ShellBus {
    /// Connect to the session bus; fails with an install hint when the extension is not loaded.
    pub fn connect() -> Result<Self> {
        let conn = zbus::blocking::connection::Builder::session().map(|b| b.method_timeout(CALL_TIMEOUT)).and_then(|b| b.build()).map_err(err)?;
        verified_owner(&conn)?;
        Ok(Self { conn })
    }

    fn proxy(&self) -> Result<zbus::blocking::Proxy<'_>> {
        let owner = verified_owner(&self.conn)?;
        zbus::blocking::Proxy::new(&self.conn, owner, SHELL_PATH, SHELL_IFACE).map_err(err)
    }
}

fn call_err(e: zbus::Error) -> AdlError {
    match &e {
        zbus::Error::MethodError(name, msg, _) if name.as_str() == "org.freedesktop.DBus.Error.AccessDenied" => {
            err(format!("refused: {} (the extension answers only the broker connected to it as a native control)", msg.as_deref().unwrap_or("")))
        }
        _ => err(e),
    }
}

impl ShellSource for ShellBus {
    fn snapshot(&self) -> Result<ShellSnapshot> {
        let json: String = self.proxy()?.call("GetWindows", &()).map_err(call_err)?;
        ShellSnapshot::parse(&json)
    }

    fn activate(&self, id: &str) -> Result<bool> {
        self.proxy()?.call("Activate", &(id,)).map_err(call_err)
    }
}

/// Cached window state from a [`ShellSource`].
pub struct ShellObserver {
    source: Box<dyn ShellSource>,
    cache: Mutex<Option<(Instant, ShellSnapshot)>>,
    last_error: Mutex<Option<String>>,
}

impl ShellObserver {
    pub fn new(source: Box<dyn ShellSource>) -> Self {
        Self { source, cache: Mutex::new(None), last_error: Mutex::new(None) }
    }

    pub fn fetch(&self) -> Result<ShellSnapshot> {
        let r = self.source.snapshot();
        *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = r.as_ref().err().map(|e| e.message.clone());
        let s = r?;
        *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), s.clone()));
        Ok(s)
    }

    pub fn snapshot(&self) -> Result<ShellSnapshot> {
        if let Some((t, s)) = &*self.cache.lock().unwrap_or_else(|e| e.into_inner())
            && t.elapsed() < STATE_TTL {
                return Ok(s.clone());
            }
        self.fetch()
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn invalidate(&self) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    /// Raise and focus a window by its stable identifier.
    pub fn activate(&self, window: &str) -> Result<()> {
        if window.is_empty() || !window.chars().all(|c| c.is_ascii_digit()) {
            return Err(AdlError::invalid("GNOME Shell window identifiers are decimal numbers"));
        }
        let found = self.source.activate(window);
        self.invalidate();
        if !found? {
            return Err(AdlError::not_found(format!("no window {window}")));
        }
        Ok(())
    }
}

/// An input/capture adapter (the portal backend) with window data from the GNOME Shell extension.
pub struct WithGnome<A: Adapter> {
    input: A,
    shell: ShellObserver,
}

impl<A: Adapter> WithGnome<A> {
    pub fn new(input: A, shell: ShellObserver) -> Self {
        Self { input, shell }
    }

    pub fn activate(&self, window: &str) -> Result<()> {
        self.shell.activate(window)
    }

    /// Name the input adapter's display under each window's centre, so ids match `layout()`.
    fn place(&self, mut ws: Vec<WindowInfo>) -> Vec<WindowInfo> {
        if let Ok(l) = self.input.layout() {
            for w in &mut ws {
                if let Some(b) = w.bounds {
                    w.display = l.displays.iter().find(|d| d.logical.contains(b.x + b.width / 2.0, b.y + b.height / 2.0)).map(|d| d.id.clone());
                }
            }
        }
        ws
    }
}

impl<A: Adapter> Adapter for WithGnome<A> {
    fn name(&self) -> String {
        format!("{}+gnome", self.input.name())
    }

    fn capabilities(&self) -> Vec<CapabilityReport> {
        let windows = match self.shell.last_error() {
            None => CapabilityReport {
                capability: Capability::Windows,
                availability: Availability::Available,
                backend: "linux-gnome-shell".into(),
                reason: String::new(),
                features: ["window_at", "focus", "geometry", "activate"].iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(),
            },
            Some(e) => CapabilityReport { capability: Capability::Windows, availability: Availability::Degraded, backend: "linux-gnome-shell".into(), reason: format!("last GNOME Shell extension query failed: {e}"), features: BTreeSet::new() },
        };
        let mut caps: Vec<_> = self.input.capabilities().into_iter().filter(|c| c.capability != Capability::Windows).collect();
        caps.insert(0, windows);
        caps
    }

    fn begin(&self) -> Result<()> {
        self.input.begin()?;
        // Without window data every targeted action would be refused; say why at enable time.
        if let Err(e) = self.shell.fetch() {
            self.input.end();
            return Err(AdlError::new(ErrorCode::Blocked, format!("cannot read window state: {}", e.message)));
        }
        Ok(())
    }

    fn end(&self) {
        self.input.end();
        self.shell.invalidate();
    }

    fn close_streams(&self) -> usize {
        self.input.close_streams()
    }

    fn layout(&self) -> Result<Layout> {
        self.input.layout()
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        Ok(self.place(self.shell.snapshot()?.windows()))
    }

    fn focused_window(&self) -> Result<Option<WindowInfo>> {
        // Always fresh: focus checks guard keyboard delivery.
        Ok(self.place(self.shell.fetch()?.focused().into_iter().collect()).pop())
    }

    fn window_at(&self, x: f64, y: f64) -> Result<Option<WindowInfo>> {
        Ok(self.place(self.shell.snapshot()?.window_at(x, y).into_iter().collect()).pop())
    }

    fn inspect(&self, window: &str) -> Result<AccessNode> {
        self.input.inspect(window)
    }

    fn capture(&self, display: &str) -> Result<(Vec<u8>, u32, u32)> {
        self.input.capture(display)
    }

    fn pointer_position(&self) -> Result<Option<(f64, f64)>> {
        self.input.pointer_position()
    }

    fn emit(&self, event: &InputEvent) -> Result<()> {
        self.input.emit(event)
    }

    fn safety_interlock(&self) -> Option<String> {
        if let Some(why) = self.input.safety_interlock() {
            return Some(why);
        }
        // Fresh every action; this also refreshes the cache that window_at reads right after.
        match self.shell.fetch() {
            Ok(s) => s.interlock(),
            Err(e) => Some(format!("cannot read window state: {}", e.message)),
        }
    }

    fn poll_events(&self) -> Vec<SessionEvent> {
        self.input.poll_events()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adl_broker::mock::MockDesktop;

    /// Topmost first: the top bar, a popup menu, two foot terminals (T2 cascaded over T1), a
    /// minimized editor, a terminal on another workspace.
    const REPORT: &str = r#"{
      "version": 1,
      "source": "get_window_actors",
      "focused": "12",
      "interlock": "",
      "windows": [
        {"id": "shell:top-bar", "app_id": "org.gnome.Shell", "title": "Top bar", "x": 0, "y": 0, "width": 1920, "height": 32, "type": "shell"},
        {"id": "40", "app_id": "foot", "wm_class": "foot", "title": "", "x": 420, "y": 320, "width": 120, "height": 80, "type": "popup_menu"},
        {"id": "12", "app_id": "foot", "wm_class": "foot", "title": "T2", "x": 400, "y": 300, "width": 600, "height": 400, "type": "normal", "focused": true},
        {"id": "13", "app_id": "", "wm_class": "org.w3bs.rig.other", "title": "elsewhere", "x": 1000, "y": 600, "width": 300, "height": 200, "type": "normal", "on_current_workspace": false},
        {"id": "14", "app_id": "org.gnome.TextEditor", "title": "notes", "x": 900, "y": 100, "width": 300, "height": 300, "type": "normal", "minimized": true, "hidden": true},
        {"id": "11", "app_id": "foot", "wm_class": "foot", "title": "T1", "x": 100, "y": 100, "width": 600, "height": 400, "type": "normal"},
        {"id": "15", "app_id": "org.gnome.Settings", "title": "About", "x": 1300, "y": 100, "width": 300, "height": 200, "type": "modal_dialog"}
      ]
    }"#;

    struct Canned(Mutex<String>, Mutex<Vec<String>>);
    impl ShellSource for Canned {
        fn snapshot(&self) -> Result<ShellSnapshot> {
            ShellSnapshot::parse(&self.0.lock().unwrap())
        }
        fn activate(&self, id: &str) -> Result<bool> {
            self.1.lock().unwrap().push(id.to_string());
            Ok(self.0.lock().unwrap().contains(&format!("\"{id}\"")))
        }
    }

    fn snap() -> ShellSnapshot {
        ShellSnapshot::parse(REPORT).unwrap()
    }

    #[test]
    fn lists_user_windows_topmost_first() {
        let ws = snap().windows();
        let ids: Vec<_> = ws.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["12", "13", "14", "11", "15"], "no top bar, no popup menu");
        assert_eq!(ws.iter().filter(|w| w.focused).count(), 1);
        assert_eq!(ws[0].title, "T2");
        assert!(ws[0].focused);
        assert_eq!(ws[1].app_id, "org.w3bs.rig.other", "falls back to wm_class");
        assert_eq!(ws[1].bounds, None, "other workspace has no on-screen box");
        assert_eq!(ws[2].bounds, None, "minimized has no on-screen box");
        assert_eq!(ws[3].bounds, Some(Rect { x: 100.0, y: 100.0, width: 600.0, height: 400.0 }));
    }

    #[test]
    fn window_at_respects_stacking_and_occluders() {
        let s = snap();
        assert_eq!(s.window_at(150.0, 150.0).unwrap().id, "11", "visible part of T1");
        assert_eq!(s.window_at(600.0, 450.0).unwrap().id, "12", "T2 covers T1 here");
        assert_eq!(s.window_at(450.0, 350.0).unwrap().id, "40", "an open popup menu covers T2");
        assert_eq!(s.window_at(500.0, 10.0).unwrap().id, "shell:top-bar", "the top bar covers windows under it");
        assert!(s.window_at(1100.0, 650.0).is_none(), "off-workspace window is not hit");
        assert!(s.window_at(950.0, 150.0).is_none(), "minimized window is not hit");
        assert!(s.window_at(5000.0, 10.0).is_none());
    }

    #[test]
    fn focus_interlock_and_bad_reports() {
        let s = snap();
        assert_eq!(s.focused().unwrap().title, "T2");
        assert_eq!(s.interlock(), None);
        let modal = REPORT.replace(r#""interlock": """#, r#""interlock": "the Activities overview is open""#);
        assert_eq!(ShellSnapshot::parse(&modal).unwrap().interlock().as_deref(), Some("the Activities overview is open"));
        assert!(ShellSnapshot::parse("{\"version\": 1, \"windows\": 3}").is_err());
        let v2 = ShellSnapshot::parse(&REPORT.replace(r#""version": 1"#, r#""version": 2"#)).unwrap_err();
        assert!(v2.message.contains("snapshot version 2"), "{}", v2.message);
        assert!(ShellSnapshot::parse("{}").is_err(), "no version means an unknown producer");
        assert_eq!(ShellSnapshot::parse(r#"{"version": 1}"#).unwrap().focused(), None);
    }

    #[test]
    fn composite_uses_gnome_for_windows_and_keeps_input() {
        let src = Canned(Mutex::new(REPORT.into()), Mutex::new(vec![]));
        let a = WithGnome::new(MockDesktop::new(), ShellObserver::new(Box::new(src)));
        assert_eq!(a.name(), "mock+gnome");
        let caps = a.capabilities();
        let w = caps.iter().find(|c| c.capability == Capability::Windows).unwrap();
        assert_eq!((w.availability, w.backend.as_str()), (Availability::Available, "linux-gnome-shell"));
        assert!(w.features.contains("window_at") && w.features.contains("activate"));
        assert_eq!(caps.iter().filter(|c| c.capability == Capability::Windows).count(), 1);
        assert_eq!(a.focused_window().unwrap().unwrap().id, "12");
        assert_eq!(a.window_at(600.0, 450.0).unwrap().unwrap().title, "T2");
        assert!(a.windows().unwrap().iter().all(|w| w.bounds.is_none() || w.display.is_some()), "visible windows are placed on a display");
        assert_eq!(a.safety_interlock(), None);
        assert!(a.activate("11").is_ok());
        assert_eq!(a.activate("9999").unwrap_err().code, ErrorCode::NotFound);
        assert_eq!(a.activate("11; rm").unwrap_err().code, ErrorCode::Invalid);
        assert_eq!(a.activate("shell:top-bar").unwrap_err().code, ErrorCode::Invalid);
    }

    #[test]
    fn interlock_blocks_input() {
        let src = Canned(Mutex::new(REPORT.replace(r#""interlock": """#, r#""interlock": "the session is locked""#)), Mutex::new(vec![]));
        let a = WithGnome::new(MockDesktop::new(), ShellObserver::new(Box::new(src)));
        assert_eq!(a.safety_interlock().as_deref(), Some("the session is locked"));
    }

    #[test]
    fn unreachable_extension_fails_closed() {
        struct Down;
        impl ShellSource for Down {
            fn snapshot(&self) -> Result<ShellSnapshot> {
                Err(err("org.w3bs.Desktop.Shell is not on the session bus"))
            }
            fn activate(&self, _: &str) -> Result<bool> {
                Err(err("down"))
            }
        }
        let a = WithGnome::new(MockDesktop::new(), ShellObserver::new(Box::new(Down)));
        assert!(a.safety_interlock().unwrap().contains("cannot read window state"));
        assert!(a.windows().is_err());
        assert!(a.begin().is_err());
        let w = a.capabilities().into_iter().find(|c| c.capability == Capability::Windows).unwrap();
        assert_eq!(w.availability, Availability::Degraded);
    }

    #[test]
    fn detects_gnome_but_not_vela() {
        // On the string, not the environment: kwin.rs's test sets XDG_CURRENT_DESKTOP concurrently.
        for (v, want) in [("GNOME", true), ("ubuntu:GNOME", true), ("gnome", true), ("Vela:GNOME", false), ("KDE", false), ("GNOME-Flashback:GNOME", true), ("", false)] {
            assert_eq!(is_gnome(v), want, "{v}");
        }
    }
}
