//! Hyprland adapter: the W3BS DE session (w3bs.org/de) and any other Hyprland desktop.
//!
//! Hyprland's portal (xdg-desktop-portal-hyprland) has no RemoteDesktop interface, so the portal
//! backend cannot inject input here. Hyprland instead implements the wlroots virtual input
//! protocols, which this adapter speaks directly over its own Wayland connection:
//! - `zwlr_virtual_pointer_v1` for absolute/relative motion, buttons and wheel steps;
//! - `zwp_virtual_keyboard_v1` with a keymap this adapter generates (like `wtype`): every keysym
//!   it needs gets its own keycode, so any Unicode character types exactly, on any layout, and
//!   nothing is ever silently dropped. Modifier state is sent explicitly with each change.
//!
//! Everything else comes from Hyprland's IPC (`$XDG_RUNTIME_DIR/hypr/<instance>/.socket.sock`,
//! one request per connection, closed straight away because Hyprland serves it synchronously):
//! `j/monitors` for the layout, `j/clients` for windows, focus and stacking, `j/cursorpos` for
//! the pointer. `.socket2.sock` events mark the layout stale. Captures go through `grim`
//! (wlr-screencopy).
//!
//! Interlocks: input is refused while hyprlock runs or an authentication dialog (polkit agent,
//! pinentry, GCR prompter, ssh-askpass) is mapped. The person moving the pointer away from where
//! the agent left it is reported as physical input (a takeover); keyboard takeover is not
//! detectable on this backend.

use crate::keymap;
use crate::wl::{Args, Wire};
use adl_broker::{Adapter, InputEvent, SessionEvent};
use adl_schema::*;
use serde_json::Value;
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BACKEND: &str = "linux-hyprland";

// ---------------------------------------------------------------- IPC

/// Hyprland's request socket for this session.
#[derive(Clone, Debug)]
pub struct HyprIpc {
    dir: PathBuf,
}

impl HyprIpc {
    /// From `HYPRLAND_INSTANCE_SIGNATURE` (set for every process in a Hyprland session).
    pub fn from_env() -> Option<Self> {
        let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok().filter(|s| !s.is_empty() && !s.contains('/'))?;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)?;
        let dir = runtime.join("hypr").join(sig);
        dir.join(".socket.sock").exists().then_some(Self { dir })
    }

    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// One request, one connection: write, then read the whole reply.
    pub fn request(&self, cmd: &str) -> Result<String> {
        let path = self.dir.join(".socket.sock");
        let mut s = UnixStream::connect(&path).map_err(|e| AdlError::new(ErrorCode::Blocked, format!("Hyprland IPC {}: {e}", path.display())))?;
        let t = Some(Duration::from_secs(2));
        s.set_read_timeout(t).and_then(|_| s.set_write_timeout(t)).map_err(|e| AdlError::internal(e.to_string()))?;
        s.write_all(cmd.as_bytes()).map_err(|e| AdlError::internal(format!("Hyprland IPC write: {e}")))?;
        let mut out = String::new();
        s.read_to_string(&mut out).map_err(|e| AdlError::internal(format!("Hyprland IPC read: {e}")))?;
        Ok(out)
    }

    pub fn json(&self, cmd: &str) -> Result<Value> {
        let r = self.request(&format!("j/{cmd}"))?;
        serde_json::from_str(&r).map_err(|e| AdlError::internal(format!("unexpected Hyprland reply to {cmd}: {e}: {}", r.chars().take(120).collect::<String>())))
    }

    fn events_socket(&self) -> PathBuf {
        self.dir.join(".socket2.sock")
    }
}

// ---------------------------------------------------------------- state from IPC

#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    pub id: i64,
    pub name: String,
    pub x: f64,
    pub y: f64,
    /// Mode size in physical pixels, before rotation.
    pub width: f64,
    pub height: f64,
    pub scale: f64,
    pub transform: i64,
    pub active_ws: i64,
    pub special_ws: i64,
    pub focused: bool,
}

