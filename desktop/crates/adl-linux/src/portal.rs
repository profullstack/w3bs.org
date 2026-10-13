//! KDE Plasma / GNOME backend over the XDG desktop portals (PRD section 3).
//!
//! - Input: `org.freedesktop.portal.RemoteDesktop`, pointer + keyboard devices, with a monitor
//!   ScreenCast in the same session so absolute motion has stream coordinates. Only the Notify*
//!   D-Bus methods are used; EIS is never opened on the same session, so the two input paths are
//!   never mixed.
//! - Capture: `org.freedesktop.portal.Screenshot`, non-interactive, cropped per display.
//! - Consent: the portal shows its own dialog when the human enables; the restore token is kept so
//!   later enables can skip it where the portal allows (persist mode 2 = until revoked).
//!
//! Window listing, focus and occlusion need desktop-specific help (a KWin script on Plasma, the
//! W3BS Shell extension on GNOME). Until those land, Windows is reported unsupported, so targeted
//! clicks and typing into a named window are refused rather than guessed.
//!
//! Status: compiles and follows the portal spec; NOT certified on any desktop yet.

use crate::keymap;
use adl_broker::{Adapter, InputEvent, SessionEvent};
use adl_schema::*;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const DEST: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const DEVICE_KEYBOARD: u32 = 1;
const DEVICE_POINTER: u32 = 2;
const SOURCE_MONITOR: u32 = 1;
const PERSIST_UNTIL_REVOKED: u32 = 2;
/// How long to wait for the human to answer a portal dialog.
const CONSENT_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone, Debug)]
struct Stream {
    node: u32,
    rect: Rect,
}

struct Session {
    handle: OwnedObjectPath,
    devices: u32,
    streams: Vec<Stream>,
}

pub struct PortalAdapter {
    conn: Mutex<Option<Connection>>,
    session: Mutex<Option<Session>>,
    generation: Mutex<u64>,
    events: Arc<Mutex<Vec<SessionEvent>>>,
    token_file: PathBuf,
    logind: Option<crate::logind::LogindWatcher>,
}

type Results = HashMap<String, OwnedValue>;

fn err(e: impl std::fmt::Display) -> AdlError {
    AdlError::new(ErrorCode::Blocked, format!("portal: {e}"))
}

