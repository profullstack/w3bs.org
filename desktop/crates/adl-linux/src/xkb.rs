//! Which characters Mutter (GNOME) can type from a keysym, read from the live keymap.
//!
//! The RemoteDesktop portal's `NotifyKeyboardKeysym` reaches Mutter's virtual keyboard, which
//! types a keysym only if a key of the CURRENT layout group produces it: it searches keycodes in
//! ascending order and levels within each key, and presses the first match, adding Shift for
//! level 2 and ISO_Level3_Shift (AltGr) for level 3. Anything else is dropped with a log line
//! ("No keycode found for keyval ... in current group") while the D-Bus call still succeeds, and a
//! level-4 match is typed WITHOUT its modifiers, i.e. as a different character. KWin remaps
//! keysyms it cannot find; Mutter does not. So on GNOME every character is checked against the
//! same keymap Mutter uses before anything of a batch is typed, and text with a character that
//! would be lost is refused whole.
//!
//! The keymap is the one the compositor hands every Wayland client (`wl_keyboard.keymap`), read
//! over a short-lived Wayland connection, and compiled with the system's libxkbcommon (loaded at
//! run time; the library Mutter itself uses). The current group comes from the W3BS GNOME Shell
//! extension (`global.backend.get_keymap_layout_group()`); without it a character must be
//! typeable in every group. If the keymap cannot be read, only printable ASCII is allowed.

use crate::keymap;
use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::io::Write;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;
use std::sync::OnceLock;
use std::time::Duration;

const SHIFT_L: u32 = 0xffe1;
const ISO_LEVEL3_SHIFT: u32 = 0xfe03;

/// One layout group of a keymap: for each keycode in ascending order, the keysyms of each level.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Group {
    pub name: String,
    pub keys: Vec<Vec<Vec<u32>>>,
}

impl Group {
    /// Level of the first key producing `sym`, searched the way Mutter does.
    fn level_of(&self, sym: u32) -> Option<usize> {
        self.keys.iter().find_map(|levels| levels.iter().position(|syms| syms.contains(&sym)))
    }

    /// Whether Mutter's virtual keyboard types `sym` exactly on this group.
    pub fn types(&self, sym: u32) -> bool {
        match self.level_of(sym) {
            Some(0) => true,
            Some(1) => self.level_of(SHIFT_L).is_some(),
            Some(2) => self.level_of(ISO_LEVEL3_SHIFT).is_some(),
            _ => false,
        }
    }
}

/// The keysyms a character may be sent as: ours first, then libxkbcommon's legacy keysym
/// (EuroSign for '€'), then the Unicode keysym.
fn candidates(c: char, utf32_to_keysym: impl Fn(u32) -> u32) -> Vec<u32> {
    let mut v = vec![keymap::char_keysym(c)];
    for k in [utf32_to_keysym(c as u32), 0x0100_0000 | c as u32] {
        if k != 0 && !v.contains(&k) {
            v.push(k);
        }
    }
    v
}

/// Resolve every character of `text` to a keysym every group in `groups` types exactly. On
/// failure returns the characters that cannot be typed, each once, in order.
pub fn resolve(text: &str, groups: &[Group], utf32_to_keysym: impl Fn(u32) -> u32) -> Result<Vec<u32>, Vec<char>> {
    let mut out = Vec::with_capacity(text.len());
    let mut missing: Vec<char> = vec![];
    for c in text.chars() {
        match candidates(c, &utf32_to_keysym).into_iter().find(|k| !groups.is_empty() && groups.iter().all(|g| g.types(*k))) {
            Some(k) => out.push(k),
            None if !missing.contains(&c) => missing.push(c),
            None => {}
        }
    }
    if missing.is_empty() { Ok(out) } else { Err(missing) }
}

