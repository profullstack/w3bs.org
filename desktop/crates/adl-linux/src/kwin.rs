//! KDE Plasma window observer: window list, geometry, stacking, focus, activation and the
//! authentication-prompt interlock, through KWin's scripting interface.
//!
//! The XDG portals give input and capture but say nothing about windows, and KWin has no D-Bus
//! method that lists them. KWin does run scripts on request, so for each query this observer:
//!
//! 1. writes a short one-shot script (0600, under `$XDG_RUNTIME_DIR/w3bs-desktop/`),
//! 2. loads it with `org.kde.kwin.Scripting.loadScript(path, plugin)` and calls `run` on the
//!    returned `/Scripting/Script<id>` object,
//! 3. waits for the script to answer with `callDBus(...)` into this process: method `Report` of
//!    `org.w3bs.Desktop.KWinBridge` at `/org/w3bs/desktop/kwin` on our own unique bus name, with
//!    a per-query nonce. Reports from any sender other than the current owner of `org.kde.KWin`,
//!    or with a stale nonce, are dropped,
//! 4. unloads the script.
//!
//! Only KWin's documented scripting API is used (`workspace.stackingOrder`,
//! `workspace.activeWindow`, `workspace.currentDesktop` and documented `Window` properties). No
//! KWin source or third-party tool code was consulted or copied.
//!
//! Window ids are KWin's `internalId` (a UUID, braces stripped), stable for a window's lifetime.
//! The app id is `desktopFileName` when KWin knows it, else `resourceClass`. `windows()` lists
//! managed user windows (normal, dialog, utility); `window_at()` walks every visible surface top
//! to bottom, panels, notifications and the desktop included, because any of them can cover a
//! target.

use adl_broker::{Adapter, InputEvent, SessionEvent};
use adl_schema::*;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

pub const BRIDGE_PATH: &str = "/org/w3bs/desktop/kwin";
pub const BRIDGE_IFACE: &str = "org.w3bs.Desktop.KWinBridge";
const KWIN: &str = "org.kde.KWin";
/// Snapshots older than this are re-queried. Focus and interlocks always re-query.
const STATE_TTL: Duration = Duration::from_millis(100);
/// How long a script has to call back before the query fails.
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// Window classes of KDE's polkit authentication agent (X11 class, then Wayland app id).
const POLKIT_CLASSES: &[&str] = &["polkit-kde-authentication-agent-1", "org.kde.polkit-kde-authentication-agent-1", "polkit-kde-manager"];
/// The screen locker greeter, when KWin manages it as a window (best effort; logind is primary).
const LOCKER_CLASSES: &[&str] = &["kscreenlocker_greet", "org.kde.kscreenlocker_greet"];

fn yes() -> bool {
    true
}

/// One surface as the snapshot script reports it.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KWinWindow {
    pub id: String,
    #[serde(default)]
    pub resource_class: String,
    #[serde(default)]
    pub desktop_file_name: String,
    #[serde(default)]
    pub caption: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub minimized: bool,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub normal: bool,
    #[serde(default)]
    pub dialog: bool,
    #[serde(default)]
    pub utility: bool,
    #[serde(default = "yes")]
    pub managed: bool,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default = "yes")]
    pub on_current_desktop: bool,
    #[serde(default)]
    pub output: String,
}

impl KWinWindow {
    pub fn app_id(&self) -> &str {
        if self.desktop_file_name.is_empty() { &self.resource_class } else { &self.desktop_file_name }
    }

    /// A window a person works in, as opposed to a panel, the desktop, a tooltip or an OSD.
    pub fn is_user_window(&self) -> bool {
        self.managed && !self.deleted && (self.normal || self.dialog || self.utility)
    }

    /// Drawn on screen right now (it may still be covered by something above it).
    pub fn is_visible(&self) -> bool {
        !self.deleted && !self.minimized && !self.hidden && self.on_current_desktop && self.width > 0.0 && self.height > 0.0
    }

