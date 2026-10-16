//! Read portal ScreenCast streams with libpipewire, loaded at run time.
//!
//! The portal hands over a PipeWire socket (`OpenPipeWireRemote`) for the session's shared
//! monitors; each monitor is one stream. The compositor sends a buffer only when pixels change
//! (and at most [`MAX_FPS`] a second, negotiated); we keep just the newest buffer of each stream
//! and copy pixels out only when the recorder asks, cropped to the window it looks at. Nothing is
//! written anywhere. No GStreamer and no build-time PipeWire dependency: without
//! `libpipewire-0.3.so.0` nothing starts and the recorder keeps using screenshots.
//!
//! Only shared-memory buffers are accepted (no DMA-BUF modifiers are offered), so mapped buffers
//! can be read directly.

use adl_broker::{PixelRect, StreamPixels};
use std::ffi::{c_char, c_int, c_void};
use std::os::fd::{IntoRawFd, OwnedFd};
use std::sync::{Mutex, OnceLock};

/// Most frames a second the compositor may send each stream.
pub const MAX_FPS: u32 = 5;

type P = *mut c_void;
type Unused = Option<unsafe extern "C" fn()>;

struct Lib {
    thread_loop_new: unsafe extern "C" fn(*const c_char, *const c_void) -> P,
    thread_loop_get_loop: unsafe extern "C" fn(P) -> P,
    thread_loop_start: unsafe extern "C" fn(P) -> c_int,
    thread_loop_stop: unsafe extern "C" fn(P),
    thread_loop_destroy: unsafe extern "C" fn(P),
    thread_loop_lock: unsafe extern "C" fn(P),
    thread_loop_unlock: unsafe extern "C" fn(P),
    context_new: unsafe extern "C" fn(P, P, usize) -> P,
    context_connect_fd: unsafe extern "C" fn(P, c_int, P, usize) -> P,
    context_destroy: unsafe extern "C" fn(P),
    core_disconnect: unsafe extern "C" fn(P) -> c_int,
    properties_new_string: unsafe extern "C" fn(*const c_char) -> P,
    stream_new: unsafe extern "C" fn(P, *const c_char, P) -> P,
    stream_add_listener: unsafe extern "C" fn(P, *mut SpaHook, *const StreamEvents, P),
    stream_connect: unsafe extern "C" fn(P, u32, u32, u32, *mut *const c_void, u32) -> c_int,
    stream_update_params: unsafe extern "C" fn(P, *mut *const c_void, u32) -> c_int,
    stream_dequeue_buffer: unsafe extern "C" fn(P) -> *mut PwBuffer,
    stream_queue_buffer: unsafe extern "C" fn(P, *mut PwBuffer) -> c_int,
    stream_destroy: unsafe extern "C" fn(P),
}

// Each symbol is transmuted to the field type it is assigned to (pipewire/*.h signatures).
#[allow(clippy::missing_transmute_annotations)]
fn lib() -> Option<&'static Lib> {
    static LIB: OnceLock<Option<Lib>> = OnceLock::new();
    LIB.get_or_init(|| unsafe {
        let h = libc::dlopen(c"libpipewire-0.3.so.0".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if h.is_null() {
            return None;
        }
        macro_rules! sym {
            ($name:literal) => {{
                let p = libc::dlsym(h, $name.as_ptr());
                if p.is_null() {
                    return None;
                }
                std::mem::transmute::<*mut c_void, _>(p)
            }};
        }
        let init: unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char) = sym!(c"pw_init");
        init(std::ptr::null_mut(), std::ptr::null_mut());
        Some(Lib {
            thread_loop_new: sym!(c"pw_thread_loop_new"),
            thread_loop_get_loop: sym!(c"pw_thread_loop_get_loop"),
            thread_loop_start: sym!(c"pw_thread_loop_start"),
            thread_loop_stop: sym!(c"pw_thread_loop_stop"),
            thread_loop_destroy: sym!(c"pw_thread_loop_destroy"),
            thread_loop_lock: sym!(c"pw_thread_loop_lock"),
            thread_loop_unlock: sym!(c"pw_thread_loop_unlock"),
            context_new: sym!(c"pw_context_new"),
            context_connect_fd: sym!(c"pw_context_connect_fd"),
            context_destroy: sym!(c"pw_context_destroy"),
            core_disconnect: sym!(c"pw_core_disconnect"),
            properties_new_string: sym!(c"pw_properties_new_string"),
            stream_new: sym!(c"pw_stream_new"),
            stream_add_listener: sym!(c"pw_stream_add_listener"),
            stream_connect: sym!(c"pw_stream_connect"),
            stream_update_params: sym!(c"pw_stream_update_params"),
            stream_dequeue_buffer: sym!(c"pw_stream_dequeue_buffer"),
            stream_queue_buffer: sym!(c"pw_stream_queue_buffer"),
            stream_destroy: sym!(c"pw_stream_destroy"),
        })
    })
    .as_ref()
}

