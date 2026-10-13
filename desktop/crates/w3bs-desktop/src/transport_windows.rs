//! Windows named-pipe transport.
//!
//! * **Private to the user.** Every instance is created with the security descriptor
//!   `O:<user>D:P(A;;GA;;;<user>)`: owned by the current user's SID, a protected DACL granting
//!   only that SID (no inherited ACEs, no Administrators, no SYSTEM), and
//!   `PIPE_REJECT_REMOTE_CLIENTS` so SMB clients are refused.
//! * **No squatting.** The first instance uses `FILE_FLAG_FIRST_PIPE_INSTANCE`: if anyone else
//!   created the name first (or another broker serves it), `bind` fails instead of joining their
//!   pipe. Clients check the other direction: they refuse a pipe whose owner SID is not their
//!   own, so a pipe pre-created by another user never receives an agent token.
//! * **Concurrent clients.** A fresh instance is created as soon as one connects, so clients are
//!   served in parallel (`PIPE_UNLIMITED_INSTANCES`).
//! * **Full duplex.** Handles are opened with `FILE_FLAG_OVERLAPPED` and every read and write
//!   waits on its own `OVERLAPPED` + event. Synchronous pipe handles serialize I/O per handle, so
//!   a blocked read would stall a write from another thread; overlapped handles do not, which is
//!   what lets the server push events on a control connection while a reader thread waits for
//!   the next request (tested in `tests::read_and_write_concurrently`).
//! * **Peer identity.** `GetNamedPipeClientProcessId`, then the client process token's user SID
//!   compared with ours. If the client process cannot be opened, the peer is unknown (`None`),
//!   which the server treats as the most restricted caller.

use super::*;
use adl_windows::win::{self, Owned};
use std::sync::{Arc, Mutex};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT};
use windows::Win32::Security::{OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Pipes::*;
use windows::Win32::System::Threading::CreateEventW;
use windows::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};

const BUF: u32 = 64 * 1024;

fn os_err(e: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(e.code().0 & 0xFFFF)
}

/// One end of a connected pipe. Clones share the handle; each I/O has its own OVERLAPPED.
pub struct Pipe(Arc<Owned>);

impl Pipe {
    fn handle(&self) -> HANDLE {
        self.0 .0
    }

    /// Run one overlapped operation to completion and return the bytes transferred.
    fn overlapped(&self, op: impl FnOnce(*mut OVERLAPPED) -> windows::core::Result<()>) -> io::Result<u32> {
        // SAFETY: the event and OVERLAPPED live until GetOverlappedResult(wait = true) returns,
        // which is after the kernel stops touching them.
        unsafe {
            let ev = Owned(CreateEventW(None, true, false, PCWSTR::null()).map_err(os_err)?);
            let mut ov = OVERLAPPED { hEvent: ev.0, ..Default::default() };
            match op(&mut ov) {
                Ok(()) => {}
                Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {}
                Err(e) => return Err(os_err(e)),
            }
            let mut n = 0u32;
            GetOverlappedResult(self.handle(), &ov, &mut n, true).map_err(os_err)?;
            Ok(n)
        }
    }
}

fn is_closed(e: &io::Error) -> bool {
    matches!(e.raw_os_error().map(|c| WIN32_ERROR(c as u32)), Some(ERROR_BROKEN_PIPE) | Some(ERROR_NO_DATA) | Some(ERROR_PIPE_NOT_CONNECTED))
}

impl Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let h = self.handle();
        let len = buf.len().min(u32::MAX as usize);
        match self.overlapped(|ov| unsafe { ReadFile(h, Some(&mut buf[..len]), None, Some(ov)) }) {
            Ok(n) => Ok(n as usize),
            Err(e) if is_closed(&e) => Ok(0),
            Err(e) => Err(e),
        }
    }
}

impl Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let h = self.handle();
        let len = buf.len().min(u32::MAX as usize);
        match self.overlapped(|ov| unsafe { WriteFile(h, Some(&buf[..len]), None, Some(ov)) }) {
            Ok(n) => Ok(n as usize),
            Err(e) if is_closed(&e) => Err(io::Error::new(io::ErrorKind::BrokenPipe, e)),
            Err(e) => Err(e),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        // Writes complete into the pipe buffer; FlushFileBuffers would block until the other end
        // has read everything, which a subscriber may never do.
        Ok(())
    }
}

