//! Local IPC transport, one interface per OS (PRD section 9):
//! Unix-domain sockets on Linux and macOS (0600 inside a 0700 per-user directory), named pipes on
//! Windows (per-user pipe name, DACL limited to the current user). Every accepted connection
//! carries what the OS says about the peer, so the server never trusts a caller-supplied identity.

use std::io::{self, Read, Write};

/// What the OS reports about the process on the other end.
#[derive(Clone, Copy, Debug)]
pub struct PeerInfo {
    pub pid: u32,
    /// The peer runs as the same user as the broker.
    pub same_user: bool,
}

/// A connected, bidirectional stream. Clones share the connection so one thread can read while
/// another writes (events and responses interleave on control connections).
pub trait Duplex: Read + Write + Send {
    fn try_clone_box(&self) -> io::Result<Box<dyn Duplex>>;
}

/// Where a socket or pipe lives. Built by `paths`.
#[derive(Clone, Debug)]
pub struct Endpoint {
    /// Unix socket path, or the full Windows pipe name (`\\.\pipe\...`).
    pub name: std::path::PathBuf,
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name.display())
    }
}

pub use imp::{connect, Listener};

/// How long a tray waits for the broker before giving up. At login the tray autostart and the
/// broker service start in the same second, so the control socket is often not there yet.
pub const BROKER_START_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// `connect`, but waits up to `wait` while the broker is not listening yet (endpoint missing, or a
/// stale socket refusing). Any other error returns at once.
pub fn connect_waiting(ep: &Endpoint, wait: std::time::Duration) -> io::Result<Box<dyn Duplex>> {
    retry_while_starting(wait, || connect(ep))
}

fn retry_while_starting<T>(wait: std::time::Duration, mut attempt: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let deadline = std::time::Instant::now() + wait;
    let mut delay = std::time::Duration::from_millis(100);
    loop {
        match attempt() {
            Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => {
                let now = std::time::Instant::now();
                if now >= deadline {
                    return Err(e);
                }
                std::thread::sleep(delay.min(deadline - now));
                delay = (delay * 2).min(std::time::Duration::from_secs(2));
            }
            r => return r,
        }
    }
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn retries_until_the_broker_listens() {
        let mut n = 0;
        let r = retry_while_starting(Duration::from_secs(5), || {
            n += 1;
            if n < 3 { Err(io::Error::from(io::ErrorKind::NotFound)) } else { Ok(n) }
        });
        assert_eq!(r.unwrap(), 3);
    }

    #[test]
    fn gives_up_after_the_wait() {
        let r: io::Result<()> = retry_while_starting(Duration::from_millis(250), || Err(io::Error::from(io::ErrorKind::ConnectionRefused)));
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::ConnectionRefused);
    }

    #[test]
    fn other_errors_are_not_retried() {
        let mut n = 0;
        let r: io::Result<()> = retry_while_starting(Duration::from_secs(5), || {
            n += 1;
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        });
        assert_eq!((r.unwrap_err().kind(), n), (io::ErrorKind::PermissionDenied, 1));
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};

    impl Duplex for UnixStream {
        fn try_clone_box(&self) -> io::Result<Box<dyn Duplex>> {
            Ok(Box::new(self.try_clone()?))
        }
    }

    pub struct Listener(UnixListener);

    impl Listener {
        /// Bind, refusing to steal an endpoint a live broker is serving, and restrict it to the user.
        pub fn bind(ep: &Endpoint) -> io::Result<Self> {
            let path = &ep.name;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            }
            if path.exists() {
                if UnixStream::connect(path).is_ok() {
                    return Err(io::Error::new(io::ErrorKind::AddrInUse, format!("{} is already served by a running broker", path.display())));
                }
                std::fs::remove_file(path)?;
            }
            let l = UnixListener::bind(path)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            Ok(Self(l))
        }

        pub fn accept(&self) -> io::Result<(Box<dyn Duplex>, Option<PeerInfo>)> {
            let (s, _) = self.0.accept()?;
            let peer = crate::peer::peer_of(&s);
            Ok((Box::new(s), peer))
        }

        pub fn remove(ep: &Endpoint) {
            let _ = std::fs::remove_file(&ep.name);
        }
    }

    pub fn connect(ep: &Endpoint) -> io::Result<Box<dyn Duplex>> {
        Ok(Box::new(UnixStream::connect(&ep.name)?))
    }
}

#[cfg(windows)]
#[path = "transport_windows.rs"]
mod imp;