// ---- C layouts (spa/utils/hook.h, pipewire/stream.h, spa/buffer/buffer.h) ----

/// `struct spa_hook`: a list link, callbacks, `removed` and `priv`; filled in by PipeWire.
#[repr(C)]
pub struct SpaHook([usize; 6]);

#[repr(C)]
pub struct StreamEvents {
    version: u32,
    destroy: Unused,
    state_changed: Option<unsafe extern "C" fn(P, c_int, c_int, *const c_char)>,
    control_info: Unused,
    io_changed: Unused,
    param_changed: Option<unsafe extern "C" fn(P, u32, *const u8)>,
    add_buffer: Unused,
    remove_buffer: Option<unsafe extern "C" fn(P, *mut PwBuffer)>,
    process: Option<unsafe extern "C" fn(P)>,
    drained: Unused,
    command: Unused,
    trigger_done: Unused,
}

#[repr(C)]
pub struct PwBuffer {
    buffer: *mut SpaBuffer,
}

#[repr(C)]
struct SpaBuffer {
    n_metas: u32,
    n_datas: u32,
    metas: P,
    datas: *mut SpaData,
}

#[repr(C)]
struct SpaData {
    kind: u32,
    flags: u32,
    fd: i64,
    mapoffset: u32,
    maxsize: u32,
    data: P,
    chunk: *mut SpaChunk,
}

#[repr(C)]
struct SpaChunk {
    offset: u32,
    size: u32,
    stride: i32,
    flags: i32,
}

const PW_DIRECTION_INPUT: u32 = 0;
const PW_STREAM_FLAG_AUTOCONNECT: u32 = 1 << 0;
const PW_STREAM_FLAG_MAP_BUFFERS: u32 = 1 << 2;
const PW_STREAM_STATE_ERROR: c_int = -1;
const SPA_PARAM_FORMAT: u32 = 4;
const SPA_CHUNK_FLAG_CORRUPTED: i32 = 1;

// spa/param/video/raw.h
const FMT_RGBX: u32 = 7;
const FMT_BGRX: u32 = 8;
const FMT_RGBA: u32 = 11;
const FMT_BGRA: u32 = 12;

// ---- SPA POD encoding (spa/pod/pod.h): 8-byte aligned, native endian ----

mod pod {
    const ID: u32 = 3;
    const INT: u32 = 4;
    const RECTANGLE: u32 = 10;
    const FRACTION: u32 = 11;
    const OBJECT: u32 = 15;
    const CHOICE: u32 = 19;
    const CHOICE_RANGE: u32 = 1;
    const CHOICE_ENUM: u32 = 3;
    const CHOICE_FLAGS: u32 = 4;