impl Duplex for Pipe {
    fn try_clone_box(&self) -> io::Result<Box<dyn Duplex>> {
        Ok(Box::new(Pipe(self.0.clone())))
    }
}

/// The current user's SID, required for the pipe DACL and the owner checks.
fn my_sid() -> io::Result<Vec<u8>> {
    win::current_user_sid().ok_or_else(|| io::Error::other("cannot read the current user's SID"))
}

struct Descriptor(PSECURITY_DESCRIPTOR);

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe {
            let _ = windows::Win32::Foundation::LocalFree(Some(HLOCAL(self.0 .0)));
        }
    }
}

// SAFETY: an immutable self-relative descriptor.
unsafe impl Send for Descriptor {}
unsafe impl Sync for Descriptor {}

fn user_only_descriptor() -> io::Result<Descriptor> {
    let sid = win::sid_string(&my_sid()?);
    let sddl = HSTRING::from(format!("O:{sid}D:P(A;;GA;;;{sid})"));
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: valid SDDL string and out-parameter; freed by Descriptor.
    unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(&sddl, SDDL_REVISION_1, &mut sd, None) }.map_err(os_err)?;
    Ok(Descriptor(sd))
}

pub struct Listener {
    name: HSTRING,
    sd: Descriptor,
    /// The instance waiting for the next client.
    next: Mutex<Option<Owned>>,
}

impl Listener {
    fn instance(&self, first: bool) -> io::Result<Owned> {
        let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: self.sd.0 .0, bInheritHandle: false.into() };
        let mut mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
        if first {
            mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        // SAFETY: valid name and security attributes.
        let h = unsafe { CreateNamedPipeW(&self.name, mode, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS, PIPE_UNLIMITED_INSTANCES, BUF, BUF, 0, Some(&sa)) };
        if h.is_invalid() {
            let e = io::Error::last_os_error();
            if first && e.raw_os_error() == Some(ERROR_ACCESS_DENIED.0 as i32) {
                return Err(io::Error::new(io::ErrorKind::AddrInUse, format!("{} already exists: a broker is already serving it, or another process created it first", self.name)));
            }
            return Err(e);
        }
        Ok(Owned(h))
    }

    /// Create the first instance, refusing a name someone else already holds.
    pub fn bind(ep: &Endpoint) -> io::Result<Self> {
        let name = HSTRING::from(ep.name.as_os_str());
        let l = Listener { name, sd: user_only_descriptor()?, next: Mutex::new(None) };
        let first = l.instance(true)?;
        *l.next.lock().unwrap() = Some(first);
        Ok(l)
    }

    pub fn accept(&self) -> io::Result<(Box<dyn Duplex>, Option<PeerInfo>)> {
        let inst = match self.next.lock().unwrap_or_else(|e| e.into_inner()).take() {
            Some(i) => i,
            None => self.instance(false)?,
        };
        let pipe = Pipe(Arc::new(inst));
        let h = pipe.handle();
        let r = pipe.overlapped(|ov| unsafe { ConnectNamedPipe(h, Some(ov)) });
        // Keep an instance listening for the next client, whatever happened to this one.
        if let Ok(n) = self.instance(false) {
            *self.next.lock().unwrap_or_else(|e| e.into_inner()) = Some(n);
        }
        match r {
            Ok(_) => {}
            // The client connected between CreateNamedPipe and ConnectNamedPipe.
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_CONNECTED.0 as i32) => {}
            Err(e) => return Err(e),
        }
        let peer = peer_of(h);
        Ok((Box::new(pipe), peer))
    }

    /// Pipes disappear with their last handle; nothing to remove.
    pub fn remove(_ep: &Endpoint) {}
}

fn peer_of(h: HANDLE) -> Option<PeerInfo> {
    let mut pid = 0u32;
    // SAFETY: out-parameter on a connected server handle.
    unsafe { GetNamedPipeClientProcessId(h, &mut pid) }.ok()?;
    let proc_ = win::open_process(pid)?;
    let theirs = win::process_user_sid(proc_.0)?;
    Some(PeerInfo { pid, same_user: Some(theirs) == win::current_user_sid() })
}

