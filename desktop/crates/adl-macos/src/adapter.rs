//! `MacAdapter`: the macOS backend.
//!
//! - Coordinates: desktop logical = Quartz global display points (origin at the top-left of the
//!   main display, y down; other displays may sit at negative origins). Scale is the display
//!   mode's pixel width over its point width. Rotation comes from `CGDisplayRotation`.
//! - Windows: `CGWindowListCopyWindowInfo` (on-screen, desktop elements excluded), front to back.
//!   Normal windows are layer 0. Every layer (menu bar, status items, overlays) counts as an
//!   occluder in `window_at`, except the Dock's transparent whole-display window, where only the
//!   Dock bar (from Accessibility) counts. Titles need Screen Recording; without it they are empty.
//! - Focus: the focused application comes from Accessibility when the process is trusted
//!   (`AXFocusedApplication` / `AXFocusedWindow`), otherwise from the front-most normal window
//!   (NSWorkspace's frontmost application is not updated in a process without a run loop).
//! - Input: Core Graphics events from a private event source tagged with `SOURCE_TAG`, posted at
//!   the HID tap. Text is Unicode strings on key events, never key positions.
//! - Session: polled (`CGSessionCopyCurrentDictionary`), no run loop or event tap needed. Physical
//!   input is detected from the HID system state's idle time (our own posted events reset it too,
//!   so only events clearly after our last injection count); no keys are ever read.

use crate::cf::{self, Owned};
use crate::ffi::*;
use crate::{keymap, permissions};
use adl_broker::{Adapter, InputEvent, SessionEvent};
use adl_schema::*;
use std::collections::{BTreeSet, HashMap};
use std::ffi::c_void;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// Stamped into `kCGEventSourceUserData` of every synthetic event ("w3bsADL\0").
pub const SOURCE_TAG: i64 = 0x7733_6273_4144_4C00;
const BACKEND: &str = "macos";
const MAX_AX_NODES: usize = 400;
const MAX_AX_DEPTH: usize = 16;
const MAX_VALUE_CHARS: usize = 4096;
const DOUBLE_CLICK: Duration = Duration::from_millis(500);
/// Pause after each synthetic text character, so applications keep up and a stop lands quickly.
const TEXT_PACE: Duration = Duration::from_millis(2);
/// Owners of windows that mean an OS authentication or permission prompt is on screen.
const PROMPT_OWNERS: &[&str] = &["SecurityAgent", "coreautha", "universalAccessAuthWarn", "loginwindow"];

struct EventSource(CGEventSourceRef);
// SAFETY: a CGEventSource is a CF object; it is only used to create events, which is
// thread-safe, and all input goes through the `input` mutex anyway.
unsafe impl Send for EventSource {}
unsafe impl Sync for EventSource {}
impl Drop for EventSource {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: we own the source.
            unsafe { CFRelease(self.0 as CFTypeRef) }
        }
    }
}

#[derive(Default)]
struct InputState {
    pointer: Option<(f64, f64)>,
    /// Core Graphics button numbers we are holding.
    buttons: BTreeSet<u32>,
    /// Modifier flags we are holding.
    flags: u64,
    /// (button, time, position, click count) of the last press, for click counting.
    last_press: Option<(u32, Instant, (f64, f64), i64)>,
    last_inject: Option<Instant>,
}

struct Watch {
    generation: u64,
    layout_sig: String,
    locked: Option<bool>,
    last_poll: Instant,
    last_wall: SystemTime,
    polls: u64,
    /// Permissions seen when the human enabled, so a later revocation can be reported.
    active: bool,
    post_at_begin: bool,
    trusted_at_begin: bool,
    pending: Vec<SessionEvent>,
}

/// One entry of the on-screen window list.
#[derive(Clone, Debug, PartialEq)]
pub struct RawWindow {
    pub id: u32,
    pub layer: i64,
    pub pid: i32,
    pub owner: String,
    pub title: String,
    pub bounds: Rect,
    pub alpha: f64,
}

impl RawWindow {
    fn normal(&self) -> bool {
        self.layer == 0 && self.alpha > 0.0 && self.bounds.width >= 2.0 && self.bounds.height >= 2.0
    }
}

pub struct MacAdapter {
    src: EventSource,
    input: Mutex<InputState>,
    watch: Mutex<Watch>,
    apps: Mutex<HashMap<i32, (String, String)>>,
    /// Visible Dock bar rectangles (from Accessibility), cached briefly: (when, Dock pid, rects).
    dock: Mutex<Option<(Instant, i32, Vec<Rect>)>>,
}

