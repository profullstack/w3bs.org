//! Small safe wrappers over the Win32 calls the adapter (and the CLI's transport/tray) share.

use std::ffi::c_void;
use windows::core::{BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Security::{GetTokenInformation, TokenIntegrityLevel, TokenUser, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_USER};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Make the whole process per-monitor DPI aware (v2) so every API reports physical pixels.
/// Idempotent; returns false if the awareness was already fixed differently (e.g. by a manifest).
pub fn init_dpi_awareness() -> bool {
    use std::sync::OnceLock;
    static DONE: OnceLock<bool> = OnceLock::new();
    // SAFETY: plain process-wide setting.
    *DONE.get_or_init(|| unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }.is_ok())
}

/// Initialize COM (multithreaded apartment) on the calling thread, once per thread.
pub fn ensure_com() {
    thread_local!(static INIT: () = {
        // SAFETY: initializing COM for this thread; S_FALSE / RPC_E_CHANGED_MODE are fine (some
        // apartment is already set up).
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    });
    INIT.with(|_| {});
}

/// An owned kernel handle closed on drop.
pub struct Owned(pub HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_invalid() && !self.0 .0.is_null() {
            // SAFETY: we own the handle.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

// SAFETY: kernel handles may be used from any thread.
unsafe impl Send for Owned {}
unsafe impl Sync for Owned {}

pub fn hwnd_of(id: isize) -> HWND {
    HWND(id as *mut c_void)
}

pub fn hwnd_id(h: HWND) -> isize {
    h.0 as isize
}

fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

pub fn window_title(h: HWND) -> String {
    let mut buf = [0u16; 512];
    // SAFETY: valid buffer.
    let n = unsafe { GetWindowTextW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: valid buffer.
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn window_pid(h: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: out-parameter.
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    pid
}

pub fn open_process(pid: u32) -> Option<Owned> {
    // SAFETY: plain call; the handle is owned by the returned value.
    unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok().map(Owned)
}

/// Executable file name of a process, e.g. `notepad.exe`.
pub fn process_image_name(pid: u32) -> Option<String> {
    let p = open_process(pid)?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: buffer and length are consistent.
    unsafe { QueryFullProcessImageNameW(p.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) }.ok()?;
    let full = String::from_utf16_lossy(&buf[..len as usize]);
    Some(full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string())
}

fn token_info(process: HANDLE, class: windows::Win32::Security::TOKEN_INFORMATION_CLASS) -> Option<Vec<u8>> {
    let mut tok = HANDLE::default();
    // SAFETY: out-parameter; closed by Owned.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut tok) }.ok()?;
    let tok = Owned(tok);
    let mut len = 0u32;
    // SAFETY: size query, then a correctly sized buffer.
    let _ = unsafe { GetTokenInformation(tok.0, class, None, 0, &mut len) };
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    unsafe { GetTokenInformation(tok.0, class, Some(buf.as_mut_ptr().cast()), len, &mut len) }.ok()?;
    Some(buf)
}

/// The token user SID of a process, as bytes (comparable with `==`).
pub fn process_user_sid(process: HANDLE) -> Option<Vec<u8>> {
    let buf = token_info(process, TokenUser)?;
    // SAFETY: the buffer holds a TOKEN_USER whose SID points inside it.
    unsafe {
        let tu = &*(buf.as_ptr() as *const TOKEN_USER);
        sid_bytes(tu.User.Sid.0)
    }
}

/// Copy a SID's bytes: revision, count, 6-byte authority, then `count` u32 sub-authorities.
///
/// # Safety
/// `sid` must point at a valid SID.
pub unsafe fn sid_bytes(sid: *mut c_void) -> Option<Vec<u8>> {
    if sid.is_null() {
        return None;
    }
    let p = sid as *const u8;
    let count = unsafe { *p.add(1) } as usize;
    let len = 8 + 4 * count;
    Some(unsafe { std::slice::from_raw_parts(p, len) }.to_vec())
}

pub fn current_user_sid() -> Option<Vec<u8>> {
    // SAFETY: pseudo handle of the current process.
    process_user_sid(unsafe { GetCurrentProcess() })
}

/// Standard string form (`S-1-5-21-...`) of SID bytes.
pub fn sid_string(sid: &[u8]) -> String {
    if sid.len() < 8 {
        return String::new();
    }
    let auth = sid[2..8].iter().fold(0u64, |a, b| (a << 8) | *b as u64);
    let mut s = format!("S-{}-{}", sid[0], auth);
    for c in sid[8..].as_chunks::<4>().0 {
        s += &format!("-{}", u32::from_le_bytes(*c));
    }
    s
}

/// Mandatory integrity level RID of a process (0x2000 medium, 0x3000 high, 0x4000 system).
pub fn process_integrity(process: HANDLE) -> Option<u32> {
    let buf = token_info(process, TokenIntegrityLevel)?;
    // SAFETY: TOKEN_MANDATORY_LABEL with a SID inside the buffer; the RID is the last
    // sub-authority.
    unsafe {
        let tml = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let sid = sid_bytes(tml.Label.Sid.0)?;
        let last = sid.len().checked_sub(4)?;
        Some(u32::from_le_bytes(sid[last..].try_into().ok()?))
    }
}

pub fn current_integrity() -> Option<u32> {
    // SAFETY: pseudo handle of the current process.
    process_integrity(unsafe { GetCurrentProcess() })
}

pub fn integrity_name(rid: u32) -> &'static str {
    match rid {
        r if r >= 0x4000 => "system",
        r if r >= 0x3000 => "high (elevated)",
        r if r >= 0x2000 => "medium",
        r if r >= 0x1000 => "low",
        _ => "untrusted",
    }
}

pub fn is_cloaked(h: HWND) -> bool {
    let mut v = 0u32;
    // SAFETY: u32 out-parameter of the documented size.
    unsafe { DwmGetWindowAttribute(h, DWMWA_CLOAKED, (&mut v as *mut u32).cast(), 4) }.is_ok() && v != 0
}

/// The visible frame of a top-level window (no invisible resize borders), physical pixels.
pub fn frame_bounds(h: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    // SAFETY: RECT out-parameter of the documented size.
    if unsafe { DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, (&mut r as *mut RECT).cast(), std::mem::size_of::<RECT>() as u32) }.is_ok() {
        return Some(r);
    }
    unsafe { GetWindowRect(h, &mut r) }.ok().map(|_| r)
}

pub fn is_visible(h: HWND) -> bool {
    // SAFETY: plain query.
    unsafe { IsWindowVisible(h) }.as_bool()
}

pub fn is_minimized(h: HWND) -> bool {
    // SAFETY: plain query.
    unsafe { IsIconic(h) }.as_bool()
}

pub fn ex_style(h: HWND) -> WINDOW_EX_STYLE {
    // SAFETY: plain query.
    WINDOW_EX_STYLE(unsafe { GetWindowLongPtrW(h, GWL_EXSTYLE) } as u32)
}

/// Top-level windows, topmost first (EnumWindows walks the Z order from the top).
pub fn top_level_windows() -> Vec<HWND> {
    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> BOOL {
        // SAFETY: lparam is the Vec passed below, alive for the duration of EnumWindows.
        let v = unsafe { &mut *(l.0 as *mut Vec<HWND>) };
        v.push(h);
        BOOL(1)
    }
    let mut v: Vec<HWND> = Vec::new();
    // SAFETY: the callback only pushes into `v`.
    let _ = unsafe { EnumWindows(Some(cb), LPARAM(&mut v as *mut Vec<HWND> as isize)) };
    v
}

pub fn foreground() -> Option<HWND> {
    // SAFETY: plain query.
    let h = unsafe { GetForegroundWindow() };
    (!h.0.is_null()).then_some(h)
}

/// The top-level window under a physical virtual-screen point.
pub fn root_at(x: i32, y: i32) -> Option<HWND> {
    // SAFETY: plain queries.
    let h = unsafe { WindowFromPoint(POINT { x, y }) };
    if h.0.is_null() {
        return None;
    }
    let root = unsafe { GetAncestor(h, GA_ROOT) };
    Some(if root.0.is_null() { h } else { root })
}

pub fn cursor_pos() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    // SAFETY: out-parameter.
    unsafe { GetCursorPos(&mut p) }.ok().map(|_| (p.x, p.y))
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn wstr(buf: &[u16]) -> String {
    wide_to_string(buf)
}