/// Without a keymap: printable ASCII, Return and Tab only (present on every Latin layout).
pub fn resolve_ascii(text: &str) -> Result<Vec<u32>, Vec<char>> {
    let mut missing: Vec<char> = vec![];
    let mut out = vec![];
    for c in text.chars() {
        if c.is_ascii_graphic() || c == ' ' || c == '\n' || c == '\t' {
            out.push(keymap::char_keysym(c));
        } else if !missing.contains(&c) {
            missing.push(c);
        }
    }
    if missing.is_empty() { Ok(out) } else { Err(missing) }
}

/// `'é' (U+00E9), '€' (U+20AC)`
pub fn describe(chars: &[char]) -> String {
    chars
        .iter()
        .map(|c| if c.is_control() || c.is_whitespace() { format!("U+{:04X}", *c as u32) } else { format!("'{c}' (U+{:04X})", *c as u32) })
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------- libxkbcommon (dlopen)

type CtxNew = unsafe extern "C" fn(c_int) -> *mut c_void;
type Unref = unsafe extern "C" fn(*mut c_void);
type KeymapFromString = unsafe extern "C" fn(*mut c_void, *const c_char, c_int, c_int) -> *mut c_void;
type KeymapU32 = unsafe extern "C" fn(*mut c_void) -> u32;
type LayoutName = unsafe extern "C" fn(*mut c_void, u32) -> *const c_char;
type NumLevels = unsafe extern "C" fn(*mut c_void, u32, u32) -> u32;
type SymsByLevel = unsafe extern "C" fn(*mut c_void, u32, u32, u32, *mut *const u32) -> c_int;
type Utf32ToKeysym = unsafe extern "C" fn(u32) -> u32;

struct Xkb {
    context_new: CtxNew,
    context_unref: Unref,
    keymap_new_from_string: KeymapFromString,
    keymap_unref: Unref,
    min_keycode: KeymapU32,
    max_keycode: KeymapU32,
    num_layouts: KeymapU32,
    layout_get_name: LayoutName,
    num_levels_for_key: NumLevels,
    key_get_syms_by_level: SymsByLevel,
    utf32_to_keysym: Utf32ToKeysym,
}

fn xkb() -> Result<&'static Xkb, String> {
    static LIB: OnceLock<Result<Xkb, String>> = OnceLock::new();
    LIB.get_or_init(|| unsafe { load_xkb() }).as_ref().map_err(|e| e.clone())
}

unsafe fn load_xkb() -> Result<Xkb, String> {
    let name = c"libxkbcommon.so.0";
    // SAFETY: dlopen/dlsym with NUL-terminated names; the handle is never closed, so the
    // function pointers stay valid for the life of the process.
    let lib = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if lib.is_null() {
        return Err("libxkbcommon.so.0 is not installed".into());
    }
    macro_rules! sym {
        ($n:literal, $t:ty) => {{
            let p = unsafe { libc::dlsym(lib, concat!($n, "\0").as_ptr() as *const c_char) };
            if p.is_null() {
                return Err(format!("libxkbcommon has no {}", $n));
            }
            // SAFETY: the symbol has the C signature declared for the field it is assigned to.
            unsafe { std::mem::transmute::<*mut c_void, $t>(p) }
        }};
    }
    Ok(Xkb {
        context_new: sym!("xkb_context_new", CtxNew),
        context_unref: sym!("xkb_context_unref", Unref),
        keymap_new_from_string: sym!("xkb_keymap_new_from_string", KeymapFromString),
        keymap_unref: sym!("xkb_keymap_unref", Unref),
        min_keycode: sym!("xkb_keymap_min_keycode", KeymapU32),
        max_keycode: sym!("xkb_keymap_max_keycode", KeymapU32),
        num_layouts: sym!("xkb_keymap_num_layouts", KeymapU32),
        layout_get_name: sym!("xkb_keymap_layout_get_name", LayoutName),
        num_levels_for_key: sym!("xkb_keymap_num_levels_for_key", NumLevels),
        key_get_syms_by_level: sym!("xkb_keymap_key_get_syms_by_level", SymsByLevel),
        utf32_to_keysym: sym!("xkb_utf32_to_keysym", Utf32ToKeysym),
    })
}