impl Default for MacAdapter {
    fn default() -> Self {
        Self::new()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn rect(r: CGRect) -> Rect {
    Rect { x: r.origin.x, y: r.origin.y, width: r.size.width, height: r.size.height }
}

/// Active displays, main display first (as `CGGetActiveDisplayList` returns them).
pub fn displays() -> Vec<DisplayInfo> {
    let mut ids = [0u32; 32];
    let mut n = 0u32;
    // SAFETY: buffer of 32 ids; n receives the count.
    if unsafe { CGGetActiveDisplayList(32, ids.as_mut_ptr(), &mut n) } != 0 {
        return vec![];
    }
    // SAFETY: plain queries on display ids the system just returned.
    let main = unsafe { CGMainDisplayID() };
    ids[..n as usize]
        .iter()
        .map(|&d| unsafe {
            let bounds = rect(CGDisplayBounds(d));
            let mode = CGDisplayCopyDisplayMode(d);
            let scale = if mode.is_null() {
                1.0
            } else {
                let (pw, w) = (CGDisplayModeGetPixelWidth(mode), CGDisplayModeGetWidth(mode));
                CGDisplayModeRelease(mode);
                if w > 0 { pw as f64 / w as f64 } else { 1.0 }
            };
            let rotation = match CGDisplayRotation(d).round() as i64 {
                90 => Rotation::Rotate90,
                180 => Rotation::Rotate180,
                270 => Rotation::Rotate270,
                _ => Rotation::Normal,
            };
            let builtin = CGDisplayIsBuiltin(d) != 0;
            DisplayInfo { id: d.to_string(), name: format!("Display {d}{}", if builtin { " (built-in)" } else { "" }), logical: bounds, scale, rotation, primary: d == main }
        })
        .collect()
}

fn layout_signature(ds: &[DisplayInfo]) -> String {
    ds.iter().map(|d| format!("{}:{}:{}:{}:{}:{}:{:?}", d.id, d.logical.x, d.logical.y, d.logical.width, d.logical.height, d.scale, d.rotation)).collect::<Vec<_>>().join("|")
}

/// The on-screen window list, front to back.
pub fn window_list() -> Vec<RawWindow> {
    // SAFETY: returns a +1 CFArray of CFDictionaries (or null).
    let Some(arr) = Owned::from_create(unsafe { CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, kCGNullWindowID) }) else {
        return vec![];
    };
    let mut out = vec![];
    for d in cf::array_items(arr.as_ptr()) {
        // SAFETY: the kCGWindow* keys are CFString constants exported by CoreGraphics.
        let (num, layer, pid, owner, name, alpha, bounds) = unsafe {
            (
                cf::dict_get(d, kCGWindowNumber),
                cf::dict_get(d, kCGWindowLayer),
                cf::dict_get(d, kCGWindowOwnerPID),
                cf::dict_get(d, kCGWindowOwnerName),
                cf::dict_get(d, kCGWindowName),
                cf::dict_get(d, kCGWindowAlpha),
                cf::dict_get(d, kCGWindowBounds),
            )
        };
        let Some(id) = cf::to_i64(num) else { continue };
        let mut r = CGRect::default();
        // SAFETY: bounds is a CFDictionary representation of a CGRect, or null (checked).
        if bounds.is_null() || !unsafe { CGRectMakeWithDictionaryRepresentation(bounds, &mut r) } {
            continue;
        }
        out.push(RawWindow {
            id: id as u32,
            layer: cf::to_i64(layer).unwrap_or(0),
            pid: cf::to_i64(pid).unwrap_or(0) as i32,
            owner: cf::to_string(owner).unwrap_or_default(),
            title: cf::to_string(name).unwrap_or_default(),
            bounds: rect(r),
            alpha: cf::to_f64(alpha).unwrap_or(1.0),
        });
    }
    out
}

/// `CGSessionCopyCurrentDictionary`: (screen locked, on console).
pub fn session_state() -> (bool, bool) {
    // SAFETY: returns a +1 dictionary or null.
    let Some(d) = Owned::from_create(unsafe { CGSessionCopyCurrentDictionary() }) else {
        // No window server session at all: treat as not on console.
        return (false, false);
    };
    let locked = cf::to_bool(cf::dict_get_str(d.as_ptr(), "CGSSessionScreenIsLocked")).unwrap_or(false);
    let on_console = cf::to_bool(cf::dict_get_str(d.as_ptr(), "kCGSSessionOnConsoleKey")).unwrap_or(true);
    (locked, on_console)
}

fn display_of(ds: &[DisplayInfo], r: &Rect) -> Option<String> {
    let (cx, cy) = (r.x + r.width / 2.0, r.y + r.height / 2.0);
    ds.iter().find(|d| d.logical.contains(cx, cy)).or_else(|| ds.iter().find(|d| d.logical.contains(r.x, r.y))).map(|d| d.id.clone())
}

// ------------------------------------------------------------------ Accessibility helpers

fn ax_attr(el: AXUIElementRef, name: &str) -> Option<Owned> {
    let attr = cf::string(name);
    let mut v: CFTypeRef = std::ptr::null();
    // SAFETY: el is a valid AXUIElement; v receives a +1 value on success.
    let err = unsafe { AXUIElementCopyAttributeValue(el, attr.as_ptr(), &mut v) };
    if err == kAXErrorSuccess { Owned::from_create(v) } else { None }
}

fn ax_string(el: AXUIElementRef, name: &str) -> Option<String> {
    ax_attr(el, name).and_then(|v| cf::to_string(v.as_ptr())).filter(|s| !s.is_empty())
}

fn ax_bool(el: AXUIElementRef, name: &str) -> Option<bool> {
    ax_attr(el, name).and_then(|v| cf::to_bool(v.as_ptr()))
}

fn ax_value_struct<T: Default>(el: AXUIElementRef, name: &str, kind: u32) -> Option<T> {
    let v = ax_attr(el, name)?;
    // SAFETY: type-checked AXValue; out has the layout the kind names.
    unsafe {
        if CFGetTypeID(v.as_ptr()) != AXValueGetTypeID() {
            return None;
        }
        let mut out = T::default();
        (AXValueGetValue(v.as_ptr(), kind, (&mut out as *mut T).cast::<c_void>()) != 0).then_some(out)
    }
}

fn ax_bounds(el: AXUIElementRef) -> Option<Rect> {
    let p: CGPoint = ax_value_struct(el, "AXPosition", kAXValueCGPointType)?;
    let s: CGSize = ax_value_struct(el, "AXSize", kAXValueCGSizeType)?;
    Some(Rect { x: p.x, y: p.y, width: s.width, height: s.height })
}

fn ax_window_id(el: AXUIElementRef) -> Option<u32> {
    type GetWindow = unsafe extern "C" fn(AXUIElementRef, *mut CGWindowID) -> AXError;
    static SYM: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    let f = (*SYM.get_or_init(|| optional_symbol(c"_AXUIElementGetWindow").map(|p| p as usize)))?;
    // SAFETY: the symbol has this signature on every macOS release that exports it.
    let get: GetWindow = unsafe { std::mem::transmute::<usize, GetWindow>(f) };
    let mut id: CGWindowID = 0;
    (unsafe { get(el, &mut id) } == kAXErrorSuccess && id != 0).then_some(id)
}

fn ax_value_text(el: AXUIElementRef) -> Option<String> {
    let v = ax_attr(el, "AXValue")?;
    let s = cf::to_string(v.as_ptr()).or_else(|| cf::to_f64(v.as_ptr()).map(|n| if n.fract() == 0.0 { format!("{}", n as i64) } else { n.to_string() }))?;
    Some(s.chars().take(MAX_VALUE_CHARS).collect())
}

fn ax_actions(el: AXUIElementRef) -> Vec<String> {
    let mut names: CFArrayRef = std::ptr::null();
    // SAFETY: names receives a +1 array on success.
    if unsafe { AXUIElementCopyActionNames(el, &mut names) } != kAXErrorSuccess {
        return vec![];
    }
    let Some(arr) = Owned::from_create(names) else { return vec![] };
    cf::array_items(arr.as_ptr()).into_iter().filter_map(cf::to_string).map(|a| a.strip_prefix("AX").unwrap_or(&a).to_ascii_lowercase()).collect()
}

fn ax_node(el: AXUIElementRef, handle: String, depth: usize, budget: &mut usize, deadline: Instant) -> AccessNode {
    let role = ax_string(el, "AXRole").unwrap_or_else(|| "AXUnknown".into());
    let name = ax_string(el, "AXTitle").or_else(|| ax_string(el, "AXDescription")).unwrap_or_default();
    let mut states = vec![];
    if ax_bool(el, "AXFocused") == Some(true) {
        states.push("focused".to_string());
    }
    if ax_bool(el, "AXEnabled") == Some(false) {
        states.push("disabled".to_string());
    }
    if ax_bool(el, "AXSelected") == Some(true) {
        states.push("selected".to_string());
    }
    let mut node = AccessNode { handle: handle.clone(), role, name, value: ax_value_text(el), bounds: ax_bounds(el), actions: ax_actions(el), states, children: vec![] };
    if depth >= MAX_AX_DEPTH {
        return node;
    }
    if let Some(kids) = ax_attr(el, "AXChildren") {
        for (i, k) in cf::array_items(kids.as_ptr()).into_iter().enumerate() {
            if *budget == 0 || Instant::now() > deadline {
                node.states.push("truncated".into());
                break;
            }
            *budget -= 1;
            node.children.push(ax_node(k, format!("{handle}.{i}"), depth + 1, budget, deadline));
        }
    }
    node
}

/// Accessibility element of the focused application (needs the process to be trusted).
fn ax_focused_app_pid() -> Option<i32> {
    // SAFETY: the system-wide element is a +1 object.
    let sys = Owned::from_create(unsafe { AXUIElementCreateSystemWide() })?;
    let app = ax_attr(sys.as_ptr(), "AXFocusedApplication")?;
    let mut pid = 0;
    // SAFETY: app is an AXUIElement.
    (unsafe { AXUIElementGetPid(app.as_ptr(), &mut pid) } == kAXErrorSuccess && pid > 0).then_some(pid)
}

fn ax_focused_window_of(pid: i32) -> Option<u32> {
    // SAFETY: +1 application element.
    let app = Owned::from_create(unsafe { AXUIElementCreateApplication(pid) })?;
    let w = ax_attr(app.as_ptr(), "AXFocusedWindow")?;
    ax_window_id(w.as_ptr())
}

// ------------------------------------------------------------------ the adapter

impl MacAdapter {
    pub fn new() -> Self {
        // SAFETY: a private-state source: our events neither inherit nor change the hardware
        // modifier state, and local hardware events are not suppressed while we post.
        let src = unsafe { CGEventSourceCreate(kCGEventSourceStatePrivate) };
        if !src.is_null() {
            // SAFETY: valid source.
            unsafe { CGEventSourceSetUserData(src, SOURCE_TAG) };
        }
        // Accessibility calls into a hung application must not stall the broker.
        // SAFETY: the system-wide element sets the global messaging timeout.
        if let Some(sys) = Owned::from_create(unsafe { AXUIElementCreateSystemWide() }) {
            unsafe { AXUIElementSetMessagingTimeout(sys.as_ptr(), 1.5) };
        }
        let ds = displays();
        let (locked, on_console) = session_state();
        Self {
            src: EventSource(src),
            input: Mutex::new(InputState::default()),
            watch: Mutex::new(Watch {
                generation: 1,
                layout_sig: layout_signature(&ds),
                locked: Some(locked || !on_console),
                last_poll: Instant::now(),
                last_wall: SystemTime::now(),
                polls: 0,
                active: false,
                post_at_begin: false,
                trusted_at_begin: false,
                pending: vec![],
            }),
            apps: Mutex::new(HashMap::new()),
            dock: Mutex::new(None),
        }
    }

