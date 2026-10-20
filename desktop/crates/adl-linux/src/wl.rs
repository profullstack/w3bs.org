//! A minimal Wayland client wire: enough to bind globals, send requests (with file descriptors)
//! and read events, without libwayland. Used to read the live keymap (`xkb`) and to drive the
//! wlroots virtual pointer and keyboard (`hyprland`).

use std::collections::VecDeque;
use std::ffi::c_void;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;

#[derive(Default)]
pub struct Args(pub Vec<u8>);

impl Args {
    pub fn new() -> Self {
        Self(vec![])
    }
    pub fn u32(mut self, v: u32) -> Self {
        self.0.extend_from_slice(&v.to_ne_bytes());
        self
    }
    pub fn string(mut self, s: &str) -> Self {
        self.0.extend_from_slice(&(s.len() as u32 + 1).to_ne_bytes());
        self.0.extend_from_slice(s.as_bytes());
        self.0.push(0);
        while !self.0.len().is_multiple_of(4) {
            self.0.push(0);
        }
        self
    }
}

/// Reader over one event's arguments.
pub struct ArgReader<'a>(pub &'a [u8]);

impl ArgReader<'_> {
    pub fn u32(&mut self) -> Result<u32, String> {
        let (h, t) = self.0.split_at_checked(4).ok_or("short Wayland event")?;
        self.0 = t;
        Ok(u32::from_ne_bytes(h.try_into().unwrap()))
    }
    pub fn string(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        let padded = n.div_ceil(4) * 4;
        let (h, t) = self.0.split_at_checked(padded).ok_or("short Wayland string")?;
        self.0 = t;
        Ok(String::from_utf8_lossy(&h[..n.saturating_sub(1)]).into_owned())
    }
}

pub struct Wire {
    pub sock: UnixStream,
    pub buf: Vec<u8>,
    pub fds: VecDeque<OwnedFd>,
}

impl Wire {
    /// Connect to the session's compositor (`$WAYLAND_DISPLAY` under `$XDG_RUNTIME_DIR`).
    pub fn connect(read_timeout: Duration) -> Result<Self, String> {
        let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
        let path = if display.starts_with('/') {
            display.into()
        } else {
            std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set")?).join(display)
        };
        let sock = UnixStream::connect(&path).map_err(|e| format!("Wayland {}: {e}", path.display()))?;
        sock.set_read_timeout(Some(read_timeout)).map_err(|e| e.to_string())?;
        Ok(Self { sock, buf: vec![], fds: VecDeque::new() })
    }

    /// Send a request carrying one file descriptor (SCM_RIGHTS), e.g. a keymap.
    pub fn send_fd(&mut self, obj: u32, opcode: u16, args: &Args, fd: RawFd) -> Result<(), String> {
        let len = 8 + args.0.len();
        let mut m = Vec::with_capacity(len);
        m.extend_from_slice(&obj.to_ne_bytes());
        m.extend_from_slice(&(((len as u32) << 16) | opcode as u32).to_ne_bytes());
        m.extend_from_slice(&args.0);
        let mut cbuf = [0u64; 4];
        let mut iov = libc::iovec { iov_base: m.as_mut_ptr() as *mut c_void, iov_len: m.len() };
        // SAFETY: one SCM_RIGHTS control message with one fd, built in a stack buffer large
        // enough for CMSG_SPACE(sizeof(int)); the iovec and buffers outlive the sendmsg call.
        let n = unsafe {
            let mut msg: libc::msghdr = std::mem::zeroed();
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = cbuf.as_mut_ptr() as *mut c_void;
            msg.msg_controllen = libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) as _;
            let c = libc::CMSG_FIRSTHDR(&msg);
            (*c).cmsg_level = libc::SOL_SOCKET;
            (*c).cmsg_type = libc::SCM_RIGHTS;
            (*c).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
            std::ptr::write_unaligned(libc::CMSG_DATA(c) as *mut RawFd, fd);
            libc::sendmsg(self.sock.as_raw_fd(), &msg, libc::MSG_NOSIGNAL)
        };
        if n as usize != m.len() {
            return Err(format!("Wayland write: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }

    pub fn send(&mut self, obj: u32, opcode: u16, args: &Args) -> Result<(), String> {
        let len = 8 + args.0.len();
        let mut m = Vec::with_capacity(len);
        m.extend_from_slice(&obj.to_ne_bytes());
        m.extend_from_slice(&(((len as u32) << 16) | opcode as u32).to_ne_bytes());
        m.extend_from_slice(&args.0);
        self.sock.write_all(&m).map_err(|e| format!("Wayland write: {e}"))
    }

    /// Read more bytes, collecting any file descriptors passed alongside them.
    pub fn fill(&mut self) -> Result<(), String> {
        let mut data = [0u8; 4096];
        let mut cbuf = [0u64; 64];
        let mut iov = libc::iovec { iov_base: data.as_mut_ptr() as *mut c_void, iov_len: data.len() };
        // SAFETY: recvmsg into stack buffers that outlive the call; control messages are walked
        // with the libc CMSG helpers within msg_controllen.
        unsafe {
            let mut msg: libc::msghdr = std::mem::zeroed();
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = cbuf.as_mut_ptr() as *mut c_void;
            msg.msg_controllen = std::mem::size_of_val(&cbuf) as _;
            let fd = self.sock.as_raw_fd();
            let n = libc::recvmsg(fd, &mut msg, libc::MSG_CMSG_CLOEXEC);
            if n < 0 {
                return Err(format!("Wayland read: {}", std::io::Error::last_os_error()));
            }
            if n == 0 {
                return Err("the compositor closed the Wayland connection".into());
            }
            let mut c = libc::CMSG_FIRSTHDR(&msg);
            while !c.is_null() {
                if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                    let data = libc::CMSG_DATA(c) as *const RawFd;
                    let count = ((*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / std::mem::size_of::<RawFd>();
                    for i in 0..count {
                        self.fds.push_back(OwnedFd::from_raw_fd(std::ptr::read_unaligned(data.add(i))));
                    }
                }
                c = libc::CMSG_NXTHDR(&msg, c);
            }
            self.buf.extend_from_slice(&data[..n as usize]);
        }
        Ok(())
    }

    /// Dispatch events to `f` until wl_callback `cb` is done.
    pub fn until_done(&mut self, cb: u32, mut f: impl FnMut(u32, u16, &mut ArgReader, &mut VecDeque<OwnedFd>) -> Result<(), String>) -> Result<(), String> {
        loop {
            while self.buf.len() >= 8 {
                let obj = u32::from_ne_bytes(self.buf[0..4].try_into().unwrap());
                let word = u32::from_ne_bytes(self.buf[4..8].try_into().unwrap());
                let (len, op) = ((word >> 16) as usize, (word & 0xffff) as u16);
                if len < 8 {
                    return Err("malformed Wayland message".into());
                }
                if self.buf.len() < len {
                    break;
                }
                let body: Vec<u8> = self.buf[8..len].to_vec();
                self.buf.drain(..len);
                let mut a = ArgReader(&body);
                if obj == 1 && op == 0 {
                    let (_, code, msg) = (a.u32()?, a.u32()?, a.string()?);
                    return Err(format!("Wayland protocol error {code}: {msg}"));
                }
                if obj == cb && op == 0 {
                    return Ok(());
                }
                f(obj, op, &mut a, &mut self.fds)?;
            }
            self.fill()?;
        }
    }
}

