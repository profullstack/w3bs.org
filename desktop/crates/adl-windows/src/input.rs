//! `SendInput` delivery. Every call checks that Windows accepted every event; a short count is
//! an error, never success. UIPI drops input to higher-integrity windows *without* any error from
//! `SendInput`, so the target's integrity level is checked before each event instead.

use crate::map::{self, TextKey, VirtualScreen};
use crate::win;
use adl_schema::*;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{GetLastError, HWND};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Pause after each typed character so the target's message queue keeps up and a disable can
/// land between characters (a 3000-character type takes about 6 s).
pub const CHAR_PACE: Duration = Duration::from_millis(2);

pub fn virtual_screen() -> VirtualScreen {
    // SAFETY: plain metric queries.
    unsafe {
        VirtualScreen {
            x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: GetSystemMetrics(SM_YVIRTUALSCREEN),
            width: GetSystemMetrics(SM_CXVIRTUALSCREEN),
            height: GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

fn send(inputs: &[INPUT]) -> Result<()> {
    // SAFETY: a slice of initialized INPUT structures.
    let n = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if n as usize != inputs.len() {
        // SAFETY: plain call.
        let err = unsafe { GetLastError() };
        return Err(AdlError::new(
            ErrorCode::OutcomeUnknown,
            format!("SendInput accepted {n} of {} events (error {}); input may be blocked by another desktop or a higher-integrity process", inputs.len(), err.0),
        ));
    }
    Ok(())
}

fn mouse(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
}

fn keybd(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
}

/// Refuse to send input that UIPI would silently drop: a target window owned by a process with a
/// higher integrity level than ours (an elevated app while the broker is not elevated).
pub fn check_uipi(target: Option<HWND>, what: &str) -> Result<()> {
    let Some(h) = target else { return Ok(()) };
    let pid = win::window_pid(h);
    // SAFETY: plain call.
    if pid == std::process::id() {
        return Ok(());
    }
    let mine = win::current_integrity().unwrap_or(0x2000);
    let theirs = match win::open_process(pid).and_then(|p| win::process_integrity(p.0)) {
        Some(v) => v,
        None => {
            return Err(AdlError::denied(format!(
                "cannot read the integrity level of the {what} ({}, pid {pid}); Windows may block input to it, so it is refused",
                win::window_title(h)
            )))
        }
    };
    if theirs > mine {
        return Err(AdlError::denied(format!(
            "the {what} ({}) runs at {} integrity and the broker at {}; Windows (UIPI) silently drops input to it. Run the broker elevated to drive elevated apps",
            win::window_title(h),
            win::integrity_name(theirs),
            win::integrity_name(mine)
        )));
    }
    Ok(())
}

/// Absolute pointer motion to a physical virtual-screen pixel, then confirm where the cursor
/// landed (and correct once if a driver mapped the normalized value differently).
pub fn move_to(x: i32, y: i32) -> Result<()> {
    let vs = virtual_screen();
    if !vs.contains(x, y) {
        return Err(AdlError::invalid(format!("({x}, {y}) is outside the virtual screen")));
    }
    let flags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
    let (mut nx, mut ny) = vs.normalize(x, y);
    let mut last = None;
    for attempt in 0..3 {
        send(&[mouse(nx, ny, 0, flags)])?;
        let deadline = Instant::now() + Duration::from_millis(60);
        loop {
            last = win::cursor_pos();
            if last == Some((x, y)) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        // Nudge by the observed error (in normalized units) and try again.
        if let Some((ax, ay)) = last
            && attempt < 2
        {
            nx = (nx + ((x - ax) as i64 * 65536 / vs.width.max(1) as i64) as i32).clamp(0, 65535);
            ny = (ny + ((y - ay) as i64 * 65536 / vs.height.max(1) as i64) as i32).clamp(0, 65535);
        }
    }
    Err(AdlError::new(ErrorCode::OutcomeUnknown, format!("pointer landed at {last:?}, not ({x}, {y}); it may be clipped or on a secure desktop")))
}

pub fn button(b: Button, pressed: bool) -> Result<()> {
    let (flags, data) = match (b, pressed) {
        (Button::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (Button::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
        (Button::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (Button::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (Button::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (Button::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (Button::Back, true) => (MOUSEEVENTF_XDOWN, XBUTTON1 as u32),
        (Button::Back, false) => (MOUSEEVENTF_XUP, XBUTTON1 as u32),
        (Button::Forward, true) => (MOUSEEVENTF_XDOWN, XBUTTON2 as u32),
        (Button::Forward, false) => (MOUSEEVENTF_XUP, XBUTTON2 as u32),
    };
    send(&[mouse(0, 0, data, flags)])
}

pub fn scroll(axis: Axis, steps: i32) -> Result<()> {
    let vertical = axis == Axis::Vertical;
    let delta = map::wheel_delta(vertical, steps);
    send(&[mouse(0, 0, delta as u32, if vertical { MOUSEEVENTF_WHEEL } else { MOUSEEVENTF_HWHEEL })])
}

fn scan_code(vk: u16) -> u16 {
    // SAFETY: plain call.
    unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) as u16 }
}

/// Resolve a portable key to (virtual key, extended).
pub fn resolve_key(key: &Key) -> Result<(u16, bool)> {
    match map::named_vk(key).map_err(AdlError::unsupported)? {
        Some(v) => Ok((v.code, v.extended)),
        None => {
            let Key::Char(c) = key else { unreachable!("named_vk resolves everything but characters") };
            let mut buf = [0u16; 2];
            let units = c.encode_utf16(&mut buf);
            if units.len() != 1 {
                return Err(AdlError::unsupported(format!("{c:?} has no key on the active keyboard layout; type it as text instead")));
            }
            // SAFETY: plain call against the calling thread's keyboard layout.
            let r = unsafe { VkKeyScanW(units[0]) };
            if r == -1 {
                return Err(AdlError::unsupported(format!("{c:?} has no key on the active keyboard layout; type it as text instead")));
            }
            Ok(((r as u16) & 0xff, false))
        }
    }
}

pub fn key(k: &Key, pressed: bool) -> Result<()> {
    let (vk, extended) = resolve_key(k)?;
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !pressed {
        flags |= KEYEVENTF_KEYUP;
    }
    send(&[keybd(vk, scan_code(vk), flags)])
}

/// Insert text with `KEYEVENTF_UNICODE` (layout independent), one character per `SendInput`
/// call so a surrogate pair is never split, with a short pace between characters. `before_each`
/// runs before every character (UIPI / interlock re-check); an error stops the text there.
pub fn text(s: &str, mut before_each: impl FnMut() -> Result<()>) -> Result<()> {
    for c in map::text_chars(s) {
        before_each()?;
        let events: Vec<INPUT> = map::char_events(c)
            .into_iter()
            .map(|e| match e {
                TextKey::Unit { unit, down } => keybd(0, unit, if down { KEYEVENTF_UNICODE } else { KEYEVENTF_UNICODE | KEYEVENTF_KEYUP }),
                TextKey::Vk { code, down } => keybd(code, scan_code(code), if down { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_KEYUP }),
            })
            .collect();
        send(&events)?;
        std::thread::sleep(CHAR_PACE);
    }
    Ok(())
}
