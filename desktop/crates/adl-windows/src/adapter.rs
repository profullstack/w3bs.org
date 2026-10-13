//! `WindowsAdapter`: the `adl_broker::Adapter` for a Windows desktop session.

use crate::capture::{self, Monitor};
use crate::session::{self, Shared};
use crate::{input, map, uia, win};
use adl_broker::{Adapter, InputEvent, SessionEvent};
use adl_schema::*;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::RemoteDesktop::*;
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS, UOI_NAME};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::WindowsAndMessaging::{IsWindow, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW};

const BACKEND: &str = "windows-native";
/// Input newer than our last injection by more than this is taken as the human's.
const OWN_INPUT_GRACE_MS: u32 = 300;
/// Shell surfaces that are not user windows (they still count as occluders in `window_at`).
const SHELL_CLASSES: &[&str] = &["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd", "W3bsDesktopSession", "W3bsDesktopTray"];

pub struct WindowsAdapter {
    shared: Arc<Shared>,
    /// (monitor signature, generation)
    layout: Mutex<(String, u64)>,
    /// GetTickCount() of our most recent SendInput.
    injected_at: AtomicU32,
    /// Last `GetLastInputInfo` tick seen by `poll_events`.
    input_seen: AtomicU32,
    dpi_aware: bool,
    integrity: Option<u32>,
}

impl Default for WindowsAdapter {
    fn default() -> Self {
        Self::new()
    }
}

fn tick() -> u32 {
    // SAFETY: plain call.
    unsafe { GetTickCount() }
}

fn last_input_tick() -> Option<u32> {
    let mut li = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    // SAFETY: correctly sized out-parameter.
    unsafe { GetLastInputInfo(&mut li) }.as_bool().then_some(li.dwTime)
}

/// AppUserModelID of a window, when the app set one (UWP and packaged apps, many browsers).
fn aumid(h: HWND) -> Option<String> {
    win::ensure_com();
    // SAFETY: COM calls on an initialized thread; the PROPVARIANT and string are freed here.
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(h).ok()?;
        let mut pv = store.GetValue(&windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID).ok()?;
        let s = PropVariantToStringAlloc(&pv).ok();
        let _ = PropVariantClear(&mut pv);
        let s = s?;
        let out = s.to_string().ok();
        CoTaskMemFree(Some(s.0 as *const _));
        out.filter(|v| !v.is_empty())
    }
}

fn app_id(h: HWND) -> String {
    aumid(h).or_else(|| win::process_image_name(win::window_pid(h))).unwrap_or_else(|| format!("pid:{}", win::window_pid(h)))
}

fn rect_of(r: windows::Win32::Foundation::RECT) -> Rect {
    Rect { x: r.left as f64, y: r.top as f64, width: (r.right - r.left) as f64, height: (r.bottom - r.top) as f64 }
}

fn display_of(b: &Rect, ms: &[Monitor]) -> Option<String> {
    let (cx, cy) = ((b.x + b.width / 2.0) as i32, (b.y + b.height / 2.0) as i32);
    ms.iter().find(|m| cx >= m.rect.left && cx < m.rect.right && cy >= m.rect.top && cy < m.rect.bottom).map(|m| m.id.clone())
}

fn describe(h: HWND, fg: Option<HWND>, ms: &[Monitor]) -> WindowInfo {
    let bounds = if win::is_minimized(h) { None } else { win::frame_bounds(h).map(rect_of).filter(|r| r.width > 0.0 && r.height > 0.0) };
    WindowInfo {
        id: map::window_id(win::hwnd_id(h)),
        app_id: app_id(h),
        title: win::window_title(h),
        display: bounds.as_ref().and_then(|b| display_of(b, ms)),
        bounds,
        focused: fg == Some(h),
    }
}

/// A window a person works in: visible, not cloaked (other virtual desktop, suspended UWP), not a
/// tool window (unless it forces a taskbar button), titled, and not a shell surface.
fn is_user_window(h: HWND) -> bool {
    if !win::is_visible(h) || win::is_cloaked(h) {
        return false;
    }
    let ex = win::ex_style(h);
    if ex.contains(WS_EX_TOOLWINDOW) && !ex.contains(WS_EX_APPWINDOW) {
        return false;
    }
    if win::window_title(h).is_empty() {
        return false;
    }
    !SHELL_CLASSES.contains(&win::class_name(h).as_str())
}