    /// Current displays plus the layout generation, bumping it when the configuration changed.
    fn current_layout(&self) -> Layout {
        let ds = displays();
        let sig = layout_signature(&ds);
        let mut w = lock(&self.watch);
        if sig != w.layout_sig {
            w.layout_sig = sig;
            w.generation += 1;
            w.pending.push(SessionEvent::LayoutChanged);
        }
        Layout { generation: w.generation, displays: ds }
    }

    /// Bundle identifier of a process, falling back to its window-list owner name.
    fn app_id(&self, pid: i32, owner: &str) -> String {
        if let Some((o, id)) = lock(&self.apps).get(&pid)
            && o == owner
        {
            return id.clone();
        }
        let bundle = objc2::rc::autoreleasepool(|_| {
            objc2_app_kit::NSRunningApplication::runningApplicationWithProcessIdentifier(pid).and_then(|a| a.bundleIdentifier()).map(|s| s.to_string())
        });
        let id = bundle.filter(|s| !s.is_empty()).unwrap_or_else(|| owner.to_string());
        lock(&self.apps).insert(pid, (owner.to_string(), id.clone()));
        id
    }

    fn info(&self, w: &RawWindow, ds: &[DisplayInfo], focused: bool) -> WindowInfo {
        WindowInfo { id: w.id.to_string(), app_id: self.app_id(w.pid, &w.owner), title: w.title.clone(), bounds: Some(w.bounds), display: display_of(ds, &w.bounds), focused }
    }