fn num(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

fn int(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(|x| x.as_i64().or_else(|| x.as_bool().map(i64::from))).unwrap_or(0)
}

fn text(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn flag(v: &Value, k: &str) -> bool {
    v.get(k).and_then(|x| x.as_bool().or_else(|| x.as_i64().map(|n| n != 0))).unwrap_or(false)
}

pub fn parse_monitors(v: &Value) -> Vec<Monitor> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter(|m| !flag(m, "disabled"))
        .map(|m| Monitor {
            id: int(m, "id"),
            name: text(m, "name"),
            x: num(m, "x"),
            y: num(m, "y"),
            width: num(m, "width"),
            height: num(m, "height"),
            scale: Some(num(m, "scale")).filter(|s| *s > 0.0).unwrap_or(1.0),
            transform: int(m, "transform"),
            active_ws: m.get("activeWorkspace").map_or(0, |w| int(w, "id")),
            special_ws: m.get("specialWorkspace").map_or(0, |w| int(w, "id")),
            focused: flag(m, "focused"),
        })
        .collect()
}

impl Monitor {
    /// Hyprland transforms 1/3/5/7 turn the output a quarter.
    fn quarter_turn(&self) -> bool {
        self.transform % 2 == 1
    }

    pub fn logical(&self) -> Rect {
        let (w, h) = if self.quarter_turn() { (self.height, self.width) } else { (self.width, self.height) };
        Rect { x: self.x, y: self.y, width: (w / self.scale).round(), height: (h / self.scale).round() }
    }

    pub fn display(&self) -> DisplayInfo {
        DisplayInfo {
            id: self.name.clone(),
            name: self.name.clone(),
            logical: self.logical(),
            scale: self.scale,
            rotation: match self.transform % 4 {
                1 => Rotation::Rotate90,
                2 => Rotation::Rotate180,
                3 => Rotation::Rotate270,
                _ => Rotation::Normal,
            },
            primary: self.focused,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Client {
    pub address: String,
    pub class: String,
    pub title: String,
    pub bounds: Rect,
    pub workspace: i64,
    pub monitor: i64,
    pub floating: bool,
    pub pinned: bool,
    pub fullscreen: bool,
    /// 0 is the focused window; higher was focused longer ago.
    pub focus_history: i64,
    pub mapped: bool,
    pub hidden: bool,
}

pub fn parse_clients(v: &Value) -> Vec<Client> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            let pair = |k: &str| c.get(k).and_then(Value::as_array).map_or((0.0, 0.0), |a| (a.first().and_then(Value::as_f64).unwrap_or(0.0), a.get(1).and_then(Value::as_f64).unwrap_or(0.0)));
            let ((x, y), (w, h)) = (pair("at"), pair("size"));
            Client {
                address: text(c, "address"),
                class: Some(text(c, "class")).filter(|s| !s.is_empty()).unwrap_or_else(|| text(c, "initialClass")),
                title: text(c, "title"),
                bounds: Rect { x, y, width: w, height: h },
                workspace: c.get("workspace").map_or(0, |w| int(w, "id")),
                monitor: int(c, "monitor"),
                floating: flag(c, "floating"),
                pinned: flag(c, "pinned"),
                fullscreen: flag(c, "fullscreen"),
                focus_history: c.get("focusHistoryID").and_then(Value::as_i64).unwrap_or(i64::MAX),
                mapped: c.get("mapped").and_then(Value::as_bool).unwrap_or(true),
                hidden: flag(c, "hidden"),
            }
        })
        .collect()
}

/// Windows on screen (mapped, not hidden, on a monitor's active or special workspace, or pinned),
/// topmost first: a special workspace over everything, then fullscreen, then floating and pinned
/// (most recently focused first), then tiled windows, which do not overlap each other.
pub fn visible<'a>(clients: &'a [Client], monitors: &[Monitor]) -> Vec<&'a Client> {
    let shown = |c: &Client| {
        c.mapped
            && !c.hidden
            && (c.pinned || monitors.iter().any(|m| c.workspace == m.active_ws || (m.special_ws != 0 && c.workspace == m.special_ws)))
    };
    let special = |c: &Client| monitors.iter().any(|m| m.special_ws != 0 && c.workspace == m.special_ws);
    let mut v: Vec<&Client> = clients.iter().filter(|c| shown(c)).collect();
    v.sort_by_key(|c| (!special(c), !c.fullscreen, !(c.floating || c.pinned), c.focus_history));
    v
}

fn window_info(c: &Client, monitors: &[Monitor]) -> WindowInfo {
    WindowInfo {
        id: c.address.clone(),
        app_id: c.class.clone(),
        title: c.title.clone(),
        bounds: Some(c.bounds),
        display: monitors.iter().find(|m| m.id == c.monitor).map(|m| m.name.clone()),
        focused: c.focus_history == 0,
    }
}

/// Window classes of authentication dialogs: synthetic input must never reach them.
const AUTH_CLASSES: &[&str] = &["polkit", "pinentry", "gcr-prompter", "ssh-askpass", "askpass", "keyring"];

pub fn auth_dialog(clients: &[Client]) -> Option<&Client> {
    clients.iter().find(|c| c.mapped && !c.hidden && AUTH_CLASSES.iter().any(|a| c.class.to_ascii_lowercase().contains(a)))
}

