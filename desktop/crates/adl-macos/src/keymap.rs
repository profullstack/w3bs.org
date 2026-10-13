//! Portable keys to macOS virtual key codes (`kVK_*`, Events.h) and event flags.
//!
//! `kVK_ANSI_*` codes are *physical* positions named for the US ANSI layout. They are used for
//! named keys and shortcuts. Text never goes through key positions: it is delivered as Unicode
//! strings on the key event (`CGEventKeyboardSetUnicodeString`), so any layout types correctly.
//! On a non-US layout a `Char` key in a shortcut presses the key at the US position.

use adl_schema::{Button, Key, Modifier};

// CGEventFlags masks (CGEventTypes.h); duplicated here so the map is testable off macOS.
pub const FLAG_SHIFT: u64 = 0x0002_0000;
pub const FLAG_CONTROL: u64 = 0x0004_0000;
pub const FLAG_OPTION: u64 = 0x0008_0000;
pub const FLAG_COMMAND: u64 = 0x0010_0000;
pub const FLAG_FN: u64 = 0x0080_0000;

/// (virtual key code, event flag) of a modifier's left-hand key. `Primary` and `Super` are
/// Command on macOS; `Ctrl` is always Control (never silently translated); `Alt` is Option.
pub fn modifier(m: Modifier) -> (u16, u64) {
    match m {
        Modifier::Ctrl => (0x3B, FLAG_CONTROL),
        Modifier::Alt | Modifier::Option => (0x3A, FLAG_OPTION),
        Modifier::Shift => (0x38, FLAG_SHIFT),
        Modifier::Super | Modifier::Primary | Modifier::Command => (0x37, FLAG_COMMAND),
    }
}

/// Virtual key code of a named key, plus whether the Fn flag must accompany it (navigation
/// keys carry it on Apple keyboards). `None` for keys a Mac keyboard does not have.
pub fn named(n: &str) -> Option<(u16, bool)> {
    Some(match n {
        "Enter" => (0x24, false),
        "Tab" => (0x30, false),
        "Space" => (0x31, false),
        "Backspace" => (0x33, false),
        "Escape" => (0x35, false),
        "CapsLock" => (0x39, false),
        "Insert" => (0x72, true), // kVK_Help sits where Insert is on extended keyboards
        "Home" => (0x73, true),
        "PageUp" => (0x74, true),
        "Delete" => (0x75, true), // forward delete
        "End" => (0x77, true),
        "PageDown" => (0x79, true),
        "Left" => (0x7B, true),
        "Right" => (0x7C, true),
        "Down" => (0x7D, true),
        "Up" => (0x7E, true),
        "Menu" => (0x6E, false), // kVK_ContextualMenu
        f if f.starts_with('F') => {
            let n: u32 = f[1..].parse().ok()?;
            let code = match n {
                1 => 0x7A,
                2 => 0x78,
                3 => 0x63,
                4 => 0x76,
                5 => 0x60,
                6 => 0x61,
                7 => 0x62,
                8 => 0x64,
                9 => 0x65,
                10 => 0x6D,
                11 => 0x67,
                12 => 0x6F,
                13 => 0x69,
                14 => 0x6B,
                15 => 0x71,
                16 => 0x6A,
                17 => 0x40,
                18 => 0x4F,
                19 => 0x50,
                20 => 0x5A,
                _ => return None,
            };
            (code, true)
        }
        _ => return None, // PrintScreen, F21-F24: no Mac equivalent
    })
}

/// US ANSI position of a character key: (virtual key code, needs Shift).
pub fn char_code(c: char) -> Option<(u16, bool)> {
    const UNSHIFTED: &[(char, u16)] = &[
        ('a', 0x00), ('s', 0x01), ('d', 0x02), ('f', 0x03), ('h', 0x04), ('g', 0x05), ('z', 0x06), ('x', 0x07),
        ('c', 0x08), ('v', 0x09), ('b', 0x0B), ('q', 0x0C), ('w', 0x0D), ('e', 0x0E), ('r', 0x0F), ('y', 0x10),
        ('t', 0x11), ('1', 0x12), ('2', 0x13), ('3', 0x14), ('4', 0x15), ('6', 0x16), ('5', 0x17), ('=', 0x18),
        ('9', 0x19), ('7', 0x1A), ('-', 0x1B), ('8', 0x1C), ('0', 0x1D), (']', 0x1E), ('o', 0x1F), ('u', 0x20),
        ('[', 0x21), ('i', 0x22), ('p', 0x23), ('l', 0x25), ('j', 0x26), ('\'', 0x27), ('k', 0x28), (';', 0x29),
        ('\\', 0x2A), (',', 0x2B), ('/', 0x2C), ('n', 0x2D), ('m', 0x2E), ('.', 0x2F), ('`', 0x32), (' ', 0x31),
    ];
    const SHIFTED: &[(char, char)] = &[
        ('!', '1'), ('@', '2'), ('#', '3'), ('$', '4'), ('%', '5'), ('^', '6'), ('&', '7'), ('*', '8'), ('(', '9'),
        (')', '0'), ('_', '-'), ('+', '='), ('{', '['), ('}', ']'), ('|', '\\'), (':', ';'), ('"', '\''), ('<', ','),
        ('>', '.'), ('?', '/'), ('~', '`'),
    ];
    let find = |c: char| UNSHIFTED.iter().find(|(k, _)| *k == c).map(|(_, v)| *v);
    if c.is_ascii_uppercase() {
        return find(c.to_ascii_lowercase()).map(|v| (v, true));
    }
    if let Some(v) = find(c) {
        return Some((v, false));
    }
    SHIFTED.iter().find(|(s, _)| *s == c).and_then(|(_, base)| find(*base)).map(|v| (v, true))
}