    /// The focused normal window: Accessibility's answer when trusted, else the front-most
    /// normal window of the front-most application.
    fn focused_id(&self, raw: &[RawWindow]) -> Option<u32> {
        let trusted = permissions::accessibility_trusted();
        let front_pid = trusted.then(ax_focused_app_pid).flatten().or_else(|| raw.iter().find(|w| w.normal()).map(|w| w.pid))?;
        if trusted
            && let Some(id) = ax_focused_window_of(front_pid)
            && raw.iter().any(|w| w.id == id && w.normal())
        {
            return Some(id);
        }
        raw.iter().find(|w| w.normal() && w.pid == front_pid).map(|w| w.id)
    }

    /// The Dock keeps a transparent window over each whole display (layer 20) for Mission
    /// Control and Launchpad; only the Dock bar itself covers anything. True when `w` is that
    /// overlay.
    fn is_dock_overlay(w: &RawWindow, ds: &[DisplayInfo]) -> bool {
        let same = |a: f64, b: f64| (a - b).abs() <= 1.0;
        w.owner == "Dock" && w.layer > 0 && ds.iter().any(|d| same(d.logical.x, w.bounds.x) && same(d.logical.y, w.bounds.y) && same(d.logical.width, w.bounds.width) && same(d.logical.height, w.bounds.height))
    }

    /// Where the Dock bar is, from the Dock's accessibility tree (its `AXList` elements). Empty
    /// without Accessibility, in which case the Dock is not counted as an occluder.
    fn dock_regions(&self, pid: i32) -> Vec<Rect> {
        if let Some((t, p, r)) = lock(&self.dock).as_ref()
            && *p == pid
            && t.elapsed() < Duration::from_secs(1)
        {
            return r.clone();
        }
        let mut regions = vec![];
        if permissions::accessibility_trusted()
            // SAFETY: +1 application element.
            && let Some(app) = Owned::from_create(unsafe { AXUIElementCreateApplication(pid) })
            && let Some(kids) = ax_attr(app.as_ptr(), "AXChildren")
        {
            regions = cf::array_items(kids.as_ptr()).into_iter().filter(|k| ax_string(*k, "AXRole").as_deref() == Some("AXList")).filter_map(ax_bounds).collect();
        }
        *lock(&self.dock) = Some((Instant::now(), pid, regions.clone()));
        regions
    }

