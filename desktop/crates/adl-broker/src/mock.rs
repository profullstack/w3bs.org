//! A deterministic fake desktop for conformance tests and the headless demo.
//!
//! Three displays exercise the coordinate rules: a primary 1920x1080 at the origin, a HiDPI
//! (scale 2) monitor to the *left* at a negative origin, and a rotated portrait monitor with
//! fractional scaling. Windows have a z-order so occlusion can be tested, and every emitted
//! event is recorded.

use crate::adapter::{Adapter, InputEvent, PixelRect, SessionEvent, StreamPixels};
use adl_schema::*;
use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Default)]
struct State {
    generation: u64,
    displays: Vec<DisplayInfo>,
    /// Topmost first.
    windows: Vec<WindowInfo>,
    events: Vec<InputEvent>,
    pending: Vec<SessionEvent>,
    typed: std::collections::HashMap<String, String>,
    pointer: (f64, f64),
    pressed_buttons: BTreeSet<String>,
    pressed_keys: BTreeSet<String>,
    session_open: bool,
    streams: usize,
    emit_delay_ms: u64,
    fail_emit_after: Option<usize>,
    focus_steal_after: Option<(usize, String)>,
    begin_error: Option<String>,
    interlock: Option<String>,
    untypeable: String,
    /// A fake live capture stream: offered when `stream_offered`, running between start and stop.
    stream_offered: bool,
    stream_running: bool,
    stream_starts: usize,
    stream_seq: u64,
    /// Painted pixel rectangles on top of the uniform 0xe0 grey, per display.
    paint: Vec<(String, PixelRect, u8)>,
}

pub struct MockDesktop {
    s: Mutex<State>,
}

impl Default for MockDesktop {
    fn default() -> Self {
        Self::new()
    }
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect { x, y, width, height }
}

fn win(id: &str, app: &str, title: &str, r: Rect, display: &str, focused: bool) -> WindowInfo {
    WindowInfo { id: id.into(), app_id: app.into(), title: title.into(), bounds: Some(r), display: Some(display.into()), focused }
}