/// libxkbcommon's `xkb_utf32_to_keysym`, or the Unicode keysym when the library is missing.
pub fn utf32_to_keysym(cp: u32) -> u32 {
    match xkb() {
        // SAFETY: a pure function of its argument.
        Ok(x) => unsafe { (x.utf32_to_keysym)(cp) },
        Err(_) => 0x0100_0000 | cp,
    }
}

/// Compile an XKB v1 keymap and return its groups: `group` alone when given (wrapped into range
/// the way libxkbcommon wraps it for each key), else every group.
pub fn compile(text: &[u8], group: Option<u32>) -> Result<Vec<Group>, String> {
    let x = xkb()?;
    let text = CString::new(text.split(|b| *b == 0).next().unwrap_or_default()).map_err(|e| e.to_string())?;
    // SAFETY: plain libxkbcommon calls on objects created and released here. Flags: no default
    // include paths and no environment names (a compiled keymap is self-contained).
    unsafe {
        let ctx = (x.context_new)(3);
        if ctx.is_null() {
            return Err("xkb_context_new failed".into());
        }
        let km = (x.keymap_new_from_string)(ctx, text.as_ptr(), 1, 0);
        if km.is_null() {
            (x.context_unref)(ctx);
            return Err("the compositor's keymap did not compile".into());
        }
        let (min, max, n) = ((x.min_keycode)(km), (x.max_keycode)(km), (x.num_layouts)(km).max(1));
        let wanted: Vec<u32> = match group {
            Some(g) => vec![g],
            None => (0..n).collect(),
        };
        let mut out = vec![];
        for g in wanted {
            let name = (x.layout_get_name)(km, g % n);
            let name = if name.is_null() { String::new() } else { CStr::from_ptr(name).to_string_lossy().into_owned() };
            // Mutter's search stops one short of max_keycode; do the same.
            let keys = (min..max)
                .map(|kc| {
                    (0..(x.num_levels_for_key)(km, kc, g))
                        .map(|level| {
                            let mut syms: *const u32 = std::ptr::null();
                            let k = (x.key_get_syms_by_level)(km, kc, g, level, &mut syms);
                            if k <= 0 || syms.is_null() { vec![] } else { std::slice::from_raw_parts(syms, k as usize).to_vec() }
                        })
                        .collect()
                })
                .collect();
            out.push(Group { name, keys });
        }
        (x.keymap_unref)(km);
        (x.context_unref)(ctx);
        Ok(out)
    }
}

// ---------------------------------------------------------------- Wayland: wl_keyboard.keymap

