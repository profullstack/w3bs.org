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

#[cfg(windows)]
pub fn parent_of(_pid: u32) -> Option<u32> {
    // Implemented in the Windows port (Toolhelp32 snapshot).
    None
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

#[cfg(windows)]
pub fn has_tty(_pid: u32) -> bool {
    // Implemented in the Windows port. Unknown counts as non-interactive.
    false
}
