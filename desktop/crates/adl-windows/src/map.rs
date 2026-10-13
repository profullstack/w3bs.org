//! Portable mapping helpers: key names to virtual-key codes, text to UTF-16 key events, and
//! virtual-screen pixels to `SendInput`'s 0..=65535 absolute range. No Win32 calls here.

use adl_schema::{Key, Modifier};

/// A virtual-key code and whether it needs `KEYEVENTF_EXTENDEDKEY`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vk {
    pub code: u16,
    pub extended: bool,
}

const fn vk(code: u16) -> Vk {
    Vk { code, extended: false }
}
const fn ext(code: u16) -> Vk {
    Vk { code, extended: true }
}

/// Virtual-key codes used here (winuser.h values).
pub mod codes {
    pub const BACK: u16 = 0x08;
    pub const TAB: u16 = 0x09;
    pub const RETURN: u16 = 0x0D;
    pub const LSHIFT: u16 = 0xA0;
    pub const LCONTROL: u16 = 0xA2;
    pub const LMENU: u16 = 0xA4;
    pub const CAPITAL: u16 = 0x14;
    pub const ESCAPE: u16 = 0x1B;
    pub const SPACE: u16 = 0x20;
    pub const PRIOR: u16 = 0x21;
    pub const NEXT: u16 = 0x22;
    pub const END: u16 = 0x23;
    pub const HOME: u16 = 0x24;
    pub const LEFT: u16 = 0x25;
    pub const UP: u16 = 0x26;
    pub const RIGHT: u16 = 0x27;
    pub const DOWN: u16 = 0x28;
    pub const SNAPSHOT: u16 = 0x2C;
    pub const INSERT: u16 = 0x2D;
    pub const DELETE: u16 = 0x2E;
    pub const LWIN: u16 = 0x5B;
    pub const APPS: u16 = 0x5D;
    pub const F1: u16 = 0x70;
}

/// Map a portable key to a virtual-key code. `Key::Char` needs the active keyboard layout and is
/// resolved by the caller (`VkKeyScanW`); this returns `None` for it.
pub fn named_vk(key: &Key) -> Result<Option<Vk>, String> {
    use codes::*;
    Ok(Some(match key {
        Key::Modifier(m) => match m {
            Modifier::Ctrl | Modifier::Primary => vk(LCONTROL),
            Modifier::Alt => vk(LMENU),
            Modifier::Shift => vk(LSHIFT),
            Modifier::Super => ext(LWIN),
            Modifier::Command | Modifier::Option => return Err(format!("{key} is a macOS modifier; use ctrl, alt, shift, super or primary on Windows")),
        },
        Key::Named(n) => match *n {
            "Enter" => vk(RETURN),
            "Tab" => vk(TAB),
            "Escape" => vk(ESCAPE),
            "Backspace" => vk(BACK),
            "Space" => vk(SPACE),
            "CapsLock" => vk(CAPITAL),
            // The navigation cluster is "extended": without the flag these arrive as the
            // numeric-keypad keys (which type digits when Num Lock is on).
            "Delete" => ext(DELETE),
            "Insert" => ext(INSERT),
            "Home" => ext(HOME),
            "End" => ext(END),
            "PageUp" => ext(PRIOR),
            "PageDown" => ext(NEXT),
            "Up" => ext(UP),
            "Down" => ext(DOWN),
            "Left" => ext(LEFT),
            "Right" => ext(RIGHT),
            "PrintScreen" => ext(SNAPSHOT),
            "Menu" => ext(APPS),
            f if f.starts_with('F') => match f[1..].parse::<u16>() {
                Ok(n @ 1..=24) => vk(F1 + n - 1),
                _ => return Err(format!("unknown key {f}")),
            },
            other => return Err(format!("unknown key {other}")),
        },
        Key::Char(_) => return Ok(None),
    }))
}

/// One `KEYEVENTF_UNICODE` key event (a UTF-16 code unit), or a real key for newlines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextKey {
    /// Send this UTF-16 unit with `KEYEVENTF_UNICODE` (down when `down`, else up).
    Unit { unit: u16, down: bool },
    /// Press or release a real virtual key (Enter for a line break).
    Vk { code: u16, down: bool },
}