    fn post(&self, st: &mut InputState, e: CGEventRef) -> Result<()> {
        if e.is_null() {
            return Err(AdlError::new(ErrorCode::OutcomeUnknown, "Core Graphics could not create the event"));
        }
        // SAFETY: e is a valid +1 event; posting copies it.
        unsafe {
            CGEventSetIntegerValueField(e, kCGEventSourceUserData, SOURCE_TAG);
            CGEventPost(kCGHIDEventTap, e);
            CFRelease(e as CFTypeRef);
        }
        st.last_inject = Some(Instant::now());
        Ok(())
    }

    fn pointer_now(&self, st: &InputState) -> (f64, f64) {
        if let Some(p) = st.pointer {
            return p;
        }
        current_pointer()
    }

    fn move_to(&self, st: &mut InputState, x: f64, y: f64) -> Result<()> {
        let (ty, button) = if st.buttons.contains(&0) {
            (kCGEventLeftMouseDragged, 0)
        } else if st.buttons.contains(&1) {
            (kCGEventRightMouseDragged, 1)
        } else if let Some(b) = st.buttons.iter().next() {
            (kCGEventOtherMouseDragged, *b)
        } else {
            (kCGEventMouseMoved, 0)
        };
        // SAFETY: valid source; the event is posted and released by `post`.
        let e = unsafe { CGEventCreateMouseEvent(self.src.0, ty, CGPoint { x, y }, button) };
        if !e.is_null() {
            unsafe {
                CGEventSetFlags(e, st.flags);
                if ty == kCGEventOtherMouseDragged {
                    CGEventSetIntegerValueField(e, kCGMouseEventButtonNumber, button as i64);
                }
            }
        }
        self.post(st, e)?;
        st.pointer = Some((x, y));
        Ok(())
    }

    fn button(&self, st: &mut InputState, b: Button, pressed: bool) -> Result<()> {
        let n = keymap::button_number(b);
        let pos = self.pointer_now(st);
        let ty = match (n, pressed) {
            (0, true) => kCGEventLeftMouseDown,
            (0, false) => kCGEventLeftMouseUp,
            (1, true) => kCGEventRightMouseDown,
            (1, false) => kCGEventRightMouseUp,
            (_, true) => kCGEventOtherMouseDown,
            (_, false) => kCGEventOtherMouseUp,
        };
        let count = if pressed {
            let c = match st.last_press {
                Some((lb, t, p, c)) if lb == n && t.elapsed() < DOUBLE_CLICK && (p.0 - pos.0).abs() < 5.0 && (p.1 - pos.1).abs() < 5.0 => c + 1,
                _ => 1,
            };
            st.last_press = Some((n, Instant::now(), pos, c));
            c
        } else {
            st.last_press.filter(|(lb, ..)| *lb == n).map(|(.., c)| c).unwrap_or(1)
        };
        // SAFETY: valid source; posted and released by `post`.
        let e = unsafe { CGEventCreateMouseEvent(self.src.0, ty, CGPoint { x: pos.0, y: pos.1 }, n) };
        if !e.is_null() {
            unsafe {
                CGEventSetIntegerValueField(e, kCGMouseEventClickState, count);
                CGEventSetIntegerValueField(e, kCGMouseEventButtonNumber, n as i64);
                CGEventSetFlags(e, st.flags);
            }
        }
        self.post(st, e)?;
        if pressed {
            st.buttons.insert(n);
        } else {
            st.buttons.remove(&n);
        }
        Ok(())
    }

    fn key(&self, st: &mut InputState, k: &Key, pressed: bool) -> Result<()> {
        let mk = keymap::key(k).ok_or_else(|| AdlError::unsupported(format!("{k} has no macOS key")))?;
        // SAFETY: valid source; posted and released by `post`.
        let e = unsafe { CGEventCreateKeyboardEvent(self.src.0, mk.code, pressed) };
        if e.is_null() {
            return self.post(st, e);
        }
        match mk.modifier_flag {
            Some(flag) => {
                if pressed {
                    st.flags |= flag;
                } else {
                    st.flags &= !flag;
                }
                unsafe {
                    CGEventSetType(e, kCGEventFlagsChanged);
                    CGEventSetFlags(e, st.flags);
                }
            }
            None => unsafe { CGEventSetFlags(e, st.flags | mk.flags) },
        }
        self.post(st, e)
    }

    fn text(&self, st: &mut InputState, text: &str) -> Result<()> {
        for piece in keymap::utf16_pieces(text, 1) {
            for down in [true, false] {
                // SAFETY: valid source; the UTF-16 buffer outlives the call; posted by `post`.
                let e = unsafe { CGEventCreateKeyboardEvent(self.src.0, 0, down) };
                if !e.is_null() {
                    unsafe {
                        CGEventKeyboardSetUnicodeString(e, piece.len(), piece.as_ptr());
                        CGEventSetFlags(e, 0);
                    }
                }
                self.post(st, e)?;
            }
            std::thread::sleep(TEXT_PACE);
        }
        Ok(())
    }