fn hwnd_from_id(id: &str) -> Result<HWND> {
    let h = map::parse_window_id(id).map(win::hwnd_of).ok_or_else(|| AdlError::not_found(format!("no window {id}")))?;
    // SAFETY: plain query.
    if !unsafe { IsWindow(Some(h)) }.as_bool() {
        return Err(AdlError::stale(format!("window {id} no longer exists")));
    }
    Ok(h)
}

/// Name of the desktop currently receiving input (`Default` normally; `Winlogon` for the lock
/// screen, UAC prompt and Ctrl+Alt+Del screen). `Err` when it cannot be opened at all, which is
/// itself the secure-desktop case for a normal user process.
fn input_desktop_name() -> std::result::Result<String, String> {
    // SAFETY: open/query/close of the input desktop handle.
    unsafe {
        let d = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS).map_err(|e| e.to_string())?;
        let mut buf = [0u16; 128];
        let mut need = 0u32;
        let r = GetUserObjectInformationW(HANDLE(d.0), UOI_NAME, Some(buf.as_mut_ptr().cast()), (buf.len() * 2) as u32, Some(&mut need));
        let _ = CloseDesktop(d);
        r.map_err(|e| e.to_string())?;
        Ok(win::wstr(&buf))
    }
}

/// WTS's view of the session lock state (`None` when unknown).
fn wts_locked() -> Option<bool> {
    // SAFETY: WTS allocates the buffer; freed with WTSFreeMemory.
    unsafe {
        let mut buf = windows::core::PWSTR::null();
        let mut len = 0u32;
        WTSQuerySessionInformationW(Some(WTS_CURRENT_SERVER_HANDLE), WTS_CURRENT_SESSION, WTSSessionInfoEx, &mut buf, &mut len).ok()?;
        let info = &*(buf.0 as *const WTSINFOEXW);
        // Lazy on purpose: the union is only meaningful for level 1.
        #[allow(clippy::unnecessary_lazy_evaluations)]
        let r = (info.Level == 1).then(|| info.Data.WTSInfoExLevel1.SessionFlags as u32 == WTS_SESSIONSTATE_LOCK);
        WTSFreeMemory(buf.0.cast());
        r
    }
}

impl WindowsAdapter {
    pub fn new() -> Self {
        let dpi_aware = win::init_dpi_awareness();
        let shared = session::start();
        let sig = capture::signature(&capture::monitors());
        Self {
            shared,
            layout: Mutex::new((sig, 1)),
            injected_at: AtomicU32::new(0),
            input_seen: AtomicU32::new(last_input_tick().unwrap_or(0)),
            dpi_aware,
            integrity: win::current_integrity(),
        }
    }

    /// Current monitors, bumping the layout generation when the configuration changed.
    fn monitors(&self) -> (Vec<Monitor>, u64, bool) {
        let ms = capture::monitors();
        let sig = capture::signature(&ms);
        let mut l = self.layout.lock().unwrap_or_else(|e| e.into_inner());
        let changed = l.0 != sig;
        if changed {
            l.0 = sig;
            l.1 += 1;
        }
        (ms, l.1, changed)
    }

    fn mark_injected(&self) {
        self.injected_at.store(tick(), Ordering::SeqCst);
    }

    fn elevated(&self) -> bool {
        self.integrity.is_some_and(|i| i >= 0x3000)
    }

    fn uipi_note(&self) -> String {
        if self.elevated() {
            "the broker runs elevated, so it can drive elevated windows too; secure desktops (UAC prompt, lock screen) stay out of reach".into()
        } else {
            "cannot drive windows of elevated (administrator) apps while the broker is not elevated (UIPI); such input is refused, never sent blind".into()
        }
    }
}

impl Adapter for WindowsAdapter {
    fn name(&self) -> String {
        BACKEND.into()
    }