/// What one portable key becomes: a key code, the flags it needs by itself, and whether it is a
/// modifier (sent as a flags-changed event).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MacKey {
    pub code: u16,
    pub flags: u64,
    pub modifier_flag: Option<u64>,
}

pub fn key(k: &Key) -> Option<MacKey> {
    match k {
        Key::Modifier(m) => {
            let (code, flag) = modifier(*m);
            Some(MacKey { code, flags: 0, modifier_flag: Some(flag) })
        }
        Key::Named(n) => named(n).map(|(code, fnk)| MacKey { code, flags: if fnk { FLAG_FN } else { 0 }, modifier_flag: None }),
        Key::Char(c) => char_code(*c).map(|(code, shift)| MacKey { code, flags: if shift { FLAG_SHIFT } else { 0 }, modifier_flag: None }),
    }
}

/// Core Graphics mouse button number: 0 left, 1 right, 2 middle, 3 back, 4 forward.
pub fn button_number(b: Button) -> u32 {
    match b {
        Button::Left => 0,
        Button::Right => 1,
        Button::Middle => 2,
        Button::Back => 3,
        Button::Forward => 4,
    }
}

/// UTF-16 pieces of at most `max` code units that never split a surrogate pair. Each piece
/// becomes one synthetic key event carrying a Unicode string.
pub fn utf16_pieces(text: &str, max: usize) -> Vec<Vec<u16>> {
    let mut out: Vec<Vec<u16>> = vec![];
    let mut cur: Vec<u16> = vec![];
    for c in text.chars() {
        let mut buf = [0u16; 2];
        let units = c.encode_utf16(&mut buf);
        if cur.len() + units.len() > max && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.extend_from_slice(units);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_keys() {
        assert_eq!(key(&"a".parse().unwrap()).unwrap().code, 0x00);
        assert_eq!(key(&"s".parse().unwrap()).unwrap().code, 0x01);
        assert_eq!(key(&"Enter".parse().unwrap()).unwrap().code, 0x24);
        assert_eq!(key(&"Escape".parse().unwrap()).unwrap().code, 0x35);
        let plus = key(&Key::Char('+')).unwrap();
        assert_eq!((plus.code, plus.flags), (0x18, FLAG_SHIFT));
        assert_eq!(key(&Key::Char('A')).unwrap().flags, FLAG_SHIFT);
        assert_eq!(key(&"F12".parse().unwrap()).unwrap().code, 0x6F);
        assert!(key(&"PrintScreen".parse().unwrap()).is_none());
        assert!(key(&Key::Char('é')).is_none());
    }

    #[test]
    fn primary_is_command_and_ctrl_stays_control() {
        assert_eq!(modifier(Modifier::Primary), (0x37, FLAG_COMMAND));
        assert_eq!(modifier(Modifier::Super), (0x37, FLAG_COMMAND));
        assert_eq!(modifier(Modifier::Ctrl), (0x3B, FLAG_CONTROL));
        assert_eq!(modifier(Modifier::Alt), (0x3A, FLAG_OPTION));
        assert!(key(&Key::Modifier(Modifier::Primary)).unwrap().modifier_flag == Some(FLAG_COMMAND));
    }

    #[test]
    fn every_named_key_maps_or_is_documented_missing() {
        for n in adl_schema::keys::NAMED_KEYS {
            let missing = matches!(*n, "PrintScreen" | "F21" | "F22" | "F23" | "F24");
            assert_eq!(named(n).is_none(), missing, "{n}");
        }
    }

    #[test]
    fn pieces_keep_surrogate_pairs() {
        let p = utf16_pieces("h\u{e9}llo \u{20ac} \u{1F600}", 1);
        assert_eq!(p.len(), 9);
        assert_eq!(p.last().unwrap().len(), 2);
        let all: Vec<u16> = p.concat();
        assert_eq!(String::from_utf16(&all).unwrap(), "h\u{e9}llo \u{20ac} \u{1F600}");
        assert_eq!(utf16_pieces("abcdef", 4), vec![vec![97, 98, 99, 100], vec![101, 102]]);
    }
}