/// Owner SID of a pipe handle (requires READ_CONTROL, included in GENERIC_READ).
fn owner_sid(h: HANDLE) -> Option<Vec<u8>> {
    let mut owner = PSID::default();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: out-parameters; the descriptor holding the SID is freed after copying it.
    unsafe {
        if GetSecurityInfo(h, SE_KERNEL_OBJECT, OWNER_SECURITY_INFORMATION, Some(&mut owner), None, None, None, Some(&mut sd)) != ERROR_SUCCESS {
            return None;
        }
        let r = win::sid_bytes(owner.0);
        let _ = windows::Win32::Foundation::LocalFree(Some(HLOCAL(sd.0)));
        r
    }
}

pub fn connect(ep: &Endpoint) -> io::Result<Box<dyn Duplex>> {
    let name = HSTRING::from(ep.name.as_os_str());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let h = loop {
        // SAFETY: valid name; the handle is owned below.
        let r = unsafe {
            CreateFileW(
                &name,
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                None,
            )
        };
        match r {
            Ok(h) => break Owned(h),
            Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() && std::time::Instant::now() < deadline => {
                // Every instance is busy for a moment (the broker is creating the next one).
                unsafe {
                    let _ = WaitNamedPipeW(&name, 500);
                }
            }
            Err(e) => return Err(os_err(e)),
        }
    };
    let mine = my_sid()?;
    if owner_sid(h.0).as_deref() != Some(mine.as_slice()) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{name} is not owned by the current user; refusing to talk to it (another user may have created it first)")));
    }
    Ok(Box::new(Pipe(Arc::new(h))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;
    use std::time::Duration;

    fn ep(tag: &str) -> Endpoint {
        Endpoint { name: format!(r"\\.\pipe\w3bs-desktop-test-{}-{tag}", std::process::id()).into() }
    }

    #[test]
    fn read_and_write_concurrently() {
        let e = ep("duplex");
        let l = Listener::bind(&e).unwrap();
        let e2 = e.clone();
        let client = std::thread::spawn(move || connect(&e2).unwrap());
        let (server, peer) = l.accept().unwrap();
        let peer = peer.expect("peer credentials");
        assert_eq!(peer.pid, std::process::id());
        assert!(peer.same_user);
        let mut client = client.join().unwrap();

        // A server thread blocks reading a line that will not arrive until the server has
        // written to the client on another handle clone. With serialized I/O this deadlocks.
        let mut reader = BufReader::new(server.try_clone_box().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            tx.send(line).unwrap();
        });
        std::thread::sleep(Duration::from_millis(200));
        let mut w = server.try_clone_box().unwrap();
        let (wtx, wrx) = mpsc::channel();
        std::thread::spawn(move || {
            w.write_all(b"event from server\n").unwrap();
            wtx.send(()).unwrap();
        });
        wrx.recv_timeout(Duration::from_secs(5)).expect("write blocked behind a pending read on the same pipe");
        let mut got = String::new();
        let mut cr = BufReader::new(client.try_clone_box().unwrap());
        cr.read_line(&mut got).unwrap();
        assert_eq!(got, "event from server\n");
        client.write_all(b"request from client\n").unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "request from client\n");
    }

    #[test]
    fn first_instance_cannot_be_squatted_and_clients_run_in_parallel() {
        let e = ep("first");
        let l = Listener::bind(&e).unwrap();
        let again = Listener::bind(&e);
        assert_eq!(again.err().map(|e| e.kind()), Some(io::ErrorKind::AddrInUse));
        let l = Arc::new(l);
        let l2 = l.clone();
        std::thread::spawn(move || {
            while let Ok((mut s, _)) = l2.accept() {
                std::thread::spawn(move || {
                    let mut r = BufReader::new(s.try_clone_box().unwrap());
                    let mut line = String::new();
                    while r.read_line(&mut line).unwrap_or(0) > 0 {
                        s.write_all(line.as_bytes()).unwrap();
                        line.clear();
                    }
                });
            }
        });
        // Several clients connected at once, each echoed independently.
        let mut clients: Vec<_> = (0..5).map(|_| connect(&e).unwrap()).collect();
        for (i, c) in clients.iter_mut().enumerate() {
            writeln!(c, "client {i}").unwrap();
        }
        for (i, c) in clients.iter_mut().enumerate() {
            let mut line = String::new();
            BufReader::new(c.try_clone_box().unwrap()).read_line(&mut line).unwrap();
            assert_eq!(line, format!("client {i}\n"));
        }
        // EOF propagates when the server side closes.
        drop(clients);
    }

    #[test]
    fn missing_pipe_is_not_found() {
        let e = ep("nobody");
        let err = connect(&e).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::NotFound, "{err}");
    }
}