    fn scroll(&self, st: &mut InputState, axis: Axis, steps: i32) -> Result<()> {
        // The broker's steps are positive down/right; Quartz wheel deltas are positive up/left.
        let (v, h) = match axis {
            Axis::Vertical => (-steps, 0),
            Axis::Horizontal => (0, -steps),
        };
        // SAFETY: valid source; posted and released by `post`.
        let e = unsafe { CGEventCreateScrollWheelEvent2(self.src.0, kCGScrollEventUnitLine, 2, v, h, 0) };
        if !e.is_null() {
            unsafe { CGEventSetFlags(e, st.flags) };
        }
        self.post(st, e)
    }

    /// Sleep is not observable without a run loop, but the monotonic clock stops while the
    /// machine sleeps and the wall clock does not; a large gap means we slept.
    fn detect_sleep(w: &mut Watch) {
        let mono = w.last_poll.elapsed();
        let wall = SystemTime::now().duration_since(w.last_wall).unwrap_or_default();
        if wall > mono + Duration::from_secs(5) {
            w.pending.push(SessionEvent::Suspending);
            w.pending.push(SessionEvent::Resumed);
        }
    }
}

/// Where the pointer is now (Quartz global points). Needs no permission.
pub fn current_pointer() -> (f64, f64) {
    // SAFETY: a null-source event only reads the current state.
    unsafe {
        let e = CGEventCreate(std::ptr::null_mut());
        if e.is_null() {
            return (0.0, 0.0);
        }
        let p = CGEventGetLocation(e);
        CFRelease(e as CFTypeRef);
        (p.x, p.y)
    }
}

fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" || &png[12..16] != b"IHDR" {
        return None;
    }
    Some((u32::from_be_bytes(png[16..20].try_into().ok()?), u32::from_be_bytes(png[20..24].try_into().ok()?)))
}

/// `CGDisplayCreateImage` encoded as PNG by ImageIO. The function is deprecated (ScreenCaptureKit
/// replaces it) and looked up at run time, so a release without it falls back to `screencapture`.
fn capture_cg(display: u32) -> Option<Vec<u8>> {
    type CreateImage = unsafe extern "C" fn(CGDirectDisplayID) -> CGImageRef;
    let f = optional_symbol(c"CGDisplayCreateImage")?;
    // SAFETY: documented signature of CGDisplayCreateImage.
    let create: CreateImage = unsafe { std::mem::transmute::<*mut c_void, CreateImage>(f) };
    unsafe {
        let img = create(display);
        if img.is_null() {
            return None;
        }
        let data = CFDataCreateMutable(std::ptr::null(), 0);
        let uti = cf::string("public.png");
        let dest = CGImageDestinationCreateWithData(data, uti.as_ptr(), 1, std::ptr::null());
        let mut out = None;
        if !dest.is_null() {
            CGImageDestinationAddImage(dest, img, std::ptr::null());
            if CGImageDestinationFinalize(dest) {
                let len = CFDataGetLength(data as CFTypeRef) as usize;
                out = Some(std::slice::from_raw_parts(CFDataGetBytePtr(data as CFTypeRef), len).to_vec());
            }
            CFRelease(dest as CFTypeRef);
        }
        CFRelease(data as CFTypeRef);
        CGImageRelease(img);
        out
    }
}

/// `screencapture -x -D<n>`: n is 1 for the main display, in active-display-list order.
fn capture_tool(index: usize) -> Result<Vec<u8>> {
    let path = std::env::temp_dir().join(format!("w3bs-desktop-capture-{}-{}.png", std::process::id(), adl_broker::util::now_ms()));
    let out = std::process::Command::new("/usr/sbin/screencapture").arg("-x").arg("-t").arg("png").arg(format!("-D{}", index + 1)).arg(&path).output().map_err(|e| AdlError::new(ErrorCode::Blocked, format!("screencapture: {e}")))?;
    let png = std::fs::read(&path);
    let _ = std::fs::remove_file(&path);
    match png {
        Ok(p) if !p.is_empty() => Ok(p),
        _ => Err(AdlError::new(ErrorCode::Blocked, format!("screencapture failed: {}", String::from_utf8_lossy(&out.stderr).trim()))),
    }
}

impl Adapter for MacAdapter {
    fn name(&self) -> String {
        BACKEND.into()
    }

