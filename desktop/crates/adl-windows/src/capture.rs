//! Monitors (layout) and per-monitor GDI capture.

use crate::win;
use adl_schema::*;
use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

/// One monitor as Windows reports it (physical pixels of the virtual screen).
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    /// Device name without the `\\.\` prefix, e.g. `DISPLAY1`.
    pub id: String,
    pub rect: RECT,
    pub dpi: u32,
    pub primary: bool,
    pub rotation: Rotation,
}

impl Monitor {
    pub fn width(&self) -> i32 {
        self.rect.right - self.rect.left
    }
    pub fn height(&self) -> i32 {
        self.rect.bottom - self.rect.top
    }

    pub fn info(&self) -> DisplayInfo {
        let pct = (self.dpi as f64 / 96.0 * 100.0).round();
        DisplayInfo {
            id: self.id.clone(),
            name: format!("{} ({} dpi, {pct}% scale)", self.id, self.dpi),
            logical: Rect { x: self.rect.left as f64, y: self.rect.top as f64, width: self.width() as f64, height: self.height() as f64 },
            // Desktop logical units are physical pixels on Windows (see the crate docs).
            scale: 1.0,
            rotation: self.rotation,
            primary: self.primary,
        }
    }
}

fn rotation_of(device: &[u16; 32]) -> Rotation {
    let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
    // SAFETY: NUL-terminated device name, correctly sized DEVMODEW.
    if !unsafe { EnumDisplaySettingsW(PCWSTR(device.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm) }.as_bool() {
        return Rotation::Normal;
    }
    // SAFETY: display devices fill the display variant of the union.
    match unsafe { dm.Anonymous1.Anonymous2.dmDisplayOrientation } {
        DMDO_90 => Rotation::Rotate90,
        DMDO_180 => Rotation::Rotate180,
        DMDO_270 => Rotation::Rotate270,
        _ => Rotation::Normal,
    }
}

pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn cb(hm: HMONITOR, _: HDC, _: *mut RECT, l: LPARAM) -> BOOL {
        // SAFETY: lparam is the Vec below, alive for the duration of EnumDisplayMonitors.
        let out = unsafe { &mut *(l.0 as *mut Vec<Monitor>) };
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: MONITORINFOEXW with cbSize set.
        if unsafe { GetMonitorInfoW(hm, &mut mi.monitorInfo) }.as_bool() {
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = unsafe { GetDpiForMonitor(hm, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
            let name = win::wstr(&mi.szDevice);
            out.push(Monitor {
                id: name.trim_start_matches("\\\\.\\").to_string(),
                rect: mi.monitorInfo.rcMonitor,
                dpi: dx,
                primary: mi.monitorInfo.dwFlags & 1 != 0, // MONITORINFOF_PRIMARY
                rotation: rotation_of(&mi.szDevice),
            });
        }
        BOOL(1)
    }
    let mut v: Vec<Monitor> = Vec::new();
    // SAFETY: the callback only pushes into `v`.
    let _ = unsafe { EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut v as *mut Vec<Monitor> as isize)) };
    v.sort_by_key(|m| (!m.primary, m.rect.left, m.rect.top));
    v
}

/// A stable fingerprint of the monitor configuration; a change bumps the layout generation.
pub fn signature(ms: &[Monitor]) -> String {
    ms.iter().map(|m| format!("{}:{},{},{},{}@{}r{:?}p{}", m.id, m.rect.left, m.rect.top, m.rect.right, m.rect.bottom, m.dpi, m.rotation, m.primary)).collect::<Vec<_>>().join(";")
}

/// BitBlt one monitor from the screen DC into a top-down 32-bit DIB and encode it as RGB PNG.
pub fn capture(m: &Monitor) -> Result<(Vec<u8>, u32, u32)> {
    let (w, h) = (m.width(), m.height());
    if w <= 0 || h <= 0 {
        return Err(AdlError::internal(format!("display {} has no area", m.id)));
    }
    let mut bgra = vec![0u8; (w as usize) * (h as usize) * 4];
    // SAFETY: GDI objects are created, used and released in order on this thread; the DIB's bits
    // are copied out before it is deleted.
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err(AdlError::new(ErrorCode::Blocked, "cannot open the screen device context"));
        }
        let mem = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(Some(screen), &bmi, DIB_RGB_COLORS, &mut bits, None, 0);
        let result = match dib {
            Ok(dib) => {
                let old = SelectObject(mem, dib.into());
                // The screen DC's origin is the primary monitor's top-left, so monitors left of or
                // above it have negative source coordinates.
                let r = BitBlt(mem, 0, 0, w, h, Some(screen), m.rect.left, m.rect.top, SRCCOPY | CAPTUREBLT);
                let _ = GdiFlush();
                if r.is_ok() && !bits.is_null() {
                    std::ptr::copy_nonoverlapping(bits as *const u8, bgra.as_mut_ptr(), bgra.len());
                }
                SelectObject(mem, old);
                let _ = DeleteObject(dib.into());
                r.map_err(|e| AdlError::new(ErrorCode::Blocked, format!("screen capture failed ({e}); a secure desktop may be active")))
            }
            Err(e) => Err(AdlError::internal(format!("CreateDIBSection: {e}"))),
        };
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        result?;
    }
    let mut rgb = Vec::with_capacity((w as usize) * (h as usize) * 3);
    for px in bgra.as_chunks::<4>().0 {
        rgb.extend_from_slice(&[px[2], px[1], px[0]]);
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut wr = enc.write_header().map_err(|e| AdlError::internal(e.to_string()))?;
        wr.write_image_data(&rgb).map_err(|e| AdlError::internal(e.to_string()))?;
    }
    Ok((out, w as u32, h as u32))
}