    fn capabilities(&self) -> Vec<CapabilityReport> {
        let feats = |f: &[&str]| f.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
        let rep = |c, a, reason: String, features| CapabilityReport { capability: c, availability: a, backend: BACKEND.into(), reason, features };
        let avail = if self.dpi_aware { Availability::Available } else { Availability::Degraded };
        let dpi_note = if self.dpi_aware { String::new() } else { "per-monitor DPI awareness could not be enabled; coordinates on scaled monitors may be virtualized. ".into() };
        let mut kb = feats(&["text_insertion", "physical_input_detection"]);
        let mut ptr = feats(&["absolute_motion", "relative_motion", "pointer_position", "button.back", "button.forward", "scroll.horizontal"]);
        if let Some(h) = session::hotkey_description() {
            kb.insert(format!("emergency_hotkey:{h}"));
            ptr.insert(format!("emergency_hotkey:{h}"));
        }
        let mut out = vec![
            rep(Capability::Pointer, avail, format!("{dpi_note}SendInput; {}", self.uipi_note()), ptr),
            rep(
                Capability::Keyboard,
                avail,
                format!(
                    "SendInput delivers to the foreground window (not target-bound); the broker checks focus before every 32-character chunk. {} Physical-input detection: a real mouse is recognized by Windows' injected flag (low-level mouse hook, no positions kept); keyboard use only via GetLastInputInfo, which is blind while the agent is typing and cannot tell other software's injected input from the human's. No keyboard hook is installed.",
                    self.uipi_note()
                ),
                kb,
            ),
            rep(Capability::ScreenCapture, avail, "GDI BitBlt per monitor; windows that exclude themselves from capture (DRM, WDA_EXCLUDEFROMCAPTURE) appear black".into(), feats(&["per_display"])),
            rep(Capability::Windows, Availability::Available, "EnumWindows + DWM frame bounds; occlusion via WindowFromPoint (the taskbar and the desktop count as covering)".into(), feats(&["window_at", "focus", "geometry", "stacking"])),
            rep(Capability::Accessibility, Availability::Available, "UI Automation control view; application support varies; password values are never read".into(), feats(&["uia"])),
        ];
        for c in [Capability::ClipboardRead, Capability::ClipboardWrite, Capability::Browser, Capability::Files, Capability::Email, Capability::Calendar] {
            out.push(rep(c, Availability::Unsupported, "not implemented by the Windows adapter".into(), BTreeSet::new()));
        }
        out
    }

    fn begin(&self) -> Result<()> {
        if let Some(why) = self.safety_interlock() {
            return Err(AdlError::new(ErrorCode::Blocked, why));
        }
        Ok(())
    }

    fn end(&self) {}

    fn layout(&self) -> Result<Layout> {
        let (ms, generation, _) = self.monitors();
        if ms.is_empty() {
            return Err(AdlError::new(ErrorCode::Blocked, "no monitors are attached to this session"));
        }
        Ok(Layout { generation, displays: ms.iter().map(Monitor::info).collect() })
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        let ms = capture::monitors();
        let fg = win::foreground();
        Ok(win::top_level_windows().into_iter().filter(|h| is_user_window(*h)).map(|h| describe(h, fg, &ms)).collect())
    }

    fn focused_window(&self) -> Result<Option<WindowInfo>> {
        let ms = capture::monitors();
        Ok(win::foreground().map(|h| describe(h, Some(h), &ms)))
    }

    fn window_at(&self, x: f64, y: f64) -> Result<Option<WindowInfo>> {
        let ms = capture::monitors();
        let fg = win::foreground();
        Ok(win::root_at(x.floor() as i32, y.floor() as i32).map(|h| describe(h, fg, &ms)))
    }

    fn inspect(&self, window: &str) -> Result<AccessNode> {
        let h = hwnd_from_id(window)?;
        uia::inspect(h, window)
    }

    fn capture(&self, display: &str) -> Result<(Vec<u8>, u32, u32)> {
        if let Err(e) = input_desktop_name() {
            return Err(AdlError::new(ErrorCode::Blocked, format!("a secure desktop is active ({e}); nothing is captured")));
        }
        let (ms, _, _) = self.monitors();
        let m = ms.iter().find(|m| m.id == display).ok_or_else(|| AdlError::not_found(format!("no display {display}")))?;
        capture::capture(m)
    }