    fn capabilities(&self) -> Vec<CapabilityReport> {
        let p = permissions::check();
        let feats = |f: &[&str]| f.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
        let report = |c, ok: bool, why: String, features| CapabilityReport {
            capability: c,
            availability: if ok { Availability::Available } else { Availability::PermissionRequired },
            backend: BACKEND.into(),
            reason: if ok { String::new() } else { why },
            features,
        };
        let input_why = format!("macOS has not allowed this process to post input events: turn it on in {}", permissions::ACCESSIBILITY_PANE);
        let mut windows = report(Capability::Windows, true, String::new(), feats(&["window_at", "focus", "stacking"]));
        if !p.screen_capture {
            windows.availability = Availability::Degraded;
            windows.reason = format!("window titles are hidden until Screen Recording is allowed ({})", permissions::SCREEN_RECORDING_PANE);
        }
        let mut out = vec![
            report(Capability::Pointer, p.post_events, input_why.clone(), feats(&["absolute_motion", "relative_motion", "pointer_position", "button.middle", "button.back", "button.forward", "scroll.horizontal", "click_count"])),
            report(Capability::Keyboard, p.post_events, input_why, feats(&["text_insertion", "target_bound_keyboard", "physical_input_detection", "unicode_text"])),
            report(Capability::ScreenCapture, p.screen_capture, format!("turn on Screen Recording for this process in {}", permissions::SCREEN_RECORDING_PANE), BTreeSet::new()),
            windows,
            report(Capability::Accessibility, p.accessibility, format!("turn on Accessibility for this process in {}", permissions::ACCESSIBILITY_PANE), BTreeSet::new()),
        ];
        for c in [Capability::ClipboardRead, Capability::ClipboardWrite, Capability::Browser, Capability::Files, Capability::Email, Capability::Calendar] {
            out.push(CapabilityReport { capability: c, availability: Availability::Unsupported, backend: BACKEND.into(), reason: "not implemented by the macOS adapter".into(), features: BTreeSet::new() });
        }
        out
    }

    fn begin(&self) -> Result<()> {
        let p = permissions::check();
        if !p.post_events {
            return Err(AdlError::new(
                ErrorCode::Blocked,
                format!("macOS does not allow this process to post input events. Turn it on in {} (for the terminal or app that starts the broker), then enable again; `w3bs desktop doctor --prompt` shows the system prompt", permissions::ACCESSIBILITY_PANE),
            ));
        }
        if self.src.0.is_null() {
            return Err(AdlError::new(ErrorCode::Blocked, "could not create a Core Graphics event source"));
        }
        *lock(&self.input) = InputState::default();
        let mut w = lock(&self.watch);
        w.active = true;
        w.post_at_begin = true;
        w.trusted_at_begin = p.accessibility;
        Ok(())
    }

    fn end(&self) {
        let mut st = lock(&self.input);
        st.buttons.clear();
        st.flags = 0;
        st.pointer = None;
        lock(&self.watch).active = false;
    }

    fn layout(&self) -> Result<Layout> {
        let l = self.current_layout();
        if l.displays.is_empty() {
            return Err(AdlError::new(ErrorCode::Blocked, "no active displays (is there a logged-in window server session?)"));
        }
        Ok(l)
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        let raw = window_list();
        let ds = displays();
        let focused = self.focused_id(&raw);
        Ok(raw.iter().filter(|w| w.normal()).map(|w| self.info(w, &ds, Some(w.id) == focused)).collect())
    }

    fn focused_window(&self) -> Result<Option<WindowInfo>> {
        let raw = window_list();
        let Some(id) = self.focused_id(&raw) else { return Ok(None) };
        let ds = displays();
        Ok(raw.iter().find(|w| w.id == id).map(|w| self.info(w, &ds, true)))
    }

    fn window_at(&self, x: f64, y: f64) -> Result<Option<WindowInfo>> {
        let raw = window_list();
        let ds = displays();
        let Some(hit) = raw.iter().filter(|w| w.alpha > 0.0 && w.bounds.contains(x, y)).find(|w| !Self::is_dock_overlay(w, &ds) || self.dock_regions(w.pid).iter().any(|r| r.contains(x, y))) else {
            return Ok(None);
        };
        let focused = hit.normal() && self.focused_id(&raw) == Some(hit.id);
        Ok(Some(self.info(hit, &ds, focused)))
    }

    fn inspect(&self, window: &str) -> Result<AccessNode> {
        let id: u32 = window.parse().map_err(|_| AdlError::not_found(format!("no window {window}")))?;
        let w = window_list().into_iter().find(|w| w.id == id).ok_or_else(|| AdlError::not_found(format!("no window {window}")))?;
        if !permissions::accessibility_trusted() {
            return Err(AdlError::new(ErrorCode::Blocked, format!("inspecting windows needs Accessibility: turn it on in {}", permissions::ACCESSIBILITY_PANE)));
        }
        // SAFETY: +1 application element.
        let app = Owned::from_create(unsafe { AXUIElementCreateApplication(w.pid) }).ok_or_else(|| AdlError::internal("AXUIElementCreateApplication failed"))?;
        let wins = ax_attr(app.as_ptr(), "AXWindows").ok_or_else(|| AdlError::new(ErrorCode::OutcomeUnknown, format!("{} did not answer the Accessibility request", w.owner)))?;
        let items = cf::array_items(wins.as_ptr());
        let close = |a: f64, b: f64| (a - b).abs() <= 2.0;
        let el = items
            .iter()
            .copied()
            .find(|el| ax_window_id(*el) == Some(id))
            .or_else(|| {
                items.iter().copied().find(|el| {
                    ax_bounds(*el).is_some_and(|b| close(b.x, w.bounds.x) && close(b.y, w.bounds.y) && close(b.width, w.bounds.width) && close(b.height, w.bounds.height))
                        && (w.title.is_empty() || ax_string(*el, "AXTitle").as_deref() == Some(w.title.as_str()))
                })
            })
            .ok_or_else(|| AdlError::stale(format!("window {window} is not in {}'s accessibility tree", w.owner)))?;
        let mut budget = MAX_AX_NODES;
        Ok(ax_node(el, format!("{window}/0"), 0, &mut budget, Instant::now() + Duration::from_secs(5)))
    }