    fn rect(&self) -> Rect {
        Rect { x: self.x, y: self.y, width: self.width, height: self.height }
    }

    fn is_class(&self, classes: &[&str]) -> bool {
        let (c, d) = (self.resource_class.to_ascii_lowercase(), self.desktop_file_name.to_ascii_lowercase());
        classes.iter().any(|k| c == *k || d == *k)
    }
}

/// One answer from the snapshot script. `windows` is in KWin's stacking order: bottom-most first.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct KWinSnapshot {
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub windows: Vec<KWinWindow>,
    /// Which workspace property the list came from (`stackingOrder` or the `windowList()` fallback).
    #[serde(default)]
    pub source: String,
}

impl KWinSnapshot {
    pub fn parse(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|e| AdlError::internal(format!("unexpected KWin script report: {e}")))
    }

    /// Every surface, topmost first.
    pub fn topmost_first(&self) -> impl Iterator<Item = &KWinWindow> {
        self.windows.iter().rev()
    }

    fn info(&self, w: &KWinWindow) -> WindowInfo {
        WindowInfo {
            id: w.id.clone(),
            app_id: w.app_id().to_string(),
            title: w.caption.clone(),
            // Minimized windows and windows on another virtual desktop have no on-screen box.
            bounds: w.is_visible().then(|| w.rect()),
            display: None,
            focused: self.active.as_deref() == Some(w.id.as_str()),
        }
    }

    /// Managed user windows, topmost first.
    pub fn windows(&self) -> Vec<WindowInfo> {
        self.topmost_first().filter(|w| w.is_user_window()).map(|w| self.info(w)).collect()
    }

    pub fn focused(&self) -> Option<WindowInfo> {
        let id = self.active.as_deref()?;
        self.windows.iter().find(|w| w.id == id).map(|w| self.info(w))
    }

    /// The topmost visible surface at a desktop logical point, of any type.
    pub fn window_at(&self, x: f64, y: f64) -> Option<WindowInfo> {
        self.topmost_first().find(|w| w.is_visible() && w.rect().contains(x, y)).map(|w| self.info(w))
    }

    /// Why input must not be delivered now, if anything.
    pub fn interlock(&self) -> Option<String> {
        if self.windows.iter().any(|w| !w.deleted && w.is_class(LOCKER_CLASSES)) {
            return Some("the session is locked".into());
        }
        if self.windows.iter().any(|w| !w.deleted && w.is_class(POLKIT_CLASSES)) {
            return Some("an authentication prompt is on screen; the human must answer it".into());
        }
        None
    }
}

/// Where snapshots come from: KWin over D-Bus, or a canned source in tests.
pub trait KWinSource: Send + Sync {
    fn snapshot(&self) -> Result<KWinSnapshot>;
    /// Raise and focus a window by id. `Ok(false)` when KWin has no such window.
    fn activate(&self, id: &str) -> Result<bool>;
}