impl MockDesktop {
    pub fn new() -> Self {
        let displays = vec![
            DisplayInfo { id: "DP-1".into(), name: "Primary 24\"".into(), logical: rect(0.0, 0.0, 1920.0, 1080.0), scale: 1.0, rotation: Rotation::Normal, primary: true },
            DisplayInfo { id: "DP-2".into(), name: "HiDPI 13\" (left)".into(), logical: rect(-1280.0, 0.0, 1280.0, 800.0), scale: 2.0, rotation: Rotation::Normal, primary: false },
            DisplayInfo { id: "HDMI-1".into(), name: "Portrait (right)".into(), logical: rect(1920.0, -200.0, 864.0, 1536.0), scale: 1.25, rotation: Rotation::Rotate90, primary: false },
        ];
        let windows = vec![
            win("w-editor", "org.kde.kate", "notes.txt - Kate", rect(100.0, 100.0, 800.0, 600.0), "DP-1", true),
            win("w-browser", "org.mozilla.firefox", "Agent profile - Firefox", rect(600.0, 200.0, 1000.0, 800.0), "DP-1", false),
            win("w-terminal", "org.gnome.Console", "Console", rect(-1200.0, 50.0, 1000.0, 700.0), "DP-2", false),
        ];
        Self { s: Mutex::new(State { generation: 1, displays, windows, pointer: (960.0, 540.0), ..Default::default() }) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.s.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn events(&self) -> Vec<InputEvent> {
        self.lock().events.clone()
    }
    pub fn clear_events(&self) {
        self.lock().events.clear();
    }
    /// Text the mock "application" received, per window.
    pub fn typed(&self, window: &str) -> String {
        self.lock().typed.get(window).cloned().unwrap_or_default()
    }
    pub fn pressed(&self) -> (Vec<String>, Vec<String>) {
        let s = self.lock();
        (s.pressed_buttons.iter().cloned().collect(), s.pressed_keys.iter().cloned().collect())
    }
    pub fn session_open(&self) -> bool {
        self.lock().session_open
    }
    pub fn focus(&self, window: &str) {
        let mut s = self.lock();
        if let Some(i) = s.windows.iter().position(|w| w.id == window) {
            let w = s.windows.remove(i);
            s.windows.insert(0, w);
        }
        for w in &mut s.windows {
            w.focused = w.id == window;
        }
    }
    /// Give `window` keyboard focus without raising it (it stays under whatever is on top).
    pub fn set_focused_only(&self, window: &str) {
        for w in &mut self.lock().windows {
            w.focused = w.id == window;
        }
    }
    /// Steal focus to `window` after `n` more emitted events (simulates a popup).
    pub fn steal_focus_after(&self, n: usize, window: &str) {
        self.lock().focus_steal_after = Some((n, window.into()));
    }
    pub fn change_layout(&self) {
        let mut s = self.lock();
        s.generation += 1;
        s.displays.retain(|d| d.id != "HDMI-1");
        s.pending.push(SessionEvent::LayoutChanged);
    }
    pub fn set_emit_delay_ms(&self, ms: u64) {
        self.lock().emit_delay_ms = ms;
    }
    pub fn fail_emit_after(&self, n: usize) {
        self.lock().fail_emit_after = Some(n);
    }
    pub fn fail_begin(&self, reason: &str) {
        self.lock().begin_error = Some(reason.into());
    }
    pub fn push_event(&self, e: SessionEvent) {
        self.lock().pending.push(e);
    }
    pub fn set_interlock(&self, why: Option<&str>) {
        self.lock().interlock = why.map(Into::into);
    }
    /// Characters the "compositor" cannot type; `preflight` refuses text containing them, the
    /// way the GNOME backend refuses characters missing from the active layout.
    pub fn set_untypeable(&self, chars: &str) {
        self.lock().untypeable = chars.into();
    }
    pub fn open_stream(&self) {
        self.lock().streams += 1;
    }
    /// Offer a live frame stream to recordings (otherwise they use `capture`).
    pub fn offer_frame_stream(&self, on: bool) {
        self.lock().stream_offered = on;
    }
    /// (running now, times started).
    pub fn frame_stream_state(&self) -> (bool, usize) {
        let s = self.lock();
        (s.stream_running, s.stream_starts)
    }
    /// Paint a rectangle of one display (pixels) and, like a compositor, send a new frame.
    pub fn paint(&self, display: &str, r: PixelRect, value: u8) {
        let mut s = self.lock();
        s.paint.push((display.into(), r, value));
        s.stream_seq += 1;
    }
    /// A new frame with no change, like a compositor repainting the same pixels.
    pub fn repaint(&self) {
        self.lock().stream_seq += 1;
    }
}

fn button_name(b: Button) -> String {
    format!("{b:?}").to_lowercase()
}

impl Adapter for MockDesktop {
    fn name(&self) -> String {
        "mock".into()
    }

    fn capabilities(&self) -> Vec<CapabilityReport> {
        let feats = |f: &[&str]| f.iter().map(|s| s.to_string()).collect();
        let r = |c, features| CapabilityReport { capability: c, availability: Availability::Available, backend: "mock".into(), reason: String::new(), features };
        let mut out = vec![
            r(Capability::Pointer, feats(&["absolute_motion", "relative_motion", "pointer_position", "button.back", "button.forward", "scroll.horizontal"])),
            r(Capability::Keyboard, feats(&["text_insertion", "target_bound_keyboard", "physical_input_detection"])),
            r(Capability::ScreenCapture, BTreeSet::new()),
            r(Capability::Windows, feats(&["window_at", "focus"])),
            r(Capability::Accessibility, BTreeSet::new()),
        ];
        for c in [Capability::ClipboardRead, Capability::ClipboardWrite, Capability::Browser, Capability::Files, Capability::Email, Capability::Calendar] {
            out.push(CapabilityReport { capability: c, availability: Availability::Unsupported, backend: "mock".into(), reason: "not implemented by the mock adapter".into(), features: BTreeSet::new() });
        }
        out
    }

    fn begin(&self) -> Result<()> {
        let mut s = self.lock();
        if let Some(r) = s.begin_error.clone() {
            return Err(AdlError::new(ErrorCode::Blocked, r));
        }
        s.session_open = true;
        Ok(())
    }

    fn end(&self) {
        let mut s = self.lock();
        s.session_open = false;
        s.streams = 0;
    }

    fn close_streams(&self) -> usize {
        std::mem::take(&mut self.lock().streams)
    }

    fn layout(&self) -> Result<Layout> {
        let s = self.lock();
        Ok(Layout { generation: s.generation, displays: s.displays.clone() })
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        Ok(self.lock().windows.clone())
    }

    fn window_at(&self, x: f64, y: f64) -> Result<Option<WindowInfo>> {
        Ok(self.lock().windows.iter().find(|w| w.bounds.is_some_and(|b| b.contains(x, y))).cloned())
    }

    fn inspect(&self, window: &str) -> Result<AccessNode> {
        let s = self.lock();
        let w = s.windows.iter().find(|w| w.id == window).ok_or_else(|| AdlError::not_found(format!("no window {window}")))?;
        let b = w.bounds.unwrap();
        let node = |h: &str, role: &str, name: &str, r: Rect, actions: &[&str]| AccessNode {
            handle: format!("{window}/{h}"), role: role.into(), name: name.into(), value: None, bounds: Some(r),
            actions: actions.iter().map(|s| s.to_string()).collect(), states: vec![], children: vec![],
        };
        Ok(AccessNode {
            handle: format!("{window}/root"), role: "frame".into(), name: w.title.clone(), value: None, bounds: Some(b),
            actions: vec![], states: if w.focused { vec!["active".into()] } else { vec![] },
            children: vec![
                node("save", "push button", "Save", rect(b.x + 10.0, b.y + 10.0, 60.0, 24.0), &["press"]),
                AccessNode { value: s.typed.get(window).cloned(), ..node("text", "text", "Document", rect(b.x + 10.0, b.y + 50.0, b.width - 20.0, b.height - 60.0), &["focus"]) },
            ],
        })
    }

    fn capture(&self, display: &str) -> Result<(Vec<u8>, u32, u32)> {
        let d = self.lock().displays.iter().find(|d| d.id == display).cloned().ok_or_else(|| AdlError::not_found(format!("no display {display}")))?;
        let (w, h) = d.pixel_size();
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_compression(png::Compression::Fast);
            let mut wr = enc.write_header().map_err(|e| AdlError::internal(e.to_string()))?;
            // The painted rectangles show in captures as they do in stream frames.
            let mut px = vec![0xe0u8; (w * h) as usize];
            for (_, r, v) in self.lock().paint.iter().filter(|(id, _, _)| id == display) {
                for y in r.y.min(h)..(r.y + r.height).min(h) {
                    let row = (y * w) as usize;
                    px[row + r.x.min(w) as usize..row + (r.x + r.width).min(w) as usize].fill(*v);
                }
            }
            wr.write_image_data(&px).map_err(|e| AdlError::internal(e.to_string()))?;
        }
        Ok((out, w, h))
    }

    fn start_frame_stream(&self) -> bool {
        let mut s = self.lock();
        if s.stream_offered {
            s.stream_running = true;
            s.stream_starts += 1;
        }
        s.stream_running
    }

    fn stop_frame_stream(&self) {
        self.lock().stream_running = false;
    }

    fn stream_pixels(&self, display: &str, crop: Option<PixelRect>) -> Option<StreamPixels> {
        let s = self.lock();
        if !s.stream_running {
            return None;
        }
        let d = s.displays.iter().find(|d| d.id == display)?;
        let (w, h) = d.pixel_size();
        let crop = crop.map(|c| {
            let (x, y) = (c.x.min(w - 1), c.y.min(h - 1));
            PixelRect { x, y, width: c.width.min(w - x), height: c.height.min(h - y) }
        });
        let mut rgba = Vec::new();
        if let Some(c) = crop {
            rgba.reserve((c.width * c.height * 4) as usize);
            for py in c.y..c.y + c.height {
                for px in c.x..c.x + c.width {
                    let v = s
                        .paint
                        .iter()
                        .rev()
                        .find(|(id, r, _)| id == display && px >= r.x && px < r.x + r.width && py >= r.y && py < r.y + r.height)
                        .map_or(0xe0, |(_, _, v)| *v);
                    rgba.extend_from_slice(&[v, v, v, 0xff]);
                }
            }
        }
        Some(StreamPixels { seq: s.stream_seq, width: w, height: h, rgba, crop })
    }

    fn pointer_position(&self) -> Result<Option<(f64, f64)>> {
        Ok(Some(self.lock().pointer))
    }

    fn emit(&self, event: &InputEvent) -> Result<()> {
        let delay = self.lock().emit_delay_ms;
        if delay > 0 {
            std::thread::sleep(Duration::from_millis(delay));
        }
        let mut s = self.lock();
        if let Some(n) = s.fail_emit_after {
            if n == 0 {
                s.fail_emit_after = None;
                return Err(AdlError::new(ErrorCode::OutcomeUnknown, "mock injection failure"));
            }
            s.fail_emit_after = Some(n - 1);
        }
        match event {
            InputEvent::MotionAbsolute { x, y } => s.pointer = (*x, *y),
            InputEvent::MotionRelative { dx, dy } => s.pointer = (s.pointer.0 + dx, s.pointer.1 + dy),
            InputEvent::Button { button, pressed } => {
                if *pressed {
                    s.pressed_buttons.insert(button_name(*button));
                } else {
                    s.pressed_buttons.remove(&button_name(*button));
                }
            }
            InputEvent::Key { key, pressed } => {
                if *pressed {
                    s.pressed_keys.insert(key.to_string());
                } else {
                    s.pressed_keys.remove(&key.to_string());
                }
            }
            InputEvent::Text { text } => {
                if let Some(f) = s.windows.iter().find(|w| w.focused).map(|w| w.id.clone()) {
                    s.typed.entry(f).or_default().push_str(text);
                }
            }
            InputEvent::Scroll { .. } => {}
        }
        s.events.push(event.clone());
        if let Some((n, w)) = s.focus_steal_after.clone() {
            if n <= 1 {
                s.focus_steal_after = None;
                drop(s);
                self.focus(&w);
            } else {
                s.focus_steal_after = Some((n - 1, w));
            }
        }
        Ok(())
    }

    fn preflight(&self, action: &Action) -> Result<()> {
        let s = self.lock();
        if let Action::Type { text } = action {
            let bad: String = text.chars().filter(|c| s.untypeable.contains(*c)).collect();
            if !bad.is_empty() {
                return Err(AdlError::unsupported(format!("cannot type {bad}; nothing was typed")));
            }
        }
        Ok(())
    }

    fn safety_interlock(&self) -> Option<String> {
        self.lock().interlock.clone()
    }

    fn poll_events(&self) -> Vec<SessionEvent> {
        std::mem::take(&mut self.lock().pending)
    }
}
