//! Vela adapter: observation and safety interlocks through Vela's compositor command socket.
//!
//! Clean-room implementation. It was written from a functional description of Vela's public
//! interfaces (socket path, line framing, query names and JSON reply keys); no Vela source was
//! read by the author, and none is included. Vela is GPL-3.0; this file is Apache-2.0.
//!
//! What Vela offers and what it does not (as of upstream 06eeaf8):
//! - `state` gives outputs (logical boxes, scale), windows (geometry, output, focus, MRU order),
//!   lock state and layer surfaces. There are no push events, so the adapter polls.
//! - `window-rects` gives visible windows topmost first, which is what occlusion checks need.
//! - Pointer and keyboard injection exist only through the wlroots virtual-input protocols, which
//!   Vela advertises only when started with `VELA_DEBUG_INPUT=1`. This adapter never sets that.
//!   Without it, input is reported unsupported rather than faked.
//! - Polkit prompts appear as an overlay layer named `vela-polkit` with exclusive keyboard. Input
//!   is refused while one is mapped, so synthetic keys can never reach a password field.
//! - Window capture sources are revoked while locked; this adapter disables on lock anyway.

use adl_broker::{Adapter, InputEvent, SessionEvent};
use adl_schema::*;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct VelaOutput {
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default = "one")]
    pub scale: f64,
    #[serde(default = "yes")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VelaWindow {
    pub id: String,
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub title: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub minimized: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VelaLayer {
    #[serde(default)]
    pub namespace: String,
    #[serde(default)]
    pub layer: u8,
    #[serde(default)]
    pub keyboard: u8,
    #[serde(default)]
    pub mapped: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VelaState {
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub focused: Option<String>,
    #[serde(default)]
    pub outputs: Vec<VelaOutput>,
    #[serde(default)]
    pub windows: Vec<VelaWindow>,
    #[serde(default)]
    pub layers: Vec<VelaLayer>,
    #[serde(default, rename = "focusedLayer")]
    pub focused_layer: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Rect4 {
    id: String,
    #[serde(default)]
    title: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

fn one() -> f64 {
    1.0
}
fn yes() -> bool {
    true
}

/// Layer namespaces Vela uses for its authentication prompt.
const POLKIT_LAYER: &str = "vela-polkit";
/// wlr-layer-shell overlay layer and exclusive keyboard interactivity.
const LAYER_OVERLAY: u8 = 3;
const KEYBOARD_EXCLUSIVE: u8 = 1;

impl VelaState {
    /// Why input must not be delivered now, if anything.
    pub fn interlock(&self) -> Option<String> {
        if self.locked {
            return Some("the session is locked".into());
        }
        if self.focused_layer.as_deref().is_some_and(|l| l.starts_with(POLKIT_LAYER)) || self.layers.iter().any(|l| l.mapped && l.namespace.starts_with(POLKIT_LAYER)) {
            return Some("an authentication prompt is on screen; the human must answer it".into());
        }
        if let Some(l) = self.layers.iter().find(|l| l.mapped && l.layer == LAYER_OVERLAY && l.keyboard == KEYBOARD_EXCLUSIVE) {
            return Some(format!("a modal overlay ({}) holds the keyboard", l.namespace));
        }
        None
    }

    fn displays(&self) -> Vec<DisplayInfo> {
        self.outputs
            .iter()
            .filter(|o| o.enabled)
            .enumerate()
            .map(|(i, o)| DisplayInfo {
                id: o.name.clone(),
                name: o.name.clone(),
                logical: Rect { x: o.x, y: o.y, width: o.w, height: o.h },
                scale: o.scale,
                // The command socket reports the rotated logical box but not the transform.
                rotation: Rotation::Normal,
                primary: i == 0,
            })
            .collect()
    }
}

struct Cache {
    state: Option<(Instant, VelaState)>,
    layout_sig: Vec<VelaOutput>,
    generation: u64,
    was_locked: bool,
    began: bool,
    pending: Vec<SessionEvent>,
}

pub struct VelaAdapter {
    socket: PathBuf,
    screen_dump: String,
    cache: Mutex<Cache>,
}

/// State older than this is re-queried. Interlocks always re-query.
const STATE_TTL: Duration = Duration::from_millis(100);

impl VelaAdapter {
    /// `$XDG_RUNTIME_DIR/vela-$WAYLAND_DISPLAY.sock`.
    pub fn default_socket() -> Option<PathBuf> {
        let rt = std::env::var_os("XDG_RUNTIME_DIR")?;
        let wd = std::env::var("WAYLAND_DISPLAY").ok()?;
        Some(PathBuf::from(rt).join(format!("vela-{wd}.sock")))
    }

    /// Vela lists itself first in `XDG_CURRENT_DESKTOP` (`Vela:KDE`); match it before KDE.
    pub fn detect() -> bool {
        let first = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        first.split(':').next() == Some("Vela") || Self::default_socket().is_some_and(|p| p.exists())
    }

    pub fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            screen_dump: "vela-screen-dump".into(),
            cache: Mutex::new(Cache { state: None, layout_sig: vec![], generation: 1, was_locked: false, began: false, pending: vec![] }),
        }
    }

    pub fn with_screen_dump(mut self, cmd: impl Into<String>) -> Self {
        self.screen_dump = cmd.into();
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Send one query line and read its one-line reply.
    pub fn query(&self, line: &str) -> Result<String> {
        let unreachable = |e: std::io::Error| AdlError::new(ErrorCode::Blocked, format!("Vela command socket {}: {e}", self.socket.display()));
        let mut s = UnixStream::connect(&self.socket).map_err(unreachable)?;
        s.set_read_timeout(Some(Duration::from_secs(2))).map_err(unreachable)?;
        s.write_all(format!("{line}\n").as_bytes()).map_err(unreachable)?;
        let mut reply = String::new();
        BufReader::new(s).read_line(&mut reply).map_err(unreachable)?;
        if reply.is_empty() {
            return Err(AdlError::new(ErrorCode::Blocked, format!("Vela did not answer `{line}`")));
        }
        Ok(reply)
    }

    /// Fire-and-forget command (Vela sends no reply and ignores unknown lines; verify via state).
    pub fn command(&self, line: &str) -> Result<()> {
        let mut s = UnixStream::connect(&self.socket).map_err(|e| AdlError::new(ErrorCode::Blocked, format!("Vela command socket: {e}")))?;
        s.write_all(format!("{line}\n").as_bytes()).map_err(|e| AdlError::internal(e.to_string()))
    }

    fn fetch(&self) -> Result<VelaState> {
        let reply = self.query("state")?;
        let st: VelaState = serde_json::from_str(&reply).map_err(|e| AdlError::internal(format!("unexpected Vela state reply: {e}")))?;
        let mut c = self.lock();
        let sig: Vec<VelaOutput> = st.outputs.clone();
        if c.layout_sig != sig {
            if !c.layout_sig.is_empty() {
                c.generation += 1;
                c.pending.push(SessionEvent::LayoutChanged);
            }
            c.layout_sig = sig;
        }
        if st.locked && !c.was_locked {
            c.pending.push(SessionEvent::Locked);
        }
        if !st.locked && c.was_locked {
            c.pending.push(SessionEvent::Unlocked);
        }
        c.was_locked = st.locked;
        c.state = Some((Instant::now(), st.clone()));
        Ok(st)
    }

    fn state(&self) -> Result<VelaState> {
        if let Some((t, s)) = &self.lock().state
            && t.elapsed() < STATE_TTL {
                return Ok(s.clone());
            }
        self.fetch()
    }

    /// Raise and focus a window by its stable identifier.
    pub fn activate(&self, window: &str) -> Result<()> {
        if window.chars().any(char::is_whitespace) {
            return Err(AdlError::invalid("window identifiers never contain whitespace"));
        }
        self.command(&format!("window {window} activate"))
    }
}

fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || &png[1..4] != b"PNG" || &png[12..16] != b"IHDR" {
        return None;
    }
    Some((u32::from_be_bytes(png[16..20].try_into().ok()?), u32::from_be_bytes(png[20..24].try_into().ok()?)))
}

impl Adapter for VelaAdapter {
    fn name(&self) -> String {
        "linux-vela".into()
    }

    fn capabilities(&self) -> Vec<CapabilityReport> {
        let rep = |c, a, reason: &str, f: &[&str]| CapabilityReport {
            capability: c, availability: a, backend: "linux-vela".into(), reason: reason.into(), features: f.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(),
        };
        let no_input = "Vela exposes pointer and keyboard injection only when its compositor runs with VELA_DEBUG_INPUT=1 (a debug switch); the wlroots virtual-input backend for that mode is not implemented yet";
        vec![
            rep(Capability::Windows, Availability::Available, "", &["window_at", "focus", "geometry", "activate"]),
            rep(Capability::ScreenCapture, Availability::Degraded, "captures through vela-screen-dump; the magnifier, if on, is visible in captures", &[]),
            rep(Capability::Pointer, Availability::Unsupported, no_input, &[]),
            rep(Capability::Keyboard, Availability::Unsupported, no_input, &[]),
            rep(Capability::Accessibility, Availability::Unsupported, "Vela has no AT-SPI bridge; the shared AT-SPI adapter will read apps' own trees", &[]),
            rep(Capability::ClipboardRead, Availability::Unsupported, "not implemented", &[]),
            rep(Capability::ClipboardWrite, Availability::Unsupported, "not implemented", &[]),
        ]
    }

    fn begin(&self) -> Result<()> {
        let st = self.fetch()?;
        if st.outputs.iter().all(|o| !o.enabled) {
            return Err(AdlError::new(ErrorCode::Blocked, "Vela reports no enabled outputs"));
        }
        self.lock().began = true;
        Ok(())
    }

    fn end(&self) {
        let mut c = self.lock();
        c.began = false;
        c.state = None;
    }

    fn layout(&self) -> Result<Layout> {
        let st = self.state()?;
        Ok(Layout { generation: self.lock().generation, displays: st.displays() })
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        let st = self.state()?;
        Ok(st
            .windows
            .iter()
            .map(|w| WindowInfo {
                id: w.id.clone(),
                app_id: w.app.clone(),
                title: w.title.clone(),
                bounds: (!w.minimized).then_some(Rect { x: w.x, y: w.y, width: w.w, height: w.h }),
                display: w.output.clone(),
                focused: st.focused.as_deref() == Some(&w.id),
            })
            .collect())
    }

    fn focused_window(&self) -> Result<Option<WindowInfo>> {
        // Always fresh: focus checks guard keyboard delivery.
        let st = self.fetch()?;
        let focused = st.focused.clone();
        drop(st);
        Ok(self.windows()?.into_iter().find(|w| Some(&w.id) == focused.as_ref()))
    }

    fn window_at(&self, x: f64, y: f64) -> Result<Option<WindowInfo>> {
        let reply = self.query("window-rects")?;
        let rects: Vec<Rect4> = serde_json::from_str(&reply).map_err(|e| AdlError::internal(format!("unexpected window-rects reply: {e}")))?;
        let Some(hit) = rects.into_iter().find(|r| Rect { x: r.x, y: r.y, width: r.w, height: r.h }.contains(x, y)) else { return Ok(None) };
        let full = self.windows()?.into_iter().find(|w| w.id == hit.id);
        Ok(Some(full.unwrap_or(WindowInfo { id: hit.id, app_id: String::new(), title: hit.title, bounds: Some(Rect { x: hit.x, y: hit.y, width: hit.w, height: hit.h }), display: None, focused: false })))
    }

    fn inspect(&self, _window: &str) -> Result<AccessNode> {
        Err(AdlError::unsupported("Vela has no accessibility bridge; use the AT-SPI adapter"))
    }

    fn capture(&self, display: &str) -> Result<(Vec<u8>, u32, u32)> {
        let st = self.state()?;
        let index = st.outputs.iter().position(|o| o.name == display).ok_or_else(|| AdlError::not_found(format!("no output {display}")))?;
        let dir = std::env::temp_dir().join(format!("adl-vela-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| AdlError::internal(e.to_string()))?;
        let file = dir.join("capture.png");
        let out = std::process::Command::new(&self.screen_dump)
            .arg(&file)
            .env("VELA_SCREEN_DUMP_OUTPUT", index.to_string())
            .output()
            .map_err(|e| AdlError::unsupported(format!("{}: {e}", self.screen_dump)))?;
        let png = std::fs::read(&file);
        let _ = std::fs::remove_dir_all(&dir);
        if !out.status.success() {
            return Err(AdlError::internal(format!("{} failed: {}", self.screen_dump, String::from_utf8_lossy(&out.stderr).trim())));
        }
        let png = png.map_err(|e| AdlError::internal(e.to_string()))?;
        let (w, h) = png_size(&png).ok_or_else(|| AdlError::internal("capture is not a PNG"))?;
        Ok((png, w, h))
    }

    fn emit(&self, _event: &InputEvent) -> Result<()> {
        Err(AdlError::unsupported("Vela input injection needs VELA_DEBUG_INPUT=1 and the wlroots virtual-input backend"))
    }

    fn safety_interlock(&self) -> Option<String> {
        match self.fetch() {
            Ok(st) => st.interlock(),
            Err(e) => Some(format!("cannot read Vela state: {}", e.message)),
        }
    }

    fn poll_events(&self) -> Vec<SessionEvent> {
        let began = self.lock().began;
        if began
            && let Err(e) = self.fetch() {
                // The compositor restarted or went away: every lease is void (spec: a restart
                // drops all client connections and recreates the socket).
                self.lock().pending.push(SessionEvent::AdapterFailed(e.message));
            }
        std::mem::take(&mut self.lock().pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_header() {
        let mut p = vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10, 0, 0, 0, 13];
        p.extend_from_slice(b"IHDR");
        p.extend_from_slice(&2560u32.to_be_bytes());
        p.extend_from_slice(&1600u32.to_be_bytes());
        assert_eq!(png_size(&p), Some((2560, 1600)));
    }
}