fn token() -> String {
    let mut b = [0u8; 8];
    // Not a secret; only needs to be unique per request.
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    b.copy_from_slice(&(t as u64 ^ std::process::id() as u64).to_le_bytes());
    format!("adl{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
}

impl PortalAdapter {
    pub fn new(token_file: PathBuf) -> Self {
        let logind = crate::logind::LogindWatcher::start().map_err(|e| eprintln!("w3bs-desktop: logind unavailable ({e}); lock/suspend rely on the native control")).ok();
        Self { conn: Mutex::new(None), session: Mutex::new(None), generation: Mutex::new(1), events: Arc::new(Mutex::new(vec![])), token_file, logind }
    }

    pub fn detect() -> bool {
        std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland" || t == "x11") && std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
    }

    fn conn(&self) -> Result<Connection> {
        let mut c = self.conn.lock().unwrap();
        if c.is_none() {
            *c = Some(Connection::session().map_err(err)?);
        }
        Ok(c.clone().unwrap())
    }

    /// Call a portal method that returns a Request handle and wait for its Response.
    fn request(&self, conn: &Connection, iface: &str, method: &str, build: impl FnOnce(&str) -> Result<OwnedObjectPath>) -> Result<Results> {
        let tok = token();
        let sender = conn.unique_name().map(|n| n.trim_start_matches(':').replace('.', "_")).ok_or_else(|| err("no unique bus name"))?;
        let req_path = format!("{PATH}/request/{sender}/{tok}");
        let req = Proxy::new(conn, DEST, req_path.clone(), "org.freedesktop.portal.Request").map_err(err)?;
        let signals = req.receive_signal("Response").map_err(err)?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            if let Some(msg) = signals.into_iter().next() {
                let _ = tx.send(msg.body().deserialize::<(u32, Results)>().map_err(|e| e.to_string()));
            }
        });
        build(&tok).map_err(|e| AdlError::new(e.code, format!("{iface}.{method}: {}", e.message)))?;
        match rx.recv_timeout(CONSENT_TIMEOUT) {
            Ok(Ok((0, results))) => Ok(results),
            Ok(Ok((1, _))) => Err(AdlError::new(ErrorCode::Denied, format!("{method} was cancelled in the portal dialog"))),
            Ok(Ok((code, _))) => Err(err(format!("{method} failed (response {code})"))),
            Ok(Err(e)) => Err(err(e)),
            Err(_) => {
                // Withdraw the request so its dialog does not linger: a late "Allow" would otherwise
                // grant a portal session the broker no longer tracks.
                if let Ok(p) = Proxy::new(conn, DEST, req_path, "org.freedesktop.portal.Request") {
                    let _ = p.call_method("Close", &());
                }
                Err(AdlError::new(ErrorCode::Blocked, format!("no answer to the {method} portal dialog")))
            }
        }
    }

    fn notify<B>(&self, method: &str, body: &B) -> Result<()>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        let conn = self.conn()?;
        let p = Proxy::new(&conn, DEST, PATH, "org.freedesktop.portal.RemoteDesktop").map_err(err)?;
        p.call_method(method, body).map(|_| ()).map_err(|e| AdlError::new(ErrorCode::OutcomeUnknown, format!("{method}: {e}")))
    }

    fn handle(&self) -> Result<OwnedObjectPath> {
        self.session.lock().unwrap().as_ref().map(|s| s.handle.clone()).ok_or_else(|| AdlError::new(ErrorCode::Blocked, "no portal session; enable first"))
    }

    fn stream_at(&self, x: f64, y: f64) -> Result<(u32, f64, f64)> {
        let s = self.session.lock().unwrap();
        let s = s.as_ref().ok_or_else(|| AdlError::new(ErrorCode::Blocked, "no portal session"))?;
        let st = s.streams.iter().find(|st| st.rect.contains(x, y)).ok_or_else(|| AdlError::invalid("point is not on a shared monitor"))?;
        Ok((st.node, x - st.rect.x, y - st.rect.y))
    }

    fn parse_streams(v: &OwnedValue) -> Vec<Stream> {
        let mut out = vec![];
        let Ok(Value::Array(arr)) = v.try_clone().map(Value::from) else { return out };
        for item in arr.iter() {
            let Value::Structure(s) = item else { continue };
            let f = s.fields();
            let (Some(Value::U32(node)), Some(Value::Dict(props))) = (f.first(), f.get(1)) else { continue };
            let Ok(props) = props.try_clone().map_err(|e| e.to_string()).and_then(|d| HashMap::<String, OwnedValue>::try_from(d).map_err(|e| e.to_string())) else { continue };
            let pair = |k: &str| -> Option<(i32, i32)> {
                let v = Value::from(props.get(k)?.try_clone().ok()?);
                if let Value::Structure(s) = v
                    && let [Value::I32(a), Value::I32(b)] = s.fields() {
                        return Some((*a, *b));
                    }
                None
            };
            let (x, y) = pair("position").unwrap_or((0, 0));
            let Some((w, h)) = pair("size") else { continue };
            out.push(Stream { node: *node, rect: Rect { x: x as f64, y: y as f64, width: w as f64, height: h as f64 } });
        }
        out
    }

    fn restore_token(&self) -> Option<String> {
        std::fs::read_to_string(&self.token_file).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }

    fn save_restore_token(&self, t: &str) {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(d) = self.token_file.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&self.token_file) {
            let _ = f.write_all(t.as_bytes());
        }
    }

    /// Whether the permission store already allows unsandboxed callers (app id "") to take
    /// non-interactive screenshots.
    fn screenshot_granted(conn: &Connection) -> bool {
        let Ok(store) = Proxy::new(conn, "org.freedesktop.impl.portal.PermissionStore", "/org/freedesktop/impl/portal/PermissionStore", "org.freedesktop.impl.portal.PermissionStore") else {
            return false;
        };
        let reply: zbus::Result<(HashMap<String, Vec<String>>, OwnedValue)> = store.call("Lookup", &("screenshot", "screenshot"));
        reply.is_ok_and(|(perms, _)| perms.get("").is_some_and(|p| p.iter().any(|v| v == "yes")))
    }

    fn keysym(&self, sym: u32, pressed: bool) -> Result<()> {
        let h = self.handle()?;
        self.notify("NotifyKeyboardKeysym", &(h, HashMap::<&str, Value>::new(), sym as i32, pressed as u32))
    }
}