/// Snapshot script. `ADL_*` constants are prepended per query.
const SNAPSHOT_JS: &str = r#"
(function () {
    function get(o, k, d) { try { var v = o[k]; return (v === undefined || v === null) ? d : v; } catch (e) { return d; } }
    function wid(w) { return String(get(w, "internalId", "")).replace(/[{}]/g, ""); }
    var cur = get(workspace, "currentDesktop", null);
    function onCurrent(w) {
        if (get(w, "onAllDesktops", false)) return true;
        var ds = get(w, "desktops", []);
        if (!cur || !ds || ds.length === 0) return true;
        for (var i = 0; i < ds.length; i++) if (ds[i] && ds[i].id === cur.id) return true;
        return false;
    }
    var list = get(workspace, "stackingOrder", null), source = "stackingOrder";
    if (!list) { list = workspace.windowList(); source = "windowList"; }
    var active = get(workspace, "activeWindow", null);
    var out = { source: source, active: active ? wid(active) : null, windows: [] };
    for (var i = 0; i < list.length; i++) {
        var w = list[i], g = get(w, "frameGeometry", null), o = get(w, "output", null);
        if (!w || !g) continue;
        out.windows.push({
            id: wid(w), resourceClass: String(get(w, "resourceClass", "")), desktopFileName: String(get(w, "desktopFileName", "")),
            caption: String(get(w, "caption", "")), x: g.x, y: g.y, width: g.width, height: g.height,
            minimized: !!get(w, "minimized", false), hidden: !!get(w, "hidden", false),
            normal: !!get(w, "normalWindow", false), dialog: !!get(w, "dialog", false), utility: !!get(w, "utility", false),
            managed: !!get(w, "managed", true), deleted: !!get(w, "deleted", false),
            onCurrentDesktop: onCurrent(w), output: o ? String(get(o, "name", "")) : ""
        });
    }
    callDBus(ADL_SERVICE, ADL_PATH, ADL_IFACE, "Report", ADL_NONCE, JSON.stringify(out));
})();
"#;

/// Activation script; `ADL_TARGET` is the window id.
const ACTIVATE_JS: &str = r#"
(function () {
    function wid(w) { return String(w.internalId).replace(/[{}]/g, ""); }
    var list = workspace.stackingOrder || workspace.windowList(), found = false, ok = false;
    for (var i = 0; i < list.length; i++) {
        var w = list[i];
        if (!w || wid(w) !== ADL_TARGET) continue;
        found = true;
        if (w.minimized) w.minimized = false;
        workspace.activeWindow = w;
        ok = !!workspace.activeWindow && wid(workspace.activeWindow) === ADL_TARGET;
    }
    callDBus(ADL_SERVICE, ADL_PATH, ADL_IFACE, "Report", ADL_NONCE, JSON.stringify({ found: found, activated: ok }));
})();
"#;

struct Pending {
    nonce: String,
    sender: String,
    tx: mpsc::Sender<String>,
}

/// The object KWin scripts call back into.
struct Bridge {
    pending: Arc<Mutex<Option<Pending>>>,
}

#[zbus::interface(name = "org.w3bs.Desktop.KWinBridge")]
impl Bridge {
    fn report(&self, #[zbus(header)] hdr: zbus::message::Header<'_>, nonce: String, json: String) {
        let mut slot = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        let from_kwin = |p: &Pending| p.nonce == nonce && hdr.sender().is_some_and(|s| s.as_str() == p.sender);
        if slot.as_ref().is_some_and(from_kwin)
            && let Some(p) = slot.take() {
                let _ = p.tx.send(json);
            }
    }
}

/// [`KWinSource`] over the session bus.
pub struct KWinScripts {
    conn: zbus::blocking::Connection,
    pending: Arc<Mutex<Option<Pending>>>,
    /// Serializes queries (one loaded script at a time) and numbers them.
    serial: Mutex<u64>,
    dir: PathBuf,
    plugin: String,
}

fn err(e: impl std::fmt::Display) -> AdlError {
    AdlError::new(ErrorCode::Blocked, format!("KWin: {e}"))
}

/// A JS string literal.
fn js(s: &str) -> String {
    serde_json::Value::from(s).to_string()
}

impl KWinScripts {
    /// Connect to the session bus and export the report object.
    pub fn connect() -> Result<Self> {
        let pending = Arc::new(Mutex::new(None));
        let conn = zbus::blocking::connection::Builder::session()
            .and_then(|b| b.serve_at(BRIDGE_PATH, Bridge { pending: pending.clone() }))
            .and_then(|b| b.build())
            .map_err(err)?;
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("w3bs-desktop");
        Ok(Self { conn, pending, serial: Mutex::new(0), dir, plugin: format!("w3bs-adl-{}", std::process::id()) })
    }