/// Read the keymap the compositor sends every Wayland client, over a minimal client connection
/// (registry, wl_seat, wl_keyboard; nothing is drawn and no input is read).
pub fn wayland_keymap() -> Result<Vec<u8>, String> {
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    let path = if display.starts_with('/') {
        display.into()
    } else {
        std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set")?).join(display)
    };
    let sock = UnixStream::connect(&path).map_err(|e| format!("Wayland {}: {e}", path.display()))?;
    sock.set_read_timeout(Some(Duration::from_secs(2))).map_err(|e| e.to_string())?;
    let mut w = Wire { sock, buf: vec![], fds: VecDeque::new() };
    const DISPLAY: u32 = 1;
    const REGISTRY: u32 = 2;
    const SEAT: u32 = 4;
    const KEYBOARD: u32 = 6;
    w.send(DISPLAY, 1, &Args::new().u32(REGISTRY))?; // get_registry
    w.send(DISPLAY, 0, &Args::new().u32(3))?; // sync
    let mut seat: Option<(u32, u32)> = None;
    w.until_done(3, |obj, op, a, _| {
        if obj == REGISTRY && op == 0 {
            let (name, iface, ver) = (a.u32()?, a.string()?, a.u32()?);
            if iface == "wl_seat" && seat.is_none() {
                seat = Some((name, ver));
            }
        }
        Ok(())
    })?;
    let (name, ver) = seat.ok_or("the compositor offers no wl_seat")?;
    w.send(REGISTRY, 0, &Args::new().u32(name).string("wl_seat").u32(ver.min(5)).u32(SEAT))?; // bind
    w.send(DISPLAY, 0, &Args::new().u32(5))?;
    let mut caps = 0;
    w.until_done(5, |obj, op, a, _| {
        if obj == SEAT && op == 0 {
            caps = a.u32()?;
        }
        Ok(())
    })?;
    if caps & 2 == 0 {
        return Err("the Wayland seat has no keyboard".into());
    }
    w.send(SEAT, 1, &Args::new().u32(KEYBOARD))?; // get_keyboard
    w.send(DISPLAY, 0, &Args::new().u32(7))?;
    let mut keymap: Option<Result<Vec<u8>, String>> = None;
    w.until_done(7, |obj, op, a, fds| {
        if obj == KEYBOARD && op == 0 && keymap.is_none() {
            let (format, size) = (a.u32()?, a.u32()?);
            let fd = fds.pop_front().ok_or("keymap event without a file descriptor")?;
            keymap = Some(if format != 1 {
                Err(format!("keymap format {format} is not XKB v1"))
            } else {
                let f = std::fs::File::from(fd);
                let mut b = vec![0; size as usize];
                // pread: the fd may be shared with other clients, so leave its offset alone.
                f.read_exact_at(&mut b, 0).map(|_| b).map_err(|e| format!("reading the keymap: {e}"))
            });
        }
        Ok(())
    })?;
    keymap.unwrap_or_else(|| Err("the compositor sent no keymap".into()))
}

struct Args(Vec<u8>);

impl Args {
    fn new() -> Self {
        Self(vec![])
    }
    fn u32(mut self, v: u32) -> Self {
        self.0.extend_from_slice(&v.to_ne_bytes());
        self
    }
    fn string(mut self, s: &str) -> Self {
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
struct ArgReader<'a>(&'a [u8]);

impl ArgReader<'_> {
    fn u32(&mut self) -> Result<u32, String> {
        let (h, t) = self.0.split_at_checked(4).ok_or("short Wayland event")?;
        self.0 = t;
        Ok(u32::from_ne_bytes(h.try_into().unwrap()))
    }
    fn string(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        let padded = n.div_ceil(4) * 4;
        let (h, t) = self.0.split_at_checked(padded).ok_or("short Wayland string")?;
        self.0 = t;
        Ok(String::from_utf8_lossy(&h[..n.saturating_sub(1)]).into_owned())
    }
}

struct Wire {
    sock: UnixStream,
    buf: Vec<u8>,
    fds: VecDeque<OwnedFd>,
}

impl Wire {
    fn send(&mut self, obj: u32, opcode: u16, args: &Args) -> Result<(), String> {
        let len = 8 + args.0.len();
        let mut m = Vec::with_capacity(len);
        m.extend_from_slice(&obj.to_ne_bytes());
        m.extend_from_slice(&(((len as u32) << 16) | opcode as u32).to_ne_bytes());
        m.extend_from_slice(&args.0);
        self.sock.write_all(&m).map_err(|e| format!("Wayland write: {e}"))
    }