fn crop(png_bytes: &[u8], bbox: Rect, d: &Rect) -> Result<(Vec<u8>, u32, u32)> {
    let dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
    let mut reader = dec.read_info().map_err(|e| AdlError::internal(e.to_string()))?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| AdlError::internal(e.to_string()))?;
    let bpp = info.line_size / info.width as usize;
    let sx = info.width as f64 / bbox.width;
    let sy = info.height as f64 / bbox.height;
    let x0 = (((d.x - bbox.x) * sx).round() as u32).min(info.width);
    let y0 = (((d.y - bbox.y) * sy).round() as u32).min(info.height);
    let w = ((d.width * sx).round() as u32).min(info.width - x0);
    let h = ((d.height * sy).round() as u32).min(info.height - y0);
    let mut out_px = Vec::with_capacity((w * h) as usize * bpp);
    for row in y0..y0 + h {
        let start = row as usize * info.line_size + x0 as usize * bpp;
        out_px.extend_from_slice(&buf[start..start + w as usize * bpp]);
    }
    let mut out = vec![];
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(info.color_type);
        enc.set_depth(info.bit_depth);
        let mut wr = enc.write_header().map_err(|e| AdlError::internal(e.to_string()))?;
        wr.write_image_data(&out_px).map_err(|e| AdlError::internal(e.to_string()))?;
    }
    Ok((out, w, h))
}

impl Adapter for PortalAdapter {
    fn name(&self) -> String {
        "linux-portal".into()
    }

    fn capabilities(&self) -> Vec<CapabilityReport> {
        let devices = self.session.lock().unwrap().as_ref().map(|s| s.devices);
        let input = |bit: u32, c: Capability, f: &[&str]| CapabilityReport {
            capability: c,
            availability: match devices {
                None => Availability::PermissionRequired,
                Some(d) if d & bit != 0 => Availability::Available,
                Some(_) => Availability::Unsupported,
            },
            backend: "linux-portal".into(),
            reason: if devices.is_none() { "granted in the RemoteDesktop portal dialog when access is enabled".into() } else { String::new() },
            features: f.iter().map(|s| s.to_string()).collect(),
        };
        let un = |c, r: &str| CapabilityReport { capability: c, availability: Availability::Unsupported, backend: "linux-portal".into(), reason: r.into(), features: BTreeSet::new() };
        vec![
            input(DEVICE_POINTER, Capability::Pointer, &["absolute_motion", "relative_motion", "button.back", "button.forward", "scroll.horizontal"]),
            input(DEVICE_KEYBOARD, Capability::Keyboard, &["keysym_text"]),
            CapabilityReport { capability: Capability::ScreenCapture, availability: Availability::Degraded, backend: "linux-portal".into(), reason: "Screenshot portal capture, cropped per monitor; the screenshot's pixel scale is chosen by the compositor".into(), features: BTreeSet::new() },
            un(Capability::Windows, "needs the KWin script (Plasma) or W3BS Shell extension (GNOME); not implemented yet, so targeted input is refused"),
            un(Capability::Accessibility, "AT-SPI adapter not implemented yet"),
        ]
    }