/// Key events for one character, in the order they must be sent in a single `SendInput` call.
///
/// A character outside the BMP is a surrogate pair: both units go down (high, then low) before
/// either goes up, so the target receives two consecutive `WM_CHAR`s it can join. Line breaks are
/// the Enter key (apps treat a `WM_CHAR` of U+000A inconsistently); a `\r` directly before `\n`
/// is dropped by the caller.
pub fn char_events(c: char) -> Vec<TextKey> {
    if c == '\n' {
        return vec![TextKey::Vk { code: codes::RETURN, down: true }, TextKey::Vk { code: codes::RETURN, down: false }];
    }
    let mut buf = [0u16; 2];
    let units = c.encode_utf16(&mut buf);
    let mut out: Vec<TextKey> = units.iter().map(|&u| TextKey::Unit { unit: u, down: true }).collect();
    out.extend(units.iter().map(|&u| TextKey::Unit { unit: u, down: false }));
    out
}

/// Characters of `text` to deliver, with CRLF folded to one line break.
pub fn text_chars(text: &str) -> Vec<char> {
    let v: Vec<char> = text.chars().collect();
    v.iter().enumerate().filter(|(i, c)| !(**c == '\r' && v.get(i + 1) == Some(&'\n'))).map(|(_, c)| if *c == '\r' { '\n' } else { *c }).collect()
}