    /// Read more bytes, collecting any file descriptors passed alongside them.
    fn fill(&mut self) -> Result<(), String> {
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
            let fd = std::os::fd::AsRawFd::as_raw_fd(&self.sock);
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
    fn until_done(&mut self, cb: u32, mut f: impl FnMut(u32, u16, &mut ArgReader, &mut VecDeque<OwnedFd>) -> Result<(), String>) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// A slice of a `us` keymap: keycodes ascending, levels per key.
    fn us() -> Group {
        Group {
            name: "English (US)".into(),
            keys: vec![
                vec![vec![0xff1b]],                // Escape
                vec![vec!['1' as u32], vec!['!' as u32]],
                vec![vec!['e' as u32], vec!['E' as u32]],
                vec![vec![0xff0d]],                // Return
                vec![vec![0xff09], vec![0xfe20]],  // Tab, ISO_Left_Tab
                vec![vec![SHIFT_L]],
                vec![vec![' ' as u32]],
                vec![vec![ISO_LEVEL3_SHIFT]],
            ],
        }
    }

    /// A slice of `de`: ß, ä, € on AltGr+e (level 3), and a level-4 only character.
    fn de() -> Group {
        Group {
            name: "German".into(),
            keys: vec![
                vec![vec!['e' as u32], vec!['E' as u32], vec![0x20ac], vec![0x20ac]],
                vec![vec![0xdf], vec!['?' as u32], vec![0x5c], vec![0x1001e9e]],
                vec![vec![0xe4], vec![0xc4]],
                vec![vec!['y' as u32], vec!['Y' as u32], vec![0x8fb], vec![0xa5]], // ¥ only on level 4
                vec![vec![SHIFT_L]],
                vec![vec![ISO_LEVEL3_SHIFT]],
            ],
        }
    }

    fn legacy(cp: u32) -> u32 {
        match cp {
            0x20ac => 0x20ac,
            0x1e9e => 0x1001e9e,
            c if c < 0x100 => c,
            c => 0x0100_0000 | c,
        }
    }

    #[test]
    fn mutter_types_only_what_the_group_has() {
        assert_eq!(resolve("e1! \n\t", &[us()], legacy), Ok(vec!['e' as u32, '1' as u32, '!' as u32, ' ' as u32, 0xff0d, 0xff09]));
        assert_eq!(resolve("héllo üß €", &[us()], legacy), Err(vec!['h', 'é', 'l', 'o', 'ü', 'ß', '€']));
        assert_eq!(resolve("ß€ä", &[de()], legacy), Ok(vec![0xdf, 0x20ac, 0xe4]), "level 3 needs AltGr, which de has");
        assert_eq!(resolve("¥", &[de()], legacy), Err(vec!['¥']), "level 4 would be typed without its modifiers");
        assert_eq!(resolve("é", &[de()], legacy), Err(vec!['é']), "dead keys are not a keysym match");
    }

    #[test]
    fn modifiers_must_exist() {
        let mut g = us();
        g.keys.retain(|k| k != &vec![vec![SHIFT_L]]);
        assert_eq!(resolve("!", &[g.clone()], legacy), Err(vec!['!']), "no Shift_L: level 2 would type '1'");
        assert!(resolve("1", &[g], legacy).is_ok());
        let mut d = de();
        d.keys.retain(|k| k != &vec![vec![ISO_LEVEL3_SHIFT]]);
        assert_eq!(resolve("€", &[d], legacy), Err(vec!['€']));
    }

    #[test]
    fn unknown_group_needs_every_group() {
        assert_eq!(resolve("e", &[us(), de()], legacy), Ok(vec!['e' as u32]));
        assert_eq!(resolve("ä!", &[us(), de()], legacy), Err(vec!['ä', '!']), "de has no '!' in this slice");
        assert!(resolve("e", &[], legacy).is_err(), "no groups types nothing");
    }

    #[test]
    fn ascii_fallback_and_descriptions() {
        assert!(resolve_ascii("plain ASCII ~!\n").is_ok());
        assert_eq!(resolve_ascii("naïve café"), Err(vec!['ï', 'é']));
        assert_eq!(describe(&['é', '€', '\u{a0}']), "'é' (U+00E9), '€' (U+20AC), U+00A0");
    }

    #[test]
    fn wire_encoding() {
        let a = Args::new().u32(7).string("wl_seat").u32(5);
        assert_eq!(a.0.len(), 4 + 4 + 8 + 4, "string is length, bytes, NUL, padded to 4");
        let mut r = ArgReader(&a.0);
        assert_eq!((r.u32().unwrap(), r.string().unwrap().as_str(), r.u32().unwrap()), (7, "wl_seat", 5));
        assert!(r.u32().is_err());
    }

    /// A fake compositor on a socketpair: registry with a seat, keyboard capability, and a keymap
    /// passed as a file descriptor.
    #[test]
    fn reads_keymap_from_a_fake_compositor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wl-test");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let keymap = b"xkb_keymap { fake };\0".to_vec();
        let km = keymap.clone();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let read_msg = |s: &mut UnixStream| -> (u32, u16, Vec<u8>) {
                let mut h = [0u8; 8];
                s.read_exact(&mut h).unwrap();
                let w = u32::from_ne_bytes(h[4..8].try_into().unwrap());
                let mut b = vec![0; (w >> 16) as usize - 8];
                s.read_exact(&mut b).unwrap();
                (u32::from_ne_bytes(h[0..4].try_into().unwrap()), (w & 0xffff) as u16, b)
            };
            let ev = |obj: u32, op: u16, a: Args| {
                let mut m = obj.to_ne_bytes().to_vec();
                m.extend_from_slice(&((((8 + a.0.len()) as u32) << 16) | op as u32).to_ne_bytes());
                m.extend_from_slice(&a.0);
                m
            };
            let done = |id: u32| ev(id, 0, Args::new().u32(0));
            assert_eq!(read_msg(&mut s).1, 1); // get_registry
            read_msg(&mut s); // sync 3
            s.write_all(&ev(2, 0, Args::new().u32(1).string("wl_compositor").u32(6))).unwrap();
            s.write_all(&ev(2, 0, Args::new().u32(9).string("wl_seat").u32(9))).unwrap();
            s.write_all(&done(3)).unwrap();
            let (_, op, b) = read_msg(&mut s); // bind
            assert_eq!(op, 0);
            let mut r = ArgReader(&b);
            assert_eq!((r.u32().unwrap(), r.string().unwrap().as_str(), r.u32().unwrap(), r.u32().unwrap()), (9, "wl_seat", 5, 4));
            read_msg(&mut s); // sync 5
            s.write_all(&ev(4, 0, Args::new().u32(3))).unwrap(); // pointer | keyboard
            s.write_all(&done(5)).unwrap();
            assert_eq!(read_msg(&mut s), (4, 1, 6u32.to_ne_bytes().to_vec())); // get_keyboard
            read_msg(&mut s); // sync 7
            let f = tempfile::tempfile().unwrap();
            f.write_all_at(&km, 0).unwrap();
            let msg = ev(6, 0, Args::new().u32(1).u32(km.len() as u32));
            // send the keymap event with the fd attached
            unsafe {
                let mut iov = libc::iovec { iov_base: msg.as_ptr() as *mut c_void, iov_len: msg.len() };
                let mut cbuf = [0u64; 8];
                let mut m: libc::msghdr = std::mem::zeroed();
                m.msg_iov = &mut iov;
                m.msg_iovlen = 1;
                m.msg_control = cbuf.as_mut_ptr() as *mut c_void;
                m.msg_controllen = libc::CMSG_SPACE(4) as _;
                let c = libc::CMSG_FIRSTHDR(&m);
                (*c).cmsg_level = libc::SOL_SOCKET;
                (*c).cmsg_type = libc::SCM_RIGHTS;
                (*c).cmsg_len = libc::CMSG_LEN(4) as _;
                std::ptr::write_unaligned(libc::CMSG_DATA(c) as *mut RawFd, std::os::fd::AsRawFd::as_raw_fd(&f));
                assert!(libc::sendmsg(std::os::fd::AsRawFd::as_raw_fd(&s), &m, 0) > 0);
            }
            s.write_all(&done(7)).unwrap();
        });
        // SAFETY (test): this test alone reads WAYLAND_DISPLAY; no other test touches it.
        unsafe { std::env::set_var("WAYLAND_DISPLAY", &path) };
        let got = wayland_keymap();
        server.join().unwrap();
        assert_eq!(got.unwrap(), keymap);
    }
}