    fn u32s(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_ne_bytes()).collect()
    }

    fn pod(kind: u32, body: &[u8]) -> Vec<u8> {
        let mut v = u32s(&[body.len() as u32, kind]);
        v.extend_from_slice(body);
        while !v.len().is_multiple_of(8) {
            v.push(0);
        }
        v
    }

    pub fn id(x: u32) -> Vec<u8> {
        pod(ID, &x.to_ne_bytes())
    }

    fn choice(kind: u32, child: u32, child_size: u32, values: &[u32]) -> Vec<u8> {
        let mut body = u32s(&[kind, 0, child_size, child]);
        body.extend(u32s(values));
        pod(CHOICE, &body)
    }

    pub fn enum_ids(default: u32, alternatives: &[u32]) -> Vec<u8> {
        let mut v = vec![default];
        v.extend_from_slice(alternatives);
        choice(CHOICE_ENUM, ID, 4, &v)
    }

    pub fn range_rect(def: (u32, u32), min: (u32, u32), max: (u32, u32)) -> Vec<u8> {
        choice(CHOICE_RANGE, RECTANGLE, 8, &[def.0, def.1, min.0, min.1, max.0, max.1])
    }

    pub fn range_fraction(def: (u32, u32), min: (u32, u32), max: (u32, u32)) -> Vec<u8> {
        choice(CHOICE_RANGE, FRACTION, 8, &[def.0, def.1, min.0, min.1, max.0, max.1])
    }

    pub fn range_int(def: u32, min: u32, max: u32) -> Vec<u8> {
        choice(CHOICE_RANGE, INT, 4, &[def, min, max])
    }

    pub fn flags_int(flags: u32) -> Vec<u8> {
        choice(CHOICE_FLAGS, INT, 4, &[flags])
    }

    pub fn object(kind: u32, id: u32, props: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut body = u32s(&[kind, id]);
        for (key, value) in props {
            body.extend(u32s(&[*key, 0]));
            body.extend_from_slice(value);
        }
        pod(OBJECT, &body)
    }

    /// Copy into 8-byte aligned storage, as PipeWire reads PODs in place.
    pub fn aligned(bytes: &[u8]) -> Vec<u64> {
        let mut out = vec![0u64; bytes.len().div_ceil(8)];
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out.as_mut_ptr().cast::<u8>(), bytes.len()) };
        out
    }

    /// (video format, width, height) from a negotiated Format object.
    ///
    /// # Safety
    /// `p` must point to a complete SPA POD.
    pub unsafe fn parse_format(p: *const u8) -> Option<(u32, u32, u32)> {
        let rd = |at: *const u8| unsafe { std::ptr::read_unaligned(at.cast::<u32>()) };
        let (size, kind) = (rd(p), rd(unsafe { p.add(4) }));
        if kind != OBJECT {
            return None;
        }
        let body = unsafe { p.add(8) };
        let end = unsafe { body.add(size as usize) };
        let mut at = unsafe { body.add(8) };
        let (mut fmt, mut w, mut h) = (None, None, None);
        while (at as usize) + 16 <= end as usize {
            let key = rd(at);
            let (vsize, mut vtype) = (rd(unsafe { at.add(8) }), rd(unsafe { at.add(12) }));
            let mut value = unsafe { at.add(16) };
            if vtype == CHOICE {
                vtype = rd(unsafe { value.add(12) });
                value = unsafe { value.add(16) };
            }
            match (key, vtype) {
                (0x20001, ID) => fmt = Some(rd(value)),
                (0x20003, RECTANGLE) => {
                    w = Some(rd(value));
                    h = Some(rd(unsafe { value.add(4) }));
                }
                _ => {}
            }
            at = unsafe { at.add(16 + (vsize as usize).div_ceil(8) * 8) };
        }
        Some((fmt?, w?, h?))
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn encodes_and_reads_back_a_format() {
            let f = super::object(0x40003, 4, &[(1, super::id(2)), (0x20001, super::enum_ids(8, &[12])), (0x20003, super::range_rect((2560, 1440), (1, 1), (8, 8)))]);
            assert_eq!(f.len() % 8, 0);
            // Header: body size then type 15 (object); object type 0x40003, id 4.
            assert_eq!(u32::from_ne_bytes(f[4..8].try_into().unwrap()), 15);
            assert_eq!(u32::from_ne_bytes(f[8..12].try_into().unwrap()), 0x40003);
            assert_eq!(u32::from_ne_bytes(f[0..4].try_into().unwrap()) as usize, f.len() - 8);
            let a = super::aligned(&f);
            assert_eq!(unsafe { super::parse_format(a.as_ptr().cast()) }, Some((8, 2560, 1440)));
            // An Id pod is 4 bytes of body padded to 8.
            let want: Vec<u8> = [4u32, 3, 7, 0].iter().flat_map(|x| x.to_ne_bytes()).collect();
            assert_eq!(super::id(7), want);
        }
    }
}

// ---- streams ----

struct Latest {
    held: *mut PwBuffer,
    seq: u64,
    format: u32,
    width: u32,
    height: u32,
    failed: bool,
}

// The pointer is only touched under the mutex, by the PipeWire loop thread and the reader.
unsafe impl Send for Latest {}

struct StreamCtx {
    lib: &'static Lib,
    stream: P,
    latest: Mutex<Latest>,
}

unsafe extern "C" fn on_state(data: P, _old: c_int, state: c_int, _error: *const c_char) {
    let ctx = unsafe { &*(data as *const StreamCtx) };
    if state == PW_STREAM_STATE_ERROR {
        let mut l = ctx.latest.lock().unwrap();
        l.failed = true;
        l.held = std::ptr::null_mut();
    }
}

