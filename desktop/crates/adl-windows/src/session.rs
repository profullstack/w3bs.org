//! Session events and the emergency-stop hotkey, from a hidden window on its own thread.
//!
//! The window is a hidden *top-level* window rather than a message-only one: message-only
//! windows (`HWND_MESSAGE`) never receive broadcasts, and `WM_DISPLAYCHANGE`,
//! `WM_POWERBROADCAST` and `WM_QUERYENDSESSION` are broadcasts. It is never shown and has the
//! tool-window style, so it does not appear in the taskbar, Alt+Tab or `windows()`.
//!
//! Received: `WM_WTSSESSION_CHANGE` (lock, unlock, logoff, console/remote disconnect),
//! `WM_POWERBROADCAST` (suspend, resume), `WM_DISPLAYCHANGE`, `WM_QUERYENDSESSION`, and
//! `WM_HOTKEY` for the emergency stop.
//!
//! Physical pointer use is detected with a low-level *mouse* hook (`WH_MOUSE_LL`) that looks at
//! one bit only: whether Windows marked the event as injected (`LLMHF_INJECTED`). A non-injected
//! event is the human's hand on a real mouse and becomes `SessionEvent::PhysicalInput` (at most
//! once per 250 ms); positions and buttons are not recorded. No keyboard hook is installed, so
//! keystrokes are never observed; the human's keyboard use is seen only through
//! `GetLastInputInfo` in the adapter, which cannot tell it apart from injected input while the
//! agent is typing.

use crate::win;
use adl_broker::SessionEvent;
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::RemoteDesktop::{WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION};
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Emergency-stop chords, tried in order until one registers. Win+Alt+Esc mirrors the Linux
/// tray's Super+Alt+Escape and is not reserved by Windows; Ctrl+Alt+Shift+F12 is the fallback if
/// another program already holds it. (Ctrl+Alt+Del and Ctrl+Shift+Esc are reserved by the OS and
/// cannot be registered at all.)
pub const HOTKEYS: &[(u32, u32, &str)] = &[
    (MOD_WIN.0 | MOD_ALT.0, 0x1B, "Win+Alt+Esc"),
    (MOD_CONTROL.0 | MOD_ALT.0 | MOD_SHIFT.0, 0x7B, "Ctrl+Alt+Shift+F12"),
];
pub const HOTKEY_ID: i32 = 0x5733; // "W3"

/// Register the first free emergency chord on `hwnd`; returns its description.
pub fn register_hotkey(hwnd: HWND) -> Option<&'static str> {
    for (mods, vk, desc) in HOTKEYS {
        // SAFETY: plain call; WM_HOTKEY goes to `hwnd`.
        if unsafe { RegisterHotKey(Some(hwnd), HOTKEY_ID, HOT_KEY_MODIFIERS(*mods) | MOD_NOREPEAT, *vk) }.is_ok() {
            return Some(desc);
        }
    }
    None
}

/// Description of the chord the session thread registered, if any.
pub fn hotkey_description() -> Option<&'static str> {
    SHARED.get().and_then(|s| *s.hotkey.lock().unwrap_or_else(|e| e.into_inner()))
}

pub struct Shared {
    pub events: Mutex<Vec<SessionEvent>>,
    pub locked: Mutex<bool>,
    pub hotkey: Mutex<Option<&'static str>>,
    pub notifications: Mutex<bool>,
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

fn push(e: SessionEvent) {
    if let Some(s) = SHARED.get() {
        if matches!(e, SessionEvent::Locked) {
            *s.locked.lock().unwrap_or_else(|e| e.into_inner()) = true;
        }
        if matches!(e, SessionEvent::Unlocked) {
            *s.locked.lock().unwrap_or_else(|e| e.into_inner()) = false;
        }
        s.events.lock().unwrap_or_else(|e| e.into_inner()).push(e);
    }
}

static LAST_PHYSICAL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

unsafe extern "system" fn mouse_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 && lp.0 != 0 {
        // SAFETY: for WH_MOUSE_LL, lParam points at an MSLLHOOKSTRUCT.
        let flags = unsafe { (*(lp.0 as *const MSLLHOOKSTRUCT)).flags };
        if flags & (LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED) == 0 {
            let now = unsafe { windows::Win32::System::SystemInformation::GetTickCount() };
            let last = LAST_PHYSICAL.load(std::sync::atomic::Ordering::Relaxed);
            if now.wrapping_sub(last) > 250 {
                LAST_PHYSICAL.store(now, std::sync::atomic::Ordering::Relaxed);
                push(SessionEvent::PhysicalInput);
            }
        }
    }
    // SAFETY: always pass the event on.
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_WTSSESSION_CHANGE => match wp.0 as u32 {
            WTS_SESSION_LOCK => push(SessionEvent::Locked),
            WTS_SESSION_UNLOCK => push(SessionEvent::Unlocked),
            WTS_SESSION_LOGOFF => push(SessionEvent::LoggingOut),
            // The session stopped being the one on screen (fast user switching, RDP drop).
            WTS_CONSOLE_DISCONNECT | WTS_REMOTE_DISCONNECT => push(SessionEvent::Locked),
            WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT => {}
            _ => {}
        },
        WM_POWERBROADCAST => match wp.0 as u32 {
            PBT_APMSUSPEND => push(SessionEvent::Suspending),
            PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => push(SessionEvent::Resumed),
            _ => {}
        },
        WM_DISPLAYCHANGE => push(SessionEvent::LayoutChanged),
        WM_QUERYENDSESSION => {
            push(SessionEvent::LoggingOut);
            return LRESULT(1);
        }
        WM_HOTKEY if wp.0 as i32 == HOTKEY_ID => push(SessionEvent::EmergencyStop),
        _ => {}
    }
    // SAFETY: default handling for everything else.
    unsafe { DefWindowProcW(h, msg, wp, lp) }
}

/// Start the session thread once per process. Returns the shared state.
pub fn start() -> Arc<Shared> {
    SHARED
        .get_or_init(|| {
            let shared = Arc::new(Shared { events: Mutex::new(vec![]), locked: Mutex::new(false), hotkey: Mutex::new(None), notifications: Mutex::new(false) });
            let (tx, rx) = mpsc::channel::<()>();
            let s2 = shared.clone();
            std::thread::Builder::new()
                .name("adl-windows-session".into())
                .spawn(move || run(s2, tx))
                .expect("spawn session thread");
            let _ = rx.recv_timeout(std::time::Duration::from_secs(5));
            shared
        })
        .clone()
}

fn run(shared: Arc<Shared>, ready: mpsc::Sender<()>) {
    win::init_dpi_awareness();
    // SAFETY: standard window-class registration and message loop on this thread.
    unsafe {
        let hinst = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let class = w!("W3bsDesktopSession");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst.into(), lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        let hwnd = match CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class, w!("W3BS desktop session"), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst.into()), None) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("adl-windows: session window unavailable ({e}); lock/suspend events and the emergency hotkey are off");
                let _ = ready.send(());
                return;
            }
        };
        *shared.notifications.lock().unwrap_or_else(|e| e.into_inner()) = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION).is_ok();
        let hk = register_hotkey(hwnd);
        if hk.is_none() {
            eprintln!("adl-windows: no emergency-stop chord could be registered (all taken)");
        }
        *shared.hotkey.lock().unwrap_or_else(|e| e.into_inner()) = hk;
        if let Err(e) = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(hinst.into()), 0) {
            eprintln!("adl-windows: physical mouse detection unavailable ({e})");
        }
        let _ = ready.send(());
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