    fn begin(&self) -> Result<()> {
        let conn = self.conn()?;
        let rd = Proxy::new(&conn, DEST, PATH, "org.freedesktop.portal.RemoteDesktop").map_err(err)?;
        let sc = Proxy::new(&conn, DEST, PATH, "org.freedesktop.portal.ScreenCast").map_err(err)?;
        let session_token = token();
        let created = self.request(&conn, "RemoteDesktop", "CreateSession", |tok| {
            let opts: HashMap<&str, Value> = HashMap::from([("handle_token", Value::from(tok)), ("session_handle_token", Value::from(session_token.as_str()))]);
            rd.call("CreateSession", &(opts,)).map_err(err)
        })?;
        let handle: String = created.get("session_handle").and_then(|v| String::try_from(v.try_clone().ok()?).ok()).ok_or_else(|| err("no session_handle"))?;
        let handle = OwnedObjectPath::try_from(handle).map_err(err)?;
        // Everything after CreateSession runs in a closure so a failure (refusal, timeout, nothing
        // granted) closes the portal session instead of leaving it, or its dialog, behind.
        let negotiate = || -> Result<(u32, Vec<Stream>)> {
            let restore = self.restore_token();
            self.request(&conn, "RemoteDesktop", "SelectDevices", |tok| {
                let mut opts: HashMap<&str, Value> = HashMap::from([("handle_token", Value::from(tok)), ("types", Value::from(DEVICE_KEYBOARD | DEVICE_POINTER)), ("persist_mode", Value::from(PERSIST_UNTIL_REVOKED))]);
                if let Some(r) = restore.as_deref() {
                    opts.insert("restore_token", Value::from(r));
                }
                rd.call("SelectDevices", &(&handle, opts)).map_err(err)
            })?;
            self.request(&conn, "ScreenCast", "SelectSources", |tok| {
                let opts: HashMap<&str, Value> = HashMap::from([("handle_token", Value::from(tok)), ("types", Value::from(SOURCE_MONITOR)), ("multiple", Value::from(true))]);
                sc.call("SelectSources", &(&handle, opts)).map_err(err)
            })?;
            let started = self.request(&conn, "RemoteDesktop", "Start", |tok| {
                let opts: HashMap<&str, Value> = HashMap::from([("handle_token", Value::from(tok))]);
                rd.call("Start", &(&handle, "", opts)).map_err(err)
            })?;
            let devices = started.get("devices").and_then(|v| u32::try_from(v).ok()).unwrap_or(0);
            let streams = started.get("streams").map(Self::parse_streams).unwrap_or_default();
            if let Some(t) = started.get("restore_token").and_then(|v| String::try_from(v.try_clone().ok()?).ok()) {
                self.save_restore_token(&t);
            }
            if devices & DEVICE_POINTER == 0 && devices & DEVICE_KEYBOARD == 0 {
                return Err(AdlError::new(ErrorCode::Denied, "the portal granted no input devices"));
            }
            if streams.is_empty() {
                return Err(AdlError::new(ErrorCode::Blocked, "the portal shared no monitors, so absolute pointer positions are unavailable"));
            }
            Ok((devices, streams))
        };
        let (devices, streams) = match negotiate() {
            Ok(v) => v,
            Err(e) => {
                if let Ok(p) = Proxy::new(&conn, DEST, handle.clone(), "org.freedesktop.portal.Session") {
                    let _ = p.call_method("Close", &());
                }
                return Err(e);
            }
        };
        // The portal closes the session if the human revokes it from the desktop's own UI.
        {
            let (conn, ev, h) = (conn.clone(), self.events.clone(), handle.clone());
            std::thread::spawn(move || {
                let Ok(s) = Proxy::new(&conn, DEST, h, "org.freedesktop.portal.Session") else { return };
                let Ok(it) = s.receive_signal("Closed") else { return };
                if it.into_iter().next().is_some() {
                    ev.lock().unwrap().push(SessionEvent::PermissionLost("the RemoteDesktop portal session was closed".into()));
                }
            });
        }
        // If the portal frontend or its desktop backend restarts, the new process has no record of
        // this session and silently drops every Notify* call (seen on Plasma 6.7: "non-existing
        // session"), while the frontend still answers. Treat any owner change as revocation.
        {
            let (conn, ev) = (conn.clone(), self.events.clone());
            std::thread::spawn(move || {
                let Ok(bus) = Proxy::new(&conn, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus") else { return };
                let Ok(it) = bus.receive_signal("NameOwnerChanged") else { return };
                for msg in it {
                    let Ok((name, old, _new)) = msg.body().deserialize::<(String, String, String)>() else { continue };
                    if !old.is_empty() && (name == DEST || name.starts_with("org.freedesktop.impl.portal.desktop.")) {
                        ev.lock().unwrap().push(SessionEvent::AdapterFailed(format!("{name} restarted; its RemoteDesktop session is gone")));
                        return;
                    }
                }
            });
        }
        *self.generation.lock().unwrap() += 1;
        *self.session.lock().unwrap() = Some(Session { handle, devices, streams });
        Ok(())
    }

    fn end(&self) {
        if let Some(s) = self.session.lock().unwrap().take()
            && let Ok(conn) = self.conn()
                && let Ok(p) = Proxy::new(&conn, DEST, s.handle, "org.freedesktop.portal.Session") {
                    let _ = p.call_method("Close", &());
                }
    }

    fn layout(&self) -> Result<Layout> {
        let s = self.session.lock().unwrap();
        let s = s.as_ref().ok_or_else(|| AdlError::new(ErrorCode::Blocked, "no portal session; monitors are known only after enabling"))?;
        let displays = s
            .streams
            .iter()
            .enumerate()
            .map(|(i, st)| DisplayInfo { id: format!("stream-{}", st.node), name: format!("Shared monitor {}", i + 1), logical: st.rect, scale: 1.0, rotation: Rotation::Normal, primary: i == 0 })
            .collect();
        Ok(Layout { generation: *self.generation.lock().unwrap(), displays })
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        Err(AdlError::unsupported("window listing on this desktop needs the KWin script or GNOME extension"))
    }

    fn focused_window(&self) -> Result<Option<WindowInfo>> {
        Ok(None)
    }

    fn window_at(&self, _x: f64, _y: f64) -> Result<Option<WindowInfo>> {
        Ok(None)
    }

    fn inspect(&self, _window: &str) -> Result<AccessNode> {
        Err(AdlError::unsupported("AT-SPI adapter not implemented yet"))
    }

    fn capture(&self, display: &str) -> Result<(Vec<u8>, u32, u32)> {
        let layout = self.layout()?;
        let d = layout.displays.iter().find(|d| d.id == display).ok_or_else(|| AdlError::not_found(format!("no display {display}")))?;
        let conn = self.conn()?;
        // Without a stored grant the portal opens "Allow apps to take screenshots?", and while that
        // dialog is up every portal call blocks, including this session's input. Refuse instead.
        if !Self::screenshot_granted(&conn) {
            return Err(AdlError::new(
                ErrorCode::Blocked,
                "screenshots are not pre-authorized; run `flatpak permission-set screenshot screenshot \"\" yes` (or allow it in the desktop's privacy settings) and try again",
            )
            .with_details(serde_json::json!({"availability": "permission_required"})));
        }
        let shot = Proxy::new(&conn, DEST, PATH, "org.freedesktop.portal.Screenshot").map_err(err)?;
        let r = self.request(&conn, "Screenshot", "Screenshot", |tok| {
            let opts: HashMap<&str, Value> = HashMap::from([("handle_token", Value::from(tok)), ("interactive", Value::from(false))]);
            shot.call("Screenshot", &("", opts)).map_err(err)
        })?;
        let uri: String = r.get("uri").and_then(|v| String::try_from(v.try_clone().ok()?).ok()).ok_or_else(|| err("no screenshot uri"))?;
        let path = uri.strip_prefix("file://").ok_or_else(|| err("screenshot is not a local file"))?;
        let path = path.replace("%20", " ");
        let bytes = std::fs::read(&path).map_err(|e| AdlError::internal(format!("{path}: {e}")));
        let _ = std::fs::remove_file(&path);
        let bytes = bytes?;
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for d in &layout.displays {
            x0 = x0.min(d.logical.x);
            y0 = y0.min(d.logical.y);
            x1 = x1.max(d.logical.x + d.logical.width);
            y1 = y1.max(d.logical.y + d.logical.height);
        }
        crop(&bytes, Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 }, &d.logical)
    }