/// The virtual screen (`SM_XVIRTUALSCREEN`.. `SM_CYVIRTUALSCREEN`), physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualScreen {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl VirtualScreen {
    /// Normalized `SendInput` coordinate (with `MOUSEEVENTF_VIRTUALDESK`) for pixel `p` on an
    /// axis starting at `origin` with `len` pixels.
    ///
    /// Windows maps a normalized value `n` back to pixel `origin + floor(n * len / 65536)`.
    /// `ceil(offset * 65536 / len)` is the smallest `n` that lands on `offset`; it stays inside the
    /// pixel for both a floor and a round-to-nearest mapping as long as `len < 32768`, which
    /// holds for any real virtual screen. Clamped to 0..=65535.
    pub fn normalize_axis(p: i32, origin: i32, len: i32) -> i32 {
        if len <= 0 {
            return 0;
        }
        let off = (p - origin).clamp(0, len - 1) as i64;
        let n = (off * 65536 + len as i64 - 1) / len as i64;
        n.clamp(0, 65535) as i32
    }

    /// The pixel Windows lands on for a normalized value (inverse of `normalize_axis`).
    pub fn denormalize_axis(n: i32, origin: i32, len: i32) -> i32 {
        origin + ((n as i64 * len as i64) >> 16) as i32
    }

    pub fn normalize(&self, x: i32, y: i32) -> (i32, i32) {
        (Self::normalize_axis(x, self.x, self.width), Self::normalize_axis(y, self.y, self.height))
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

/// Wheel delta for one portable scroll step: positive steps scroll down (vertical) or right
/// (horizontal). `WHEEL_DELTA` is 120; Windows' vertical wheel is positive *up*.
pub fn wheel_delta(vertical: bool, steps: i32) -> i32 {
    if vertical { -steps * 120 } else { steps * 120 }
}

/// `HWND` as the window id the broker sees.
pub fn window_id(hwnd: isize) -> String {
    format!("0x{:x}", hwnd as usize)
}

pub fn parse_window_id(id: &str) -> Option<isize> {
    let h = id.strip_prefix("0x").or_else(|| id.strip_prefix("0X"))?;
    usize::from_str_radix(h, 16).ok().filter(|v| *v != 0).map(|v| v as isize)
}

/// UI Automation control type id -> role name (UIA_*ControlTypeId, 50000..=50040).
pub fn control_type_name(id: i32) -> &'static str {
    const NAMES: [&str; 41] = [
        "button", "calendar", "check box", "combo box", "edit", "hyperlink", "image", "list item", "list", "menu",
        "menu bar", "menu item", "progress bar", "radio button", "scroll bar", "slider", "spinner", "status bar", "tab", "tab item",
        "text", "tool bar", "tool tip", "tree", "tree item", "custom", "group", "thumb", "data grid", "data item",
        "document", "split button", "window", "pane", "header", "header item", "table", "title bar", "separator", "semantic zoom",
        "app bar",
    ];
    usize::try_from(id - 50000).ok().and_then(|i| NAMES.get(i)).copied().unwrap_or("unknown")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_lands_on_every_pixel() {
        // Floor mapping (what Windows documents) and round-to-nearest both land exactly.
        for &(origin, len) in &[(0, 1024), (0, 1920), (-1280, 3200), (-2560, 6400), (0, 7680), (-100, 333)] {
            for p in (origin..origin + len).step_by(7).chain([origin, origin + len - 1]) {
                let n = VirtualScreen::normalize_axis(p, origin, len);
                assert!((0..=65535).contains(&n));
                assert_eq!(VirtualScreen::denormalize_axis(n, origin, len), p, "floor: origin {origin} len {len} p {p}");
                let round = origin + ((n as f64 * len as f64 / 65536.0).round() as i32);
                assert_eq!(round, p, "round: origin {origin} len {len} p {p}");
            }
        }
        assert_eq!(VirtualScreen::normalize_axis(-5000, -1280, 3200), 0);
        assert_eq!(VirtualScreen::normalize_axis(99999, -1280, 3200), VirtualScreen::normalize_axis(1919, -1280, 3200));
    }

    #[test]
    fn surrogate_pairs_go_down_together() {
        let ev = char_events('😀');
        assert_eq!(ev, vec![
            TextKey::Unit { unit: 0xD83D, down: true },
            TextKey::Unit { unit: 0xDE00, down: true },
            TextKey::Unit { unit: 0xD83D, down: false },
            TextKey::Unit { unit: 0xDE00, down: false },
        ]);
        assert_eq!(char_events('€'), vec![TextKey::Unit { unit: 0x20AC, down: true }, TextKey::Unit { unit: 0x20AC, down: false }]);
        assert_eq!(char_events('\n')[0], TextKey::Vk { code: codes::RETURN, down: true });
        assert_eq!(text_chars("a\r\nb\rc"), vec!['a', '\n', 'b', '\n', 'c']);
    }

    #[test]
    fn keys_map_to_virtual_keys() {
        let k = |s: &str| named_vk(&s.parse().unwrap());
        assert_eq!(k("Up").unwrap(), Some(Vk { code: codes::UP, extended: true }));
        assert_eq!(k("Enter").unwrap(), Some(Vk { code: codes::RETURN, extended: false }));
        assert_eq!(k("F12").unwrap(), Some(Vk { code: 0x7B, extended: false }));
        assert_eq!(k("F24").unwrap(), Some(Vk { code: 0x87, extended: false }));
        assert_eq!(k("ctrl").unwrap(), Some(Vk { code: codes::LCONTROL, extended: false }));
        assert_eq!(k("primary").unwrap(), Some(Vk { code: codes::LCONTROL, extended: false }));
        assert_eq!(k("super").unwrap(), Some(Vk { code: codes::LWIN, extended: true }));
        assert!(k("cmd").is_err());
        assert_eq!(k("s").unwrap(), None);
        // Every portable named key maps.
        for n in adl_schema::keys::NAMED_KEYS {
            assert!(k(n).unwrap().is_some(), "{n}");
        }
    }

    #[test]
    fn wheel_and_ids() {
        assert_eq!(wheel_delta(true, 1), -120);
        assert_eq!(wheel_delta(false, -2), -240);
        assert_eq!(window_id(0x1a2b), "0x1a2b");
        assert_eq!(parse_window_id("0x1a2b"), Some(0x1a2b));
        assert_eq!(parse_window_id("w-editor"), None);
        assert_eq!(parse_window_id("0x0"), None);
        assert_eq!(control_type_name(50004), "edit");
        assert_eq!(control_type_name(50033), "pane");
        assert_eq!(control_type_name(1), "unknown");
    }
}