    fn write_script(&self, path: &std::path::Path, text: &str) -> Result<()> {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&self.dir);
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path).map_err(|e| err(format!("{}: {e}", path.display())))?;
        f.write_all(text.as_bytes()).map_err(|e| err(format!("{}: {e}", path.display())))
    }

    /// Load, run and unload one script; return what it reported.
    fn run(&self, body: &str, extra: &[(&str, &str)]) -> Result<String> {
        let mut serial = self.serial.lock().unwrap_or_else(|e| e.into_inner());
        *serial += 1;
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let nonce = format!("{:x}-{}", t as u64 ^ std::process::id() as u64, *serial);
        let me = self.conn.unique_name().map(|n| n.to_string()).ok_or_else(|| err("no unique bus name"))?;
        let bus = zbus::blocking::fdo::DBusProxy::new(&self.conn).map_err(err)?;
        let owner = bus.get_name_owner(KWIN.try_into().map_err(err)?).map_err(|e| err(format!("KWin is not on the session bus ({e})")))?.to_string();

        let mut head = format!("var ADL_SERVICE = {}, ADL_PATH = {}, ADL_IFACE = {}, ADL_NONCE = {};\n", js(&me), js(BRIDGE_PATH), js(BRIDGE_IFACE), js(&nonce));
        for (k, v) in extra {
            head += &format!("var {k} = {};\n", js(v));
        }
        let path = self.dir.join(format!("kwin-{nonce}.js"));
        self.write_script(&path, &(head + body))?;

        let (tx, rx) = mpsc::channel();
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending { nonce, sender: owner, tx });
        let scripting = zbus::blocking::Proxy::new(&self.conn, KWIN, "/Scripting", "org.kde.kwin.Scripting").map_err(err);
        let outcome = scripting.and_then(|s| {
            let path_s = path.to_string_lossy().to_string();
            let load = || -> Result<i32> { s.call("loadScript", &(path_s.as_str(), self.plugin.as_str())).map_err(err) };
            let mut id = load()?;
            if id < 0 {
                // A previous query of ours left its plugin loaded (e.g. a timeout); clear it once.
                let _: zbus::Result<bool> = s.call("unloadScript", &(self.plugin.as_str(),));
                id = load()?;
            }
            if id < 0 {
                return Err(err("loadScript refused the query script"));
            }
            let script = zbus::blocking::Proxy::new(&self.conn, KWIN, format!("/Scripting/Script{id}"), "org.kde.kwin.Script").map_err(err)?;
            script.call_method("run", &()).map_err(err)?;
            let reply = rx.recv_timeout(REPLY_TIMEOUT).map_err(|_| err("the query script did not report back (is scripting disabled?)"));
            let _: zbus::Result<bool> = s.call("unloadScript", &(self.plugin.as_str(),));
            reply
        });
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).take();
        let _ = std::fs::remove_file(&path);
        outcome
    }
}

impl KWinSource for KWinScripts {
    fn snapshot(&self) -> Result<KWinSnapshot> {
        KWinSnapshot::parse(&self.run(SNAPSHOT_JS, &[])?)
    }

    fn activate(&self, id: &str) -> Result<bool> {
        #[derive(Deserialize)]
        struct Activated {
            found: bool,
            activated: bool,
        }
        let r: Activated = serde_json::from_str(&self.run(ACTIVATE_JS, &[("ADL_TARGET", id)])?).map_err(|e| AdlError::internal(format!("unexpected KWin activate report: {e}")))?;
        if r.found && !r.activated {
            return Err(AdlError::new(ErrorCode::Blocked, "KWin refused to activate the window (focus stealing prevention?)"));
        }
        Ok(r.found)
    }
}

/// Cached window state from a [`KWinSource`].
pub struct KWinObserver {
    source: Box<dyn KWinSource>,
    cache: Mutex<Option<(Instant, KWinSnapshot)>>,
    last_error: Mutex<Option<String>>,
}

