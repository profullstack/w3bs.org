//! What the OS says about the process on the other end of a Unix socket.
//!
//! This is defence in depth, not a sandbox: a hostile process running as the same user can do
//! anything the user can (PRD section 11). It does stop an agent's ordinary tool calls from
//! enabling the layer or approving its own requests through the human control socket.

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

#[derive(Clone, Copy, Debug)]
pub struct Cred {
    pub pid: u32,
    pub uid: u32,
}

#[cfg(target_os = "linux")]
pub fn peer_cred(s: &UnixStream) -> Option<Cred> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: valid fd, correctly sized out-parameter.
    let rc = unsafe { libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) };
    (rc == 0 && cred.pid > 0).then_some(Cred { pid: cred.pid as u32, uid: cred.uid })
}

#[cfg(not(target_os = "linux"))]
pub fn peer_cred(_s: &UnixStream) -> Option<Cred> {
    None
}

pub fn my_uid() -> u32 {
    // SAFETY: getuid cannot fail.
    unsafe { libc::getuid() }
}

/// Parent pid from /proc/<pid>/stat (field 4, after the parenthesised command name).
pub fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 2..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// The pid itself followed by its ancestors, stopping at init.
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

/// Whether the process's stdin is a terminal device.
pub fn has_tty(pid: u32) -> bool {
    std::fs::read_link(format!("/proc/{pid}/fd/0"))
        .map(|p| {
            let s = p.to_string_lossy();
            s.starts_with("/dev/pts/") || s.starts_with("/dev/tty")
        })
        .unwrap_or(false)
}
