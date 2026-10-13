//! Output for the GUI-subsystem executable (`w3bs-desktopw`), which Windows starts with no
//! console and no standard handles. Writes to absent handles are discarded by Rust's standard
//! library rather than failing, but the broker's and tray's diagnostics are worth keeping, so
//! they go to a per-command log file instead. Elsewhere this does nothing.

use std::path::Path;

/// Keep at most this much of a log; the previous one is kept as `<name>.log.1`.
#[cfg_attr(not(windows), allow(dead_code))]
const MAX_LOG: u64 = 1024 * 1024;

#[cfg(windows)]
pub fn redirect_output(dir: &Path, name: &str) {
    use std::io::Write;
    use std::os::windows::io::IntoRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{GetStdHandle, SetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};

    // A caller that redirected our output (a test harness, `Start-Process -Redirect...`) keeps it.
    // SAFETY: querying this process's standard handles.
    let present = |id| unsafe { GetStdHandle(id) }.is_ok_and(|h| !h.is_invalid() && !h.0.is_null());
    if present(STD_ERROR_HANDLE) && present(STD_OUTPUT_HANDLE) {
        return;
    }
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let path = dir.join(format!("{name}.log"));
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG) {
        let _ = std::fs::rename(&path, dir.join(format!("{name}.log.1")));
    }
    let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else { return };
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let _ = writeln!(file, "--- w3bs-desktopw {name} {} pid {} started (unix time {secs})", env!("CARGO_PKG_VERSION"), std::process::id());
    // The handle lives for the rest of the process; Rust's stdout/stderr look the standard
    // handles up on every write, so both now land in the file.
    let h = HANDLE(file.into_raw_handle());
    // SAFETY: `h` is an open file handle owned by this process for its whole lifetime.
    unsafe {
        let _ = SetStdHandle(STD_OUTPUT_HANDLE, h);
        let _ = SetStdHandle(STD_ERROR_HANDLE, h);
    }
}

#[cfg(not(windows))]
pub fn redirect_output(_dir: &Path, _name: &str) {}