impl KWinObserver {
    pub fn new(source: Box<dyn KWinSource>) -> Self {
        Self { source, cache: Mutex::new(None), last_error: Mutex::new(None) }
    }

    /// A KDE Plasma session that is not Vela (Vela lists itself first, e.g. `Vela:KDE`).
    pub fn detect() -> bool {
        let d = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        d.split(':').next() != Some("Vela") && d.split(':').any(|c| c.eq_ignore_ascii_case("KDE"))
    }

    pub fn fetch(&self) -> Result<KWinSnapshot> {
        let r = self.source.snapshot();
        *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = r.as_ref().err().map(|e| e.message.clone());
        let s = r?;
        *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), s.clone()));
        Ok(s)
    }

    pub fn snapshot(&self) -> Result<KWinSnapshot> {
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
        if window.is_empty() || !window.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            return Err(AdlError::invalid("KWin window identifiers are UUIDs"));
        }
        let found = self.source.activate(window);
        self.invalidate();
        if !found? {
            return Err(AdlError::not_found(format!("no window {window}")));
        }
        Ok(())
    }
}

/// An input/capture adapter (the portal backend) with window data from KWin.
pub struct WithKWin<A: Adapter> {
    input: A,
    kwin: KWinObserver,
}

impl<A: Adapter> WithKWin<A> {
    pub fn new(input: A, kwin: KWinObserver) -> Self {
        Self { input, kwin }
    }