/// Whether a screen locker is running (hyprlock, swaylock).
fn locker_running() -> Option<String> {
    let dir = std::fs::read_dir("/proc").ok()?;
    for e in dir.flatten() {
        if !e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if let Ok(comm) = std::fs::read_to_string(e.path().join("comm")) {
            let comm = comm.trim();
            if comm == "hyprlock" || comm == "swaylock" {
                return Some(comm.to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------- generated keymap

const SHIFT_L: u32 = 0xffe1;
const CONTROL_L: u32 = 0xffe3;
const ALT_L: u32 = 0xffe9;
const SUPER_L: u32 = 0xffeb;

/// Evdev codes with no symbol on a US (evdev, pc105) keymap, used when the compositor's own keymap
/// cannot be read. Measured from xkeyboard-config with `xkbcli compile-keymap --layout us`.
pub const FREE_US_CODES: &[u32] = &[89, 95, 112, 124, 141, 146, 160, 170, 175, 176, 211, 214, 222, 240];

/// The virtual keyboard's keymap. Hyprland resolves its key bindings through the *seat's* keymap,
/// not the virtual keyboard's, so a keycode that is a media key there (evdev 113-115 are Mute and
/// Volume on a US map) fires that binding whatever keysym our keymap gives it. So:
/// - every key a US keyboard has sits at its real position with both levels (a/A, 1/!), and Shift
///   goes through the modifier state, as with a physical keyboard: bindings see what they would
///   see from real keys (SUPER+1 is still workspace 1);
/// - anything else (é, ✓, 日) goes on a keycode that has no symbol at all in the seat's keymap,
///   so no binding can match it. There are few of those; text that needs more is typed in parts.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyTable {
    /// evdev code -> (base keysym, shifted keysym or 0)
    fixed: std::collections::BTreeMap<u32, (u32, u32)>,
    free: Vec<u32>,
    /// (evdev code, keysym) currently assigned from `free`
    extra: Vec<(u32, u32)>,
}

impl KeyTable {
    pub fn new(free: &[u32]) -> Self {
        let mut fixed = std::collections::BTreeMap::new();
        for (code, sym) in [(42, SHIFT_L), (29, CONTROL_L), (56, ALT_L), (125, SUPER_L)] {
            fixed.insert(code, (sym, 0));
        }
        for n in adl_schema::keys::NAMED_KEYS {
            if let (Some(code), Some(sym)) = (keymap::key_code(&Key::Named(n)), keymap::keysym(&Key::Named(n))) {
                fixed.entry(code).or_insert((sym, 0));
            }
        }
        for c in ' '..='~' {
            if let Some((code, shift)) = keymap::char_code(c) {
                let e = fixed.entry(code).or_insert((0, 0));
                if shift { e.1 = keymap::char_keysym(c) } else { e.0 = keymap::char_keysym(c) }
            }
        }
        let free = free.iter().copied().filter(|c| (1..=247).contains(c) && !fixed.contains_key(c)).collect();
        Self { fixed, free, extra: vec![] }
    }

    /// Where `sym` is now: (evdev code, needs Shift).
    pub fn lookup(&self, sym: u32) -> Option<(u32, bool)> {
        for (code, (base, shifted)) in &self.fixed {
            if *base == sym {
                return Some((*code, false));
            }
            if *shifted == sym && sym != 0 {
                return Some((*code, true));
            }
        }
        self.extra.iter().find(|(_, s)| *s == sym).map(|(c, _)| (*c, false))
    }

    pub fn capacity(&self) -> usize {
        self.free.len()
    }

    /// Make room for `syms` (at most `capacity()` of them need a free code). Returns whether the
    /// keymap changed and must be uploaded again.
    pub fn assign(&mut self, syms: &[u32]) -> bool {
        let missing: Vec<u32> = syms.iter().copied().filter(|s| self.lookup(*s).is_none()).collect();
        if missing.is_empty() {
            return false;
        }
        // Keep extras still wanted; drop the rest to make room.
        self.extra.retain(|(_, s)| syms.contains(s));
        for sym in missing {
            if let Some(code) = self.free.iter().copied().find(|c| !self.extra.iter().any(|(e, _)| e == c)) {
                self.extra.push((code, sym));
            }
        }
        true
    }

    /// XKB v1 keymap text. Keysyms are written as hex; two-level keys get their types from
    /// the `complete` set (ALPHABETIC for letters, TWO_LEVEL otherwise).
    pub fn keymap(&self) -> String {
        let mut keys: Vec<(u32, u32, u32)> = self.fixed.iter().map(|(c, (b, s))| (*c, *b, *s)).collect();
        keys.extend(self.extra.iter().map(|(c, s)| (*c, *s, 0)));
        keys.sort();
        let mut k = String::from("xkb_keymap {\nxkb_keycodes \"w3bs\" {\n  minimum = 8;\n  maximum = 255;\n");
        for (code, _, _) in &keys {
            k += &format!("  <C{code}> = {};\n", code + 8);
        }
        k += "};\nxkb_types \"w3bs\" { include \"complete\" };\nxkb_compatibility \"w3bs\" { include \"complete\" };\nxkb_symbols \"w3bs\" {\n";
        for (code, base, shifted) in &keys {
            if *shifted != 0 {
                k += &format!("  key <C{code}> {{ [ 0x{base:08x}, 0x{shifted:08x} ] }};\n");
            } else {
                k += &format!("  key <C{code}> {{ [ 0x{base:08x} ] }};\n");
            }
        }
        k += "  modifier_map Shift { <C42> };\n  modifier_map Control { <C29> };\n  modifier_map Mod1 { <C56> };\n  modifier_map Mod4 { <C125> };\n};\n};\n";
        k
    }
}

/// Keycodes the seat's keymap leaves empty, read from the compositor (before our own virtual
/// keyboard exists, so it is the real keyboard's map); the US list when that is not possible.
fn free_codes() -> Vec<u32> {
    let live = crate::xkb::wayland_keymap()
        .ok()
        .filter(|t| !String::from_utf8_lossy(t).contains("xkb_keycodes \"w3bs\""))
        .and_then(|t| crate::xkb::empty_keycodes(&t).ok())
        .map(|v| v.into_iter().map(|kc| kc - 8).filter(|c| (1..=247).contains(c)).collect::<Vec<u32>>())
        .filter(|v| v.len() >= 4);
    live.unwrap_or_else(|| FREE_US_CODES.to_vec())
}

/// XKB modifier mask bits for the standard modifier map.
pub fn modifier_mask(m: Modifier) -> u32 {
    match m {
        Modifier::Shift => 1,
        Modifier::Ctrl | Modifier::Primary => 4,
        Modifier::Alt => 8,
        Modifier::Super => 64,
        Modifier::Command | Modifier::Option => 0,
    }
}

fn modifier_sym(m: Modifier) -> Option<u32> {
    Some(match m {
        Modifier::Shift => SHIFT_L,
        Modifier::Ctrl | Modifier::Primary => CONTROL_L,
        Modifier::Alt => ALT_L,
        Modifier::Super => SUPER_L,
        Modifier::Command | Modifier::Option => return None,
    })
}

// ---------------------------------------------------------------- virtual input

const DISPLAY: u32 = 1;
const REGISTRY: u32 = 2;

/// One Wayland connection holding a virtual pointer and a virtual keyboard. Dropping it closes the
/// connection, which destroys both devices (and releases anything held).
struct VirtualInput {
    w: Wire,
    next: u32,
    pointer: u32,
    keyboard: u32,
    keys: KeyTable,
    mods: u32,
    started: Instant,
}

impl VirtualInput {
    fn open() -> std::result::Result<Self, String> {
        let free = free_codes();
        let mut w = Wire::connect(Duration::from_secs(2))?;
        w.send(DISPLAY, 1, &Args::new().u32(REGISTRY))?; // get_registry
        w.send(DISPLAY, 0, &Args::new().u32(3))?; // sync
        let (mut seat, mut vp, mut vk) = (None, None, None);
        w.until_done(3, |obj, op, a, _| {
            if obj == REGISTRY && op == 0 {
                let (name, iface, ver) = (a.u32()?, a.string()?, a.u32()?);
                match iface.as_str() {
                    "wl_seat" if seat.is_none() => seat = Some((name, ver)),
                    "zwlr_virtual_pointer_manager_v1" => vp = Some((name, ver)),
                    "zwp_virtual_keyboard_manager_v1" => vk = Some((name, ver)),
                    _ => {}
                }
            }
            Ok(())
        })?;
        let seat = seat.ok_or("the compositor offers no wl_seat")?;
        let vp = vp.ok_or("the compositor does not offer zwlr_virtual_pointer_manager_v1")?;
        let vk = vk.ok_or("the compositor does not offer zwp_virtual_keyboard_manager_v1")?;
        let (seat_id, vpm, vkm, pointer, keyboard) = (4, 5, 6, 7, 8);
        let bind = |w: &mut Wire, (name, ver): (u32, u32), iface: &str, max: u32, id: u32| w.send(REGISTRY, 0, &Args::new().u32(name).string(iface).u32(ver.min(max)).u32(id));
        bind(&mut w, seat, "wl_seat", 1, seat_id)?;
        bind(&mut w, vp, "zwlr_virtual_pointer_manager_v1", 1, vpm)?;
        bind(&mut w, vk, "zwp_virtual_keyboard_manager_v1", 1, vkm)?;
        w.send(vpm, 0, &Args::new().u32(seat_id).u32(pointer))?; // create_virtual_pointer
        w.send(vkm, 0, &Args::new().u32(seat_id).u32(keyboard))?; // create_virtual_keyboard
        let mut vi = Self { w, next: 9, pointer, keyboard, keys: KeyTable::new(&free), mods: 0, started: Instant::now() };
        vi.upload_keymap()?;
        vi.roundtrip()?;
        Ok(vi)
    }

    fn time(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }

    /// wl_display.sync and wait: delivers everything sent so far and surfaces protocol errors.
    fn roundtrip(&mut self) -> std::result::Result<(), String> {
        let cb = self.next;
        self.next += 1;
        self.w.send(DISPLAY, 0, &Args::new().u32(cb))?;
        self.w.until_done(cb, |_, _, _, fds| {
            fds.clear();
            Ok(())
        })
    }

    fn upload_keymap(&mut self) -> std::result::Result<(), String> {
        let mut text = self.keys.keymap().into_bytes();
        text.push(0);
        // SAFETY: memfd_create with a valid NUL-terminated name; the fd is owned immediately.
        let fd = unsafe { libc::memfd_create(c"w3bs-keymap".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(format!("memfd_create: {}", std::io::Error::last_os_error()));
        }
        // SAFETY: fd was just returned by memfd_create and is not owned elsewhere.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut f = std::fs::File::from(fd);
        f.write_all(&text).map_err(|e| format!("writing the keymap: {e}"))?;
        // keymap(format XKB_V1, fd, size)
        self.w.send_fd(self.keyboard, 0, &Args::new().u32(1).u32(text.len() as u32), f.as_raw_fd())
    }

    fn frame(&mut self) -> std::result::Result<(), String> {
        self.w.send(self.pointer, 4, &Args::new())
    }

    fn fixed(v: f64) -> u32 {
        (v * 256.0).round() as i32 as u32
    }

    fn motion_absolute(&mut self, x: f64, y: f64, layout: Rect) -> std::result::Result<(), String> {
        // 1/8 px resolution over the whole layout box (no output bound: Hyprland maps the extent
        // onto every monitor's union).
        let k = 8.0;
        let (ex, ey) = ((layout.width * k).round().max(1.0), (layout.height * k).round().max(1.0));
        let ux = ((x - layout.x) * k).round().clamp(0.0, ex - 1.0);
        let uy = ((y - layout.y) * k).round().clamp(0.0, ey - 1.0);
        let t = self.time();
        self.w.send(self.pointer, 1, &Args::new().u32(t).u32(ux as u32).u32(uy as u32).u32(ex as u32).u32(ey as u32))?;
        self.frame()
    }

    fn motion_relative(&mut self, dx: f64, dy: f64) -> std::result::Result<(), String> {
        let t = self.time();
        self.w.send(self.pointer, 0, &Args::new().u32(t).u32(Self::fixed(dx)).u32(Self::fixed(dy)))?;
        self.frame()
    }

    fn button(&mut self, b: Button, pressed: bool) -> std::result::Result<(), String> {
        let t = self.time();
        self.w.send(self.pointer, 2, &Args::new().u32(t).u32(keymap::button_code(b)).u32(pressed as u32))?;
        self.frame()
    }

    fn scroll(&mut self, axis: Axis, steps: i32) -> std::result::Result<(), String> {
        let t = self.time();
        let a = match axis {
            Axis::Vertical => 0,
            Axis::Horizontal => 1,
        };
        self.w.send(self.pointer, 5, &Args::new().u32(0))?; // axis_source(wheel)
        self.w.send(self.pointer, 7, &Args::new().u32(t).u32(a).u32(Self::fixed(15.0 * steps as f64)).u32(steps as u32))?; // axis_discrete
        self.frame()
    }

    fn ensure(&mut self, syms: &[u32]) -> std::result::Result<(), String> {
        if self.keys.assign(syms) {
            self.upload_keymap()?;
            self.roundtrip()?;
        }
        Ok(())
    }

    fn send_mods(&mut self, mods: u32) -> std::result::Result<(), String> {
        self.w.send(self.keyboard, 2, &Args::new().u32(mods).u32(0).u32(0).u32(0))
    }

    fn raw_key(&mut self, code: u32, pressed: bool) -> std::result::Result<(), String> {
        let t = self.time();
        self.w.send(self.keyboard, 1, &Args::new().u32(t).u32(code).u32(pressed as u32))
    }

    /// Press or release the key that produces `sym`. A shifted symbol pressed without Shift held
    /// gets Shift for that key only.
    fn key(&mut self, sym: u32, pressed: bool) -> std::result::Result<(), String> {
        self.ensure(&[sym])?;
        let (code, shift) = self.keys.lookup(sym).ok_or_else(|| format!("no keycode left for keysym 0x{sym:x}"))?;
        let add = shift && self.mods & 1 == 0;
        if add {
            self.send_mods(self.mods | 1)?;
        }
        self.raw_key(code, pressed)?;
        if add {
            self.send_mods(self.mods)?;
        }
        Ok(())
    }

    fn modifier(&mut self, m: Modifier, pressed: bool) -> std::result::Result<(), String> {
        let sym = modifier_sym(m).ok_or_else(|| format!("{m:?} does not exist on Linux"))?;
        let (code, _) = self.keys.lookup(sym).ok_or("modifier missing from the keymap")?;
        self.raw_key(code, pressed)?;
        let bit = modifier_mask(m);
        if pressed { self.mods |= bit } else { self.mods &= !bit }
        self.send_mods(self.mods)
    }

    /// Type text: a press and release per character, in parts when it needs more symbols
    /// without a US key than there are free keycodes.
    fn text(&mut self, s: &str) -> std::result::Result<(), String> {
        let syms: Vec<u32> = s.chars().map(keymap::char_keysym).collect();
        let cap = self.keys.capacity().max(1);
        let mut i = 0;
        while i < syms.len() {
            let mut extras: Vec<u32> = vec![];
            let mut j = i;
            while j < syms.len() {
                let s = syms[j];
                let fixed = self.keys.lookup(s).is_some_and(|_| !self.keys.extra.iter().any(|(_, e)| *e == s));
                if !fixed && !extras.contains(&s) {
                    if extras.len() == cap {
                        break;
                    }
                    extras.push(s);
                }
                j += 1;
            }
            self.ensure(&extras)?;
            for &sym in &syms[i..j] {
                self.key(sym, true)?;
                self.key(sym, false)?;
            }
            i = j;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- adapter

#[derive(Default)]
struct Shared {
    input: Option<VirtualInput>,
    /// Where this adapter last put the pointer; anything else means a person moved it.
    expected: Option<(f64, f64)>,
    pending: Vec<SessionEvent>,
    generation: u64,
}

pub struct HyprlandAdapter {
    ipc: HyprIpc,
    grim: Option<PathBuf>,
    s: Arc<Mutex<Shared>>,
}

fn which(cmd: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(cmd)).find(|f| f.is_file()))
}

impl HyprlandAdapter {
    /// Running inside a Hyprland session with its IPC socket present.
    pub fn detect() -> bool {
        HyprIpc::from_env().is_some()
    }

    pub fn new(ipc: HyprIpc) -> Self {
        let s = Arc::new(Mutex::new(Shared { generation: 1, ..Default::default() }));
        Self::watch_events(&ipc, s.clone());
        Self { ipc, grim: which("grim"), s }
    }

    pub fn from_env() -> Result<Self> {
        HyprIpc::from_env().map(Self::new).ok_or_else(|| AdlError::new(ErrorCode::Blocked, "no Hyprland session (HYPRLAND_INSTANCE_SIGNATURE and its IPC socket are missing)"))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.s.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Monitor and config changes arrive on `.socket2.sock` as `event>>data` lines.
    fn watch_events(ipc: &HyprIpc, s: Arc<Mutex<Shared>>) {
        let path = ipc.events_socket();
        std::thread::spawn(move || loop {
            if let Ok(sock) = UnixStream::connect(&path) {
                for line in BufReader::new(sock).lines() {
                    let Ok(line) = line else { break };
                    let ev = line.split(">>").next().unwrap_or("");
                    if matches!(ev, "monitoradded" | "monitoraddedv2" | "monitorremoved" | "monitorremovedv2" | "configreloaded") {
                        let mut g = s.lock().unwrap_or_else(|e| e.into_inner());
                        g.generation += 1;
                        g.pending.push(SessionEvent::LayoutChanged);
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        });
    }

    fn monitors(&self) -> Result<Vec<Monitor>> {
        Ok(parse_monitors(&self.ipc.json("monitors")?))
    }

    fn clients(&self) -> Result<Vec<Client>> {
        Ok(parse_clients(&self.ipc.json("clients")?))
    }

    /// The union of all monitors, which absolute virtual-pointer coordinates span.
    fn layout_box(monitors: &[Monitor]) -> Rect {
        let rects: Vec<Rect> = monitors.iter().map(Monitor::logical).collect();
        let x0 = rects.iter().map(|r| r.x).fold(f64::INFINITY, f64::min);
        let y0 = rects.iter().map(|r| r.y).fold(f64::INFINITY, f64::min);
        let x1 = rects.iter().map(|r| r.x + r.width).fold(f64::NEG_INFINITY, f64::max);
        let y1 = rects.iter().map(|r| r.y + r.height).fold(f64::NEG_INFINITY, f64::max);
        if rects.is_empty() {
            return Rect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 };
        }
        Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 }
    }

    fn with_input<T>(&self, f: impl FnOnce(&mut VirtualInput) -> std::result::Result<T, String>) -> Result<T> {
        let mut g = self.lock();
        let vi = g.input.as_mut().ok_or_else(|| AdlError::new(ErrorCode::Disabled, "virtual input is not open; enable agent access first"))?;
        match f(vi).and_then(|t| vi.roundtrip().map(|_| t)) {
            Ok(t) => Ok(t),
            Err(e) => {
                // A broken connection cannot be trusted with held keys: drop it, which releases them.
                g.input = None;
                g.pending.push(SessionEvent::AdapterFailed(format!("Hyprland virtual input: {e}")));
                Err(AdlError::new(ErrorCode::OutcomeUnknown, format!("Hyprland virtual input: {e}")))
            }
        }
    }
}

impl Adapter for HyprlandAdapter {
    fn name(&self) -> String {
        BACKEND.into()
    }

    fn capabilities(&self) -> Vec<CapabilityReport> {
        let rep = |c, a, reason: &str, f: &[&str]| CapabilityReport {
            capability: c, availability: a, backend: BACKEND.into(), reason: reason.into(), features: f.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(),
        };
        let capture = if self.grim.is_some() {
            rep(Capability::ScreenCapture, Availability::Available, "", &["grim", "wlr_screencopy"])
        } else {
            rep(Capability::ScreenCapture, Availability::Unsupported, "install grim (wlr-screencopy) for screenshots", &[])
        };
        vec![
            rep(Capability::Pointer, Availability::Available, "", &["relative_motion", "pointer_position", "physical_input_detection", "button.back", "button.forward"]),
            rep(Capability::Keyboard, Availability::Available, "keyboard takeover is not detectable on this backend", &["unicode_text", "generated_keymap"]),
            capture,
            rep(Capability::Windows, Availability::Available, "", &["window_at", "focus", "geometry"]),
            rep(Capability::Accessibility, Availability::Unsupported, "the AT-SPI adapter is not implemented yet", &[]),
            rep(Capability::ClipboardRead, Availability::Unsupported, "not implemented", &[]),
            rep(Capability::ClipboardWrite, Availability::Unsupported, "not implemented", &[]),
        ]
    }

    fn begin(&self) -> Result<()> {
        if self.monitors()?.is_empty() {
            return Err(AdlError::new(ErrorCode::Blocked, "Hyprland reports no enabled monitors"));
        }
        let vi = VirtualInput::open().map_err(|e| AdlError::new(ErrorCode::Blocked, format!("Hyprland virtual input: {e}")))?;
        let mut g = self.lock();
        g.input = Some(vi);
        g.expected = None;
        Ok(())
    }

    fn end(&self) {
        let mut g = self.lock();
        g.input = None;
        g.expected = None;
    }

    fn layout(&self) -> Result<Layout> {
        let displays = self.monitors()?.iter().map(Monitor::display).collect();
        Ok(Layout { generation: self.lock().generation, displays })
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        let (mons, clients) = (self.monitors()?, self.clients()?);
        Ok(visible(&clients, &mons).into_iter().map(|c| window_info(c, &mons)).collect())
    }

    fn focused_window(&self) -> Result<Option<WindowInfo>> {
        let (mons, clients) = (self.monitors()?, self.clients()?);
        Ok(visible(&clients, &mons).into_iter().find(|c| c.focus_history == 0).map(|c| window_info(c, &mons)))
    }

    fn window_at(&self, x: f64, y: f64) -> Result<Option<WindowInfo>> {
        let (mons, clients) = (self.monitors()?, self.clients()?);
        Ok(visible(&clients, &mons).into_iter().find(|c| c.bounds.contains(x, y)).map(|c| window_info(c, &mons)))
    }

    fn inspect(&self, _window: &str) -> Result<AccessNode> {
        Err(AdlError::unsupported("accessibility trees on Linux need the AT-SPI adapter, which is not implemented yet"))
    }

    fn capture(&self, display: &str) -> Result<(Vec<u8>, u32, u32)> {
        let grim = self.grim.as_ref().ok_or_else(|| AdlError::unsupported("grim is not installed (w3bs.org/de installs it)"))?;
        if !self.monitors()?.iter().any(|m| m.name == display) {
            return Err(AdlError::not_found(format!("no monitor {display}")));
        }
        let out = std::process::Command::new(grim).args(["-t", "png", "-l", "1", "-o", display, "-"]).output().map_err(|e| AdlError::internal(format!("grim: {e}")))?;
        if !out.status.success() {
            return Err(AdlError::internal(format!("grim failed: {}", String::from_utf8_lossy(&out.stderr).trim())));
        }
        let png = out.stdout;
        if png.len() < 24 || &png[12..16] != b"IHDR" {
            return Err(AdlError::internal("grim did not return a PNG"));
        }
        let (w, h) = (u32::from_be_bytes(png[16..20].try_into().unwrap()), u32::from_be_bytes(png[20..24].try_into().unwrap()));
        Ok((png, w, h))
    }

    fn pointer_position(&self) -> Result<Option<(f64, f64)>> {
        let v = self.ipc.json("cursorpos")?;
        Ok(Some((num(&v, "x"), num(&v, "y"))))
    }

    fn emit(&self, event: &InputEvent) -> Result<()> {
        match event {
            InputEvent::MotionAbsolute { x, y } => {
                let b = Self::layout_box(&self.monitors()?);
                self.with_input(|vi| vi.motion_absolute(*x, *y, b))?;
                self.lock().expected = Some((*x, *y));
            }
            InputEvent::MotionRelative { dx, dy } => {
                self.with_input(|vi| vi.motion_relative(*dx, *dy))?;
                // Acceleration may apply; re-read where it landed.
                self.lock().expected = self.pointer_position().ok().flatten();
            }
            InputEvent::Button { button, pressed } => self.with_input(|vi| vi.button(*button, *pressed))?,
            InputEvent::Scroll { axis, steps } => self.with_input(|vi| vi.scroll(*axis, *steps))?,
            InputEvent::Key { key: Key::Modifier(m), pressed } => self.with_input(|vi| vi.modifier(*m, *pressed))?,
            InputEvent::Key { key, pressed } => {
                let sym = keymap::keysym(key).ok_or_else(|| AdlError::unsupported(format!("no keysym for {key}")))?;
                self.with_input(|vi| vi.key(sym, *pressed))?
            }
            InputEvent::Text { text } => self.with_input(|vi| vi.text(text))?,
        }
        Ok(())
    }

    fn preflight(&self, action: &Action) -> Result<()> {
        if let Action::KeyPress { key } | Action::KeyDown { key } | Action::KeyUp { key } = action
            && matches!(key, Key::Modifier(Modifier::Command | Modifier::Option)) {
                return Err(AdlError::unsupported("Command and Option are macOS keys; use primary, ctrl or alt"));
            }
        Ok(())
    }

    fn safety_interlock(&self) -> Option<String> {
        if let Some(l) = locker_running() {
            return Some(format!("the session is locked ({l})"));
        }
        match self.clients() {
            Ok(c) => auth_dialog(&c).map(|d| format!("an authentication dialog ({}) is on screen; the person must answer it", d.class)),
            Err(e) => Some(format!("cannot read Hyprland state: {}", e.message)),
        }
    }

    fn poll_events(&self) -> Vec<SessionEvent> {
        let expected = self.lock().expected;
        if let Some((ex, ey)) = expected
            && let Ok(Some((x, y))) = self.pointer_position() {
                // Only the pointer can be checked; one report per departure.
                if (x - ex).abs() > 4.0 || (y - ey).abs() > 4.0 {
                    let mut g = self.lock();
                    if g.expected == expected {
                        g.expected = None;
                        g.pending.push(SessionEvent::PhysicalInput);
                    }
                }
            }
        std::mem::take(&mut self.lock().pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mons() -> Vec<Monitor> {
        parse_monitors(&json!([
            {"id": 0, "name": "DP-1", "width": 2560, "height": 1440, "x": 0, "y": 0, "scale": 1.25, "transform": 0,
             "activeWorkspace": {"id": 1, "name": "1"}, "specialWorkspace": {"id": 0, "name": ""}, "focused": true, "disabled": false},
            {"id": 1, "name": "HDMI-A-1", "width": 1920, "height": 1080, "x": 2048, "y": -200, "scale": 1.0, "transform": 1,
             "activeWorkspace": {"id": 4, "name": "4"}, "specialWorkspace": {"id": -98, "name": "special:term"}, "focused": false, "disabled": false},
            {"id": 2, "name": "eDP-1", "width": 1, "height": 1, "x": 0, "y": 0, "scale": 1, "transform": 0, "disabled": true}
        ]))
    }

    fn client(addr: &str, class: &str, at: [f64; 2], size: [f64; 2], ws: i64, focus: i64) -> Value {
        json!({"address": addr, "mapped": true, "hidden": false, "at": at, "size": size, "workspace": {"id": ws, "name": ws.to_string()},
               "floating": false, "monitor": 0, "class": class, "title": class, "pinned": false, "fullscreen": 0, "focusHistoryID": focus})
    }

    #[test]
    fn monitors_map_to_logical_displays() {
        let m = mons();
        assert_eq!(m.len(), 2, "disabled monitors are left out");
        let d = m[0].display();
        assert_eq!(d.logical, Rect { x: 0.0, y: 0.0, width: 2048.0, height: 1152.0 });
        assert_eq!(d.pixel_size(), (2560, 1440));
        let r = m[1].display();
        assert_eq!(r.rotation, Rotation::Rotate90);
        assert_eq!(r.logical, Rect { x: 2048.0, y: -200.0, width: 1080.0, height: 1920.0 }, "a quarter turn swaps width and height");
        assert_eq!(HyprlandAdapter::layout_box(&m), Rect { x: 0.0, y: -200.0, width: 3128.0, height: 1920.0 });
    }

    #[test]
    fn stacking_puts_special_fullscreen_and_floating_first() {
        let mut floating = client("0xf", "pavucontrol", [100.0, 100.0], [400.0, 300.0], 1, 1);
        floating["floating"] = json!(true);
        let mut full = client("0xfs", "mpv", [0.0, 0.0], [2048.0, 1152.0], 1, 2);
        full["fullscreen"] = json!(2);
        let mut hidden = client("0xh", "foot", [0.0, 0.0], [10.0, 10.0], 1, 5);
        hidden["hidden"] = json!(true);
        let cs = parse_clients(&json!([
            client("0xa", "foot", [0.0, 0.0], [1024.0, 1152.0], 1, 0),
            floating,
            full,
            client("0xs", "foot", [2100.0, 0.0], [500.0, 500.0], -98, 3),
            client("0xo", "firefox", [0.0, 0.0], [2048.0, 1152.0], 2, 4),
            hidden
        ]));
        let v: Vec<&str> = visible(&cs, &mons()).iter().map(|c| c.address.as_str()).collect();
        assert_eq!(v, ["0xs", "0xfs", "0xf", "0xa"], "other workspaces and hidden windows are not on screen");
        let w = window_info(visible(&cs, &mons())[3], &mons());
        assert!(w.focused && w.display.as_deref() == Some("DP-1"));
    }

    #[test]
    fn authentication_dialogs_are_found() {
        let cs = parse_clients(&json!([client("0x1", "foot", [0.0, 0.0], [1.0, 1.0], 1, 0), client("0x2", "hyprpolkitagent", [0.0, 0.0], [1.0, 1.0], 1, 1)]));
        assert_eq!(auth_dialog(&cs).unwrap().address, "0x2");
        assert!(auth_dialog(&cs[..1]).is_none());
    }

    #[test]
    fn keys_sit_at_their_us_positions() {
        let t = KeyTable::new(FREE_US_CODES);
        // What a binding sees is what a real keyboard would send.
        assert_eq!(t.lookup('e' as u32), Some((18, false)));
        assert_eq!(t.lookup('E' as u32), Some((18, true)), "uppercase is Shift + the letter key");
        assert_eq!(t.lookup('1' as u32), Some((2, false)));
        assert_eq!(t.lookup('!' as u32), Some((2, true)));
        assert_eq!(t.lookup(0xff0d), Some((28, false)), "Enter");
        assert_eq!(t.lookup(SUPER_L), Some((125, false)));
        // Never on a media key: evdev 113-115 are Mute and Volume on a US keymap.
        for code in 113..=115 {
            assert!(!t.fixed.contains_key(&code), "code {code} must stay unused");
        }
        let km = t.keymap();
        assert!(km.contains("key <C18> { [ 0x00000065, 0x00000045 ] };"), "e/E");
        assert!(km.contains("modifier_map Mod4 { <C125> };"));
        assert!(km.contains("<C18> = 26;"));
    }

    #[test]
    fn other_symbols_use_only_free_codes_and_make_room() {
        let mut t = KeyTable::new(FREE_US_CODES);
        assert_eq!(t.capacity(), FREE_US_CODES.len());
        let e_acute = keymap::char_keysym('é');
        assert!(t.lookup(e_acute).is_none());
        assert!(t.assign(&[e_acute]));
        let (code, shift) = t.lookup(e_acute).unwrap();
        assert!(FREE_US_CODES.contains(&code) && !shift);
        assert!(t.keymap().contains(&format!("key <C{code}> {{ [ 0x{e_acute:08x} ] }};")));
        assert!(!t.assign(&[e_acute, 'a' as u32]), "nothing new to place");
        // A full set of new symbols evicts the ones no longer wanted.
        let cjk: Vec<u32> = (0..FREE_US_CODES.len() as u32).map(|i| 0x0100_4e00 + i).collect();
        assert!(t.assign(&cjk));
        assert!(t.lookup(e_acute).is_none());
        assert!(cjk.iter().all(|s| t.lookup(*s).is_some_and(|(c, _)| FREE_US_CODES.contains(&c))));
    }

    #[test]
    fn modifier_masks() {
        assert_eq!(modifier_mask(Modifier::Shift) | modifier_mask(Modifier::Primary) | modifier_mask(Modifier::Alt) | modifier_mask(Modifier::Super), 1 | 4 | 8 | 64);
    }

    #[test]
    fn ipc_requests_one_connection_each() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".socket.sock");
        let l = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let t = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut s, _) = l.accept().unwrap();
                let mut buf = [0u8; 64];
                let n = s.read(&mut buf).unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let reply = if req == "j/cursorpos" { r#"{"x": 12.5, "y": 40}"# } else { "[]" };
                s.write_all(reply.as_bytes()).unwrap();
            }
        });
        let ipc = HyprIpc::at(dir.path().into());
        assert_eq!(ipc.json("cursorpos").unwrap()["x"], 12.5);
        assert!(parse_clients(&ipc.json("clients").unwrap()).is_empty());
        t.join().unwrap();
    }
}