    fn emit(&self, event: &InputEvent) -> Result<()> {
        let h = self.handle()?;
        let none = HashMap::<&str, Value>::new;
        match event {
            InputEvent::MotionAbsolute { x, y } => {
                let (node, sx, sy) = self.stream_at(*x, *y)?;
                self.notify("NotifyPointerMotionAbsolute", &(h, none(), node, sx, sy))
            }
            InputEvent::MotionRelative { dx, dy } => self.notify("NotifyPointerMotion", &(h, none(), *dx, *dy)),
            InputEvent::Button { button, pressed } => self.notify("NotifyPointerButton", &(h, none(), keymap::button_code(*button) as i32, *pressed as u32)),
            InputEvent::Scroll { axis, steps } => self.notify("NotifyPointerAxisDiscrete", &(h, none(), matches!(axis, Axis::Horizontal) as u32, *steps)),
            InputEvent::Key { key, pressed } => match keymap::key_code(key) {
                Some(code) => self.notify("NotifyKeyboardKeycode", &(h, none(), code as i32, *pressed as u32)),
                None => {
                    let sym = keymap::keysym(key).ok_or_else(|| AdlError::unsupported(format!("{key} has no Linux equivalent")))?;
                    self.keysym(sym, *pressed)
                }
            },
            InputEvent::Text { text } => {
                for c in text.chars() {
                    let sym = keymap::char_keysym(c);
                    self.keysym(sym, true)?;
                    self.keysym(sym, false)?;
                }
                Ok(())
            }
        }
    }

    fn poll_events(&self) -> Vec<SessionEvent> {
        let mut out = std::mem::take(&mut *self.events.lock().unwrap());
        if let Some(l) = &self.logind {
            out.extend(l.drain());
        }
        out
    }
}