unsafe extern "C" fn on_param(data: P, id: u32, param: *const u8) {
    if id != SPA_PARAM_FORMAT || param.is_null() {
        return;
    }
    let ctx = unsafe { &*(data as *const StreamCtx) };
    let Some((format, width, height)) = (unsafe { pod::parse_format(param) }) else { return };
    {
        let mut l = ctx.latest.lock().unwrap();
        (l.format, l.width, l.height) = (format, width, height);
    }
    // Shared memory only (MemPtr | MemFd), so mapped buffers can be read in place.
    let buffers = pod::aligned(&pod::object(0x40004, 5, &[(1, pod::range_int(4, 2, 16)), (6, pod::flags_int((1 << 1) | (1 << 2)))]));
    let mut params = [buffers.as_ptr() as *const c_void];
    unsafe { (ctx.lib.stream_update_params)(ctx.stream, params.as_mut_ptr(), 1) };
}

unsafe extern "C" fn on_remove_buffer(data: P, buffer: *mut PwBuffer) {
    let ctx = unsafe { &*(data as *const StreamCtx) };
    let mut l = ctx.latest.lock().unwrap();
    if l.held == buffer {
        l.held = std::ptr::null_mut();
    }
}

unsafe fn usable(b: *mut PwBuffer) -> bool {
    unsafe {
        let sb = (*b).buffer;
        if sb.is_null() || (*sb).n_datas == 0 || (*sb).datas.is_null() {
            return false;
        }
        let d = &*(*sb).datas;
        !d.data.is_null() && !d.chunk.is_null() && (*d.chunk).size > 0 && (*d.chunk).flags & SPA_CHUNK_FLAG_CORRUPTED == 0
    }
}

unsafe extern "C" fn on_process(data: P) {
    let ctx = unsafe { &*(data as *const StreamCtx) };
    let (dequeue, queue) = (ctx.lib.stream_dequeue_buffer, ctx.lib.stream_queue_buffer);
    // Keep only the newest buffer; hand every older one straight back.
    let mut newest: *mut PwBuffer = std::ptr::null_mut();
    loop {
        let b = unsafe { dequeue(ctx.stream) };
        if b.is_null() {
            break;
        }
        if !newest.is_null() {
            unsafe { queue(ctx.stream, newest) };
        }
        newest = b;
    }
    if newest.is_null() {
        return;
    }
    if !unsafe { usable(newest) } {
        unsafe { queue(ctx.stream, newest) };
        return;
    }
    let mut l = ctx.latest.lock().unwrap();
    if !l.held.is_null() {
        unsafe { queue(ctx.stream, l.held) };
    }
    l.held = newest;
    l.seq += 1;
}

static EVENTS: StreamEvents = StreamEvents {
    version: 2,
    destroy: None,
    state_changed: Some(on_state),
    control_info: None,
    io_changed: None,
    param_changed: Some(on_param),
    add_buffer: None,
    remove_buffer: Some(on_remove_buffer),
    process: Some(on_process),
    drained: None,
    command: None,
    trigger_done: None,
};

struct Stream {
    node: u32,
    ctx: Box<StreamCtx>,
    _hook: Box<SpaHook>,
}

/// Live streams of the portal session's monitors.
pub struct Capture {
    lib: &'static Lib,
    thread_loop: P,
    context: P,
    core: P,
    streams: Vec<Stream>,
}

// PipeWire objects are only used under the thread loop's lock or after it stopped.
unsafe impl Send for Capture {}
unsafe impl Sync for Capture {}