    fn capture(&self, display: &str) -> Result<(Vec<u8>, u32, u32)> {
        let ds = displays();
        let (index, d) = ds.iter().enumerate().find(|(_, d)| d.id == display).ok_or_else(|| AdlError::not_found(format!("no display {display}")))?;
        if !permissions::check().screen_capture {
            return Err(AdlError::new(ErrorCode::Blocked, format!("screen capture needs Screen Recording: turn it on in {}", permissions::SCREEN_RECORDING_PANE)));
        }
        let id: u32 = d.id.parse().unwrap_or(0);
        let png = match capture_cg(id) {
            Some(p) => p,
            None => capture_tool(index)?,
        };
        let (w, h) = png_size(&png).ok_or_else(|| AdlError::internal("capture did not produce a PNG"))?;
        Ok((png, w, h))
    }

    fn pointer_position(&self) -> Result<Option<(f64, f64)>> {
        Ok(Some(current_pointer()))
    }

    fn emit(&self, event: &InputEvent) -> Result<()> {
        let mut st = lock(&self.input);
        match event {
            InputEvent::MotionAbsolute { x, y } => self.move_to(&mut st, *x, *y),
            InputEvent::MotionRelative { dx, dy } => {
                let (x, y) = current_pointer();
                self.move_to(&mut st, x + dx, y + dy)
            }
            InputEvent::Button { button, pressed } => self.button(&mut st, *button, *pressed),
            InputEvent::Scroll { axis, steps } => self.scroll(&mut st, *axis, *steps),
            InputEvent::Key { key, pressed } => self.key(&mut st, key, *pressed),
            InputEvent::Text { text } => self.text(&mut st, text),
        }
    }

    fn safety_interlock(&self) -> Option<String> {
        // SAFETY: plain query.
        if unsafe { IsSecureEventInputEnabled() } != 0 {
            return Some("secure keyboard entry is on (a password field or another app has enabled secure input)".into());
        }
        let (locked, on_console) = session_state();
        if locked {
            return Some("the screen is locked".into());
        }
        if !on_console {
            return Some("this user session is not on the console".into());
        }
        if let Some(w) = window_list().into_iter().find(|w| w.alpha > 0.0 && PROMPT_OWNERS.contains(&w.owner.as_str())) {
            return Some(format!("an authentication or permission prompt is on screen ({})", w.owner));
        }
        None
    }

    fn poll_events(&self) -> Vec<SessionEvent> {
        let (polls, active) = {
            let mut w = lock(&self.watch);
            Self::detect_sleep(&mut w);
            w.polls += 1;
            (w.polls, w.active)
        };
        let (locked, on_console) = session_state();
        let now_locked = locked || !on_console;
        // SAFETY: plain query.
        let idle = unsafe { CGEventSourceSecondsSinceLastEventType(kCGEventSourceStateHIDSystemState, kCGAnyInputEventType) };
        // Our own posted events also reset the HID idle time (seen on macOS 26), so an event only
        // counts as the human's when it came clearly after our last injection.
        let ours = lock(&self.input).last_inject.is_some_and(|t| t.elapsed().as_secs_f64() < idle + 0.05);
        if polls % 10 == 0 {
            let _ = self.current_layout();
        }
        let perms = (active && polls % 10 == 0).then(permissions::check);
        let mut w = lock(&self.watch);
        if w.locked != Some(now_locked) {
            if w.locked.is_some() {
                w.pending.push(if now_locked { SessionEvent::Locked } else { SessionEvent::Unlocked });
            }
            w.locked = Some(now_locked);
        }
        let gap = w.last_poll.elapsed().as_secs_f64();
        if idle < gap && !ours {
            w.pending.push(SessionEvent::PhysicalInput);
        }
        if let Some(p) = perms {
            if w.post_at_begin && !p.post_events {
                w.post_at_begin = false;
                w.pending.push(SessionEvent::PermissionLost("posting input events (Accessibility)".into()));
            }
            if w.trusted_at_begin && !p.accessibility {
                w.trusted_at_begin = false;
                w.pending.push(SessionEvent::PermissionLost("Accessibility".into()));
            }
        }
        w.last_poll = Instant::now();
        w.last_wall = SystemTime::now();
        std::mem::take(&mut w.pending)
    }
}