    pub fn activate(&self, window: &str) -> Result<()> {
        self.kwin.activate(window)
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

impl<A: Adapter> Adapter for WithKWin<A> {
    fn name(&self) -> String {
        format!("{}+kwin", self.input.name())
    }

    fn capabilities(&self) -> Vec<CapabilityReport> {
        let windows = match self.kwin.last_error() {
            None => CapabilityReport {
                capability: Capability::Windows,
                availability: Availability::Available,
                backend: "linux-kwin".into(),
                reason: String::new(),
                features: ["window_at", "focus", "geometry", "activate"].iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(),
            },
            Some(e) => CapabilityReport { capability: Capability::Windows, availability: Availability::Degraded, backend: "linux-kwin".into(), reason: format!("last KWin query failed: {e}"), features: BTreeSet::new() },
        };
        let mut caps: Vec<_> = self.input.capabilities().into_iter().filter(|c| c.capability != Capability::Windows).collect();
        caps.insert(0, windows);
        caps
    }

    fn begin(&self) -> Result<()> {
        self.input.begin()?;
        // Without window data every targeted action would be refused; say why at enable time.
        if let Err(e) = self.kwin.fetch() {
            self.input.end();
            return Err(AdlError::new(ErrorCode::Blocked, format!("cannot read KWin window state: {}", e.message)));
        }
        Ok(())
    }

    fn end(&self) {
        self.input.end();
        self.kwin.invalidate();
    }

    fn close_streams(&self) -> usize {
        self.input.close_streams()
    }

    fn layout(&self) -> Result<Layout> {
        self.input.layout()
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        Ok(self.place(self.kwin.snapshot()?.windows()))
    }

    fn focused_window(&self) -> Result<Option<WindowInfo>> {
        // Always fresh: focus checks guard keyboard delivery.
        Ok(self.place(self.kwin.fetch()?.focused().into_iter().collect()).pop())
    }

    fn window_at(&self, x: f64, y: f64) -> Result<Option<WindowInfo>> {
        Ok(self.place(self.kwin.snapshot()?.window_at(x, y).into_iter().collect()).pop())
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
        match self.kwin.fetch() {
            Ok(s) => s.interlock(),
            Err(e) => Some(format!("cannot read KWin window state: {}", e.message)),
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

    /// Two foot terminals (T2 cascaded over T1), a panel on top, the desktop at the bottom.
    const REPORT: &str = r#"{
      "source": "stackingOrder",
      "active": "bbbb",
      "windows": [
        {"id": "dddd", "resourceClass": "plasmashell", "desktopFileName": "org.kde.plasmashell", "caption": "Desktop", "x": 0, "y": 0, "width": 1920, "height": 1080, "managed": true},
        {"id": "aaaa", "resourceClass": "foot", "desktopFileName": "", "caption": "T1", "x": 100, "y": 100, "width": 600, "height": 400, "normal": true, "output": "Virtual-1"},
        {"id": "eeee", "resourceClass": "kwrite", "desktopFileName": "org.kde.kwrite", "caption": "notes", "x": 900, "y": 100, "width": 300, "height": 300, "normal": true, "minimized": true},
        {"id": "ffff", "resourceClass": "foot", "caption": "elsewhere", "x": 1000, "y": 600, "width": 300, "height": 200, "normal": true, "onCurrentDesktop": false},
        {"id": "bbbb", "resourceClass": "foot", "desktopFileName": "foot", "caption": "T2", "x": 400, "y": 300, "width": 600, "height": 400, "normal": true, "output": "Virtual-1"},
        {"id": "cccc", "resourceClass": "plasmashell", "desktopFileName": "org.kde.plasmashell", "caption": "Panel", "x": 0, "y": 1036, "width": 1920, "height": 44}
      ]
    }"#;

    struct Canned(Mutex<String>, Mutex<Vec<String>>);
    impl KWinSource for Canned {
        fn snapshot(&self) -> Result<KWinSnapshot> {
            KWinSnapshot::parse(&self.0.lock().unwrap())
        }
        fn activate(&self, id: &str) -> Result<bool> {
            self.1.lock().unwrap().push(id.to_string());
            Ok(self.0.lock().unwrap().contains(&format!("\"{id}\"")))
        }
    }

    fn snap() -> KWinSnapshot {
        KWinSnapshot::parse(REPORT).unwrap()
    }

    #[test]
    fn lists_user_windows_topmost_first() {
        let ws = snap().windows();
        let ids: Vec<_> = ws.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["bbbb", "ffff", "eeee", "aaaa"]);
        assert_eq!(ws.iter().filter(|w| w.focused).count(), 1);
        assert_eq!(ws[0].app_id, "foot");
        assert_eq!(ws[3].app_id, "foot", "falls back to resourceClass");
        assert_eq!(ws[2].app_id, "org.kde.kwrite", "prefers desktopFileName");
        assert_eq!(ws[2].bounds, None, "minimized has no on-screen box");
        assert_eq!(ws[1].bounds, None, "other virtual desktop has no on-screen box");
        assert_eq!(ws[3].bounds, Some(Rect { x: 100.0, y: 100.0, width: 600.0, height: 400.0 }));
    }

    #[test]
    fn window_at_respects_stacking_and_occluders() {
        let s = snap();
        assert_eq!(s.window_at(150.0, 150.0).unwrap().id, "aaaa", "visible part of T1");
        assert_eq!(s.window_at(500.0, 400.0).unwrap().id, "bbbb", "T2 covers T1 here");
        assert_eq!(s.window_at(1100.0, 650.0).unwrap().id, "dddd", "off-desktop window is not hit");
        assert_eq!(s.window_at(950.0, 150.0).unwrap().id, "dddd", "minimized window is not hit");
        let p = s.window_at(500.0, 1050.0).unwrap();
        assert_eq!((p.id.as_str(), p.app_id.as_str()), ("cccc", "org.kde.plasmashell"), "the panel covers windows under it");
        assert!(s.window_at(5000.0, 10.0).is_none());
    }

    #[test]
    fn focus_and_interlocks() {
        let s = snap();
        assert_eq!(s.focused().unwrap().title, "T2");
        assert_eq!(s.interlock(), None);
        let polkit = REPORT.replace(r#""resourceClass": "kwrite""#, r#""resourceClass": "polkit-kde-authentication-agent-1""#);
        assert!(KWinSnapshot::parse(&polkit).unwrap().interlock().unwrap().contains("authentication prompt"));
        let wl = REPORT.replace(r#""desktopFileName": "org.kde.kwrite""#, r#""desktopFileName": "org.kde.polkit-kde-authentication-agent-1""#);
        assert!(KWinSnapshot::parse(&wl).unwrap().interlock().is_some());
        let locked = REPORT.replace(r#""resourceClass": "kwrite""#, r#""resourceClass": "kscreenlocker_greet""#);
        assert_eq!(KWinSnapshot::parse(&locked).unwrap().interlock().as_deref(), Some("the session is locked"));
        assert!(KWinSnapshot::parse("{\"windows\": 3}").is_err());
        assert_eq!(KWinSnapshot::parse("{}").unwrap().focused(), None);
    }

    #[test]
    fn composite_uses_kwin_for_windows_and_keeps_input() {
        let src = Canned(Mutex::new(REPORT.into()), Mutex::new(vec![]));
        let a = WithKWin::new(MockDesktop::new(), KWinObserver::new(Box::new(src)));
        assert_eq!(a.name(), "mock+kwin");
        let caps = a.capabilities();
        let w = caps.iter().find(|c| c.capability == Capability::Windows).unwrap();
        assert_eq!(w.availability, Availability::Available);
        assert!(w.features.contains("window_at") && w.features.contains("activate"));
        assert_eq!(caps.iter().filter(|c| c.capability == Capability::Windows).count(), 1);
        assert_eq!(a.focused_window().unwrap().unwrap().id, "bbbb");
        assert_eq!(a.window_at(500.0, 400.0).unwrap().unwrap().title, "T2");
        assert!(a.windows().unwrap().iter().all(|w| w.bounds.is_none() || w.display.is_some()), "visible windows are placed on a display");
        assert_eq!(a.safety_interlock(), None);
        assert!(a.activate("aaaa").is_ok());
        assert_eq!(a.activate("9999").unwrap_err().code, ErrorCode::NotFound);
        assert_eq!(a.activate("x; workspace").unwrap_err().code, ErrorCode::Invalid);
    }

    #[test]
    fn unreachable_kwin_fails_closed() {
        struct Down;
        impl KWinSource for Down {
            fn snapshot(&self) -> Result<KWinSnapshot> {
                Err(err("KWin is not on the session bus"))
            }
            fn activate(&self, _: &str) -> Result<bool> {
                Err(err("down"))
            }
        }
        let a = WithKWin::new(MockDesktop::new(), KWinObserver::new(Box::new(Down)));
        assert!(a.safety_interlock().unwrap().contains("cannot read KWin"));
        assert!(a.windows().is_err());
        assert!(a.begin().is_err());
        let w = a.capabilities().into_iter().find(|c| c.capability == Capability::Windows).unwrap();
        assert_eq!(w.availability, Availability::Degraded);
    }

    #[test]
    fn detects_kde_but_not_vela() {
        // Env is process-global; keep every variant in one test.
        let saved = std::env::var_os("XDG_CURRENT_DESKTOP");
        for (v, want) in [("KDE", true), ("kde", true), ("Vela:KDE", false), ("GNOME", false), ("X-Generic:KDE", true)] {
            unsafe { std::env::set_var("XDG_CURRENT_DESKTOP", v) };
            assert_eq!(KWinObserver::detect(), want, "{v}");
        }
        match saved {
            Some(s) => unsafe { std::env::set_var("XDG_CURRENT_DESKTOP", s) },
            None => unsafe { std::env::remove_var("XDG_CURRENT_DESKTOP") },
        }
    }

    #[test]
    fn js_literals_are_escaped() {
        assert_eq!(js("a\"b\\c\n"), r#""a\"b\\c\n""#);
    }
}
