//! What the OS says about the process on the other end of a local connection.
//!
//! This is defence in depth, not a sandbox: a hostile process running as the same user can do
//! anything the user can (PRD section 11). It does stop an agent's ordinary tool calls from
//! enabling the layer or approving its own requests through the human control endpoint.

#[cfg(unix)]
use crate::transport::PeerInfo;

#[cfg(target_os = "linux")]
pub fn peer_of(s: &std::os::unix::net::UnixStream) -> Option<PeerInfo> {
    use std::os::fd::AsRawFd;
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: valid fd, correctly sized out-parameter.
    let rc = unsafe { libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) };
    (rc == 0 && cred.pid > 0).then_some(PeerInfo { pid: cred.pid as u32, same_user: cred.uid == my_uid() })
}

#[cfg(target_os = "macos")]
pub fn peer_of(s: &std::os::unix::net::UnixStream) -> Option<PeerInfo> {
    use std::os::fd::AsRawFd;
    let fd = s.as_raw_fd();
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: valid fd and out-parameters.
    if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
        return None;
    }
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: LOCAL_PEERPID fills a pid_t.
    let rc = unsafe { libc::getsockopt(fd, libc::SOL_LOCAL, libc::LOCAL_PEERPID, (&mut pid as *mut libc::pid_t).cast(), &mut len) };
    (rc == 0 && pid > 0).then_some(PeerInfo { pid: pid as u32, same_user: uid == my_uid() })
}

#[cfg(unix)]
pub fn my_uid() -> u32 {
    // SAFETY: getuid cannot fail.
    unsafe { libc::getuid() }
}

/// Parent pid of a process.
#[cfg(target_os = "linux")]
pub fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 2..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(target_os = "macos")]
fn bsdinfo(pid: u32) -> Option<libc::proc_bsdinfo> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: buffer is exactly one proc_bsdinfo.
    let n = unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size) };
    (n == size).then_some(info)
}

#[cfg(target_os = "macos")]
pub fn parent_of(pid: u32) -> Option<u32> {
    bsdinfo(pid).map(|i| i.pbi_ppid).filter(|p| *p > 0)
}

/// Parent pid of a process (Toolhelp32 snapshot). Windows does not re-parent orphans, so a
/// recorded parent may have exited (and its pid been reused); `lineage` stops at cycles.
#[cfg(windows)]
pub fn parent_of(pid: u32) -> Option<u32> {
    win::processes().get(&pid).map(|p| p.0).filter(|p| *p != 0 && *p != pid)
}

/// The pid itself followed by its ancestors, stopping at init/launchd.
pub fn lineage(pid: u32) -> Vec<u32> {
    let mut out = vec![pid];
    let mut cur = pid;
    while let Some(p) = parent_of(cur) {
        if p <= 1 || out.contains(&p) || out.len() > 64 {
            break;
        }
        out.push(p);
        cur = p;
    }
    out
}

/// Whether the process has an interactive terminal.
#[cfg(target_os = "linux")]
pub fn has_tty(pid: u32) -> bool {
    std::fs::read_link(format!("/proc/{pid}/fd/0"))
        .map(|p| {
            let s = p.to_string_lossy();
            s.starts_with("/dev/pts/") || s.starts_with("/dev/tty")
        })
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
pub fn has_tty(pid: u32) -> bool {
    // A controlling terminal device is recorded in the BSD info (NODEV = u32::MAX when none).
    bsdinfo(pid).is_some_and(|i| i.e_tdev != u32::MAX && i.e_tdev != 0)
}

/// Whether the process was started from the person's interactive desktop.
///
/// Windows has no controlling terminal to look up, so this is a heuristic, and unknown answers
/// count as non-interactive: the process must run in the broker's own session (not session 0,
/// not another user's) and its ancestry, walked up through the Toolhelp32 snapshot, must reach
/// the interactive shell (`explorer.exe`) or a terminal emulator (Windows Terminal, conhost
/// started directly, Alacritty, WezTerm, mintty) before anything else ends the chain. Services,
/// scheduled tasks, SSH sessions (`sshd.exe`) and CI runners do not qualify.
#[cfg(windows)]
pub fn has_tty(pid: u32) -> bool {
    const ROOTS: &[&str] = &["explorer.exe", "windowsterminal.exe", "wt.exe", "conhost.exe", "openconsole.exe", "alacritty.exe", "wezterm-gui.exe", "mintty.exe"];
    const REMOTE: &[&str] = &["sshd.exe", "services.exe", "svchost.exe", "wininit.exe", "winlogon.exe"];
    if win::session_of(pid).is_none_or(|s| s == 0 || Some(s) != win::session_of(std::process::id())) {
        return false;
    }
    let procs = win::processes();
    let mut cur = pid;
    for _ in 0..32 {
        let Some((parent, exe)) = procs.get(&cur) else { return false };
        let exe = exe.to_ascii_lowercase();
        if cur != pid && ROOTS.contains(&exe.as_str()) {
            return true;
        }
        if REMOTE.contains(&exe.as_str()) || *parent == 0 || *parent == cur {
            return false;
        }
        cur = *parent;
    }
    false
}

#[cfg(windows)]
mod win {
    use std::collections::HashMap;
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};

    /// pid -> (parent pid, executable name), from one snapshot.
    pub fn processes() -> HashMap<u32, (u32, String)> {
        let mut out = HashMap::new();
        // SAFETY: snapshot handle owned and closed below; entries are correctly sized.
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
            let snap = adl_windows::win::Owned(snap);
            let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut ok = Process32FirstW(snap.0, &mut e).is_ok();
            while ok {
                out.insert(e.th32ProcessID, (e.th32ParentProcessID, adl_windows::win::wstr(&e.szExeFile)));
                ok = Process32NextW(snap.0, &mut e).is_ok();
            }
        }
        out
    }

    pub fn session_of(pid: u32) -> Option<u32> {
        let mut s = 0u32;
        // SAFETY: out-parameter.
        unsafe { windows::Win32::System::RemoteDesktop::ProcessIdToSessionId(pid, &mut s) }.ok().map(|_| s)
    }
}