    fn pointer_position(&self) -> Result<Option<(f64, f64)>> {
        Ok(win::cursor_pos().map(|(x, y)| (x as f64, y as f64)))
    }

    fn emit(&self, event: &InputEvent) -> Result<()> {
        self.mark_injected();
        let r = match event {
            InputEvent::MotionAbsolute { x, y } => {
                let (x, y) = (x.floor() as i32, y.floor() as i32);
                input::check_uipi(win::root_at(x, y), "window under the target point").and_then(|_| input::move_to(x, y))
            }
            InputEvent::MotionRelative { dx, dy } => {
                let (cx, cy) = win::cursor_pos().ok_or_else(|| AdlError::internal("cannot read the pointer position"))?;
                let (x, y) = (cx + dx.round() as i32, cy + dy.round() as i32);
                input::check_uipi(win::root_at(x, y), "window under the target point").and_then(|_| input::move_to(x, y))
            }
            InputEvent::Button { button, pressed } => {
                let under = win::cursor_pos().and_then(|(x, y)| win::root_at(x, y));
                // A release always goes through, so nothing stays held.
                if *pressed {
                    input::check_uipi(under, "window under the pointer")?;
                }
                input::button(*button, *pressed)
            }
            InputEvent::Scroll { axis, steps } => {
                let under = win::cursor_pos().and_then(|(x, y)| win::root_at(x, y));
                input::check_uipi(under, "window under the pointer").and_then(|_| input::scroll(*axis, *steps))
            }
            InputEvent::Key { key, pressed } => {
                if *pressed {
                    input::check_uipi(win::foreground(), "focused window")?;
                }
                input::key(key, *pressed)
            }
            InputEvent::Text { text } => {
                let fg0 = win::foreground();
                input::check_uipi(fg0, "focused window")?;
                input::text(text, || {
                    self.mark_injected();
                    // Focus moving mid-chunk stops the text at that character.
                    if win::foreground() != fg0 {
                        return Err(AdlError::stale("focus moved away from the target window while typing"));
                    }
                    match self.safety_interlock() {
                        Some(why) => Err(AdlError::new(ErrorCode::Blocked, format!("input paused: {why}"))),
                        None => Ok(()),
                    }
                })
            }
        };
        self.mark_injected();
        r
    }

    fn safety_interlock(&self) -> Option<String> {
        if *self.shared.locked.lock().unwrap_or_else(|e| e.into_inner()) {
            return Some("the session is locked or not on screen".into());
        }
        match input_desktop_name() {
            Err(_) => return Some("a secure desktop is active (UAC prompt, lock screen or Ctrl+Alt+Del screen); the human must deal with it".into()),
            Ok(name) if !name.eq_ignore_ascii_case("Default") => return Some(format!("the input desktop is {name:?} (a secure desktop such as a UAC prompt or the lock screen)")),
            Ok(_) => {}
        }
        if wts_locked() == Some(true) {
            return Some("the session is locked".into());
        }
        None
    }

    fn poll_events(&self) -> Vec<SessionEvent> {
        let mut out = std::mem::take(&mut *self.shared.events.lock().unwrap_or_else(|e| e.into_inner()));
        // Display changes that arrive without WM_DISPLAYCHANGE (DPI scale, rotation on some
        // drivers) are caught by the signature.
        let (_, _, changed) = self.monitors();
        if changed && !out.contains(&SessionEvent::LayoutChanged) {
            out.push(SessionEvent::LayoutChanged);
        }
        if let Some(t) = last_input_tick() {
            let prev = self.input_seen.swap(t, Ordering::SeqCst);
            let since_ours = t.wrapping_sub(self.injected_at.load(Ordering::SeqCst));
            if t != prev && since_ours > OWN_INPUT_GRACE_MS && since_ours < u32::MAX / 2 {
                out.push(SessionEvent::PhysicalInput);
            }
        }
        out
    }
}