impl Capture {
    /// Connect to the portal's PipeWire socket and start one stream per monitor node.
    pub fn start(fd: OwnedFd, nodes: &[u32]) -> Option<Capture> {
        let lib = lib()?;
        unsafe {
            let thread_loop = (lib.thread_loop_new)(c"w3bs-recorder".as_ptr(), std::ptr::null());
            if thread_loop.is_null() {
                return None;
            }
            let context = (lib.context_new)((lib.thread_loop_get_loop)(thread_loop), std::ptr::null_mut(), 0);
            if context.is_null() {
                (lib.thread_loop_destroy)(thread_loop);
                return None;
            }
            let core = (lib.context_connect_fd)(context, fd.into_raw_fd(), std::ptr::null_mut(), 0);
            let mut cap = Capture { lib, thread_loop, context, core, streams: vec![] };
            if core.is_null() {
                return None; // Drop tears down the context and loop.
            }
            let format = pod::aligned(&pod::object(
                0x40003,
                3,
                &[
                    (1, pod::id(2)), // mediaType video
                    (2, pod::id(1)), // mediaSubtype raw
                    (0x20001, pod::enum_ids(FMT_BGRX, &[FMT_BGRX, FMT_BGRA, FMT_RGBX, FMT_RGBA])),
                    (0x20003, pod::range_rect((1920, 1080), (1, 1), (16384, 16384))),
                    (0x20004, pod::range_fraction((0, 1), (0, 1), (1000, 1))),
                    (0x20005, pod::range_fraction((MAX_FPS, 1), (0, 1), (MAX_FPS, 1))),
                ],
            ));
            for &node in nodes {
                let props = (lib.properties_new_string)(c"media.type=Video media.category=Capture media.role=Screen".as_ptr());
                let stream = (lib.stream_new)(core, c"w3bs-recorder".as_ptr(), props);
                if stream.is_null() {
                    continue;
                }
                let ctx = Box::new(StreamCtx { lib, stream, latest: Mutex::new(Latest { held: std::ptr::null_mut(), seq: 0, format: 0, width: 0, height: 0, failed: false }) });
                let mut hook = Box::new(SpaHook([0; 6]));
                (lib.stream_add_listener)(stream, &mut *hook, &EVENTS, &*ctx as *const StreamCtx as P);
                let mut params = [format.as_ptr() as *const c_void];
                if (lib.stream_connect)(stream, PW_DIRECTION_INPUT, node, PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS, params.as_mut_ptr(), 1) < 0 {
                    (lib.stream_destroy)(stream);
                    continue;
                }
                cap.streams.push(Stream { node, ctx, _hook: hook });
            }
            if cap.streams.is_empty() || (lib.thread_loop_start)(thread_loop) < 0 {
                return None;
            }
            Some(cap)
        }
    }

    /// The newest frame of `node`, with `crop` converted to RGBA.
    pub fn pixels(&self, node: u32, crop: Option<PixelRect>) -> Option<StreamPixels> {
        let s = self.streams.iter().find(|s| s.node == node)?;
        let l = s.ctx.latest.lock().unwrap();
        if l.failed || l.held.is_null() || l.width == 0 || l.height == 0 {
            return None;
        }
        let order: [usize; 3] = match l.format {
            FMT_RGBX | FMT_RGBA => [0, 1, 2],
            FMT_BGRX | FMT_BGRA => [2, 1, 0],
            _ => return None,
        };
        let Some(c) = crop else {
            return Some(StreamPixels { seq: l.seq, width: l.width, height: l.height, rgba: vec![], crop: None });
        };
        let (x, y) = (c.x.min(l.width - 1), c.y.min(l.height - 1));
        let c = PixelRect { x, y, width: c.width.min(l.width - x), height: c.height.min(l.height - y) };
        // The buffer stays ours while the lock is held: the loop thread requeues or forgets it
        // only under this same lock.
        let rgba = unsafe {
            let d = &*(*(*l.held).buffer).datas;
            let chunk = &*d.chunk;
            let stride = if chunk.stride > 0 { chunk.stride as usize } else { l.width as usize * 4 };
            let offset = (chunk.offset % d.maxsize.max(1)) as usize;
            let avail = (d.maxsize as usize).saturating_sub(offset);
            let base = (d.data as *const u8).add(offset);
            if (y + c.height - 1) as usize * stride + (x + c.width) as usize * 4 > avail {
                return None;
            }
            let mut out = Vec::with_capacity((c.width * c.height * 4) as usize);
            for row in y..y + c.height {
                let line = std::slice::from_raw_parts(base.add(row as usize * stride + x as usize * 4), c.width as usize * 4);
                for p in line.chunks_exact(4) {
                    out.extend_from_slice(&[p[order[0]], p[order[1]], p[order[2]], 0xff]);
                }
            }
            out
        };
        Some(StreamPixels { seq: l.seq, width: l.width, height: l.height, rgba, crop: Some(c) })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let lib = self.lib;
        unsafe {
            (lib.thread_loop_lock)(self.thread_loop);
            for s in &self.streams {
                // Destroying the stream frees its buffers, the held one included.
                s.ctx.latest.lock().unwrap().held = std::ptr::null_mut();
                (lib.stream_destroy)(s.ctx.stream);
            }
            if !self.core.is_null() {
                (lib.core_disconnect)(self.core);
            }
            (lib.thread_loop_unlock)(self.thread_loop);
            (lib.thread_loop_stop)(self.thread_loop);
            (lib.context_destroy)(self.context);
            (lib.thread_loop_destroy)(self.thread_loop);
        }
    }
}
