//! Portable keys to Linux evdev codes and X keysyms.
//!
//! Evdev codes are *physical* positions (named for the US layout). Use them for shortcuts and
//! named keys. Text goes through keysyms (portal `NotifyKeyboardKeysym`) or a generated keymap
//! (virtual keyboard), never through US key positions, so other layouts type correctly.

use adl_schema::{Button, Key, Modifier};

/// Linux `BTN_*` code for a pointer button.
pub fn button_code(b: Button) -> u32 {
    match b {
        Button::Left => 0x110,
        Button::Right => 0x111,
        Button::Middle => 0x112,
        Button::Back => 0x113,
        Button::Forward => 0x114,
    }
}

/// Evdev code of the modifier's left-hand key. `Primary` is Control on Linux; macOS-only
/// modifiers have no code.
pub fn modifier_code(m: Modifier) -> Option<u32> {
    Some(match m {
        Modifier::Ctrl | Modifier::Primary => 29,
        Modifier::Shift => 42,
        Modifier::Alt => 56,
        Modifier::Super => 125,
        Modifier::Command | Modifier::Option => return None,
    })
}

fn named_code(n: &str) -> Option<u32> {
    Some(match n {
        "Escape" => 1,
        "Backspace" => 14,
        "Tab" => 15,
        "Enter" => 28,
        "Space" => 57,
        "CapsLock" => 58,
        "PrintScreen" => 99,
        "Home" => 102,
        "Up" => 103,
        "PageUp" => 104,
        "Left" => 105,
        "Right" => 106,
        "End" => 107,
        "Down" => 108,
        "PageDown" => 109,
        "Insert" => 110,
        "Delete" => 111,
        "Menu" => 127,
        f if f.starts_with('F') => {
            let n: u32 = f[1..].parse().ok()?;
            match n {
                1..=10 => 58 + n,
                11 => 87,
                12 => 88,
                13..=24 => 170 + n,
                _ => return None,
            }
        }
        _ => return None,
    })
}

/// US-layout position of a character key: (evdev code, needs Shift).
pub fn char_code(c: char) -> Option<(u32, bool)> {
    const ROW_Q: &str = "qwertyuiop";
    const ROW_A: &str = "asdfghjkl";
    const ROW_Z: &str = "zxcvbnm";
    let lower = c.to_ascii_lowercase();
    let shifted = c.is_ascii_uppercase();
    if let Some(i) = ROW_Q.find(lower) {
        return Some((16 + i as u32, shifted));
    }
    if let Some(i) = ROW_A.find(lower) {
        return Some((30 + i as u32, shifted));
    }
    if let Some(i) = ROW_Z.find(lower) {
        return Some((44 + i as u32, shifted));
    }
    let digits = "1234567890";
    if let Some(i) = digits.find(c) {
        return Some((2 + i as u32, false));
    }
    if let Some(i) = "!@#$%^&*()".find(c) {
        return Some((2 + i as u32, true));
    }
    let punct: &[(char, char, u32)] = &[
        ('-', '_', 12), ('=', '+', 13), ('[', '{', 26), (']', '}', 27), (';', ':', 39), ('\'', '"', 40),
        ('`', '~', 41), ('\\', '|', 43), (',', '<', 51), ('.', '>', 52), ('/', '?', 53), (' ', ' ', 57),
    ];
    for (base, shift, code) in punct {
        if c == *base {
            return Some((*code, false));
        }
        if c == *shift {
            return Some((*code, true));
        }
    }
    match c {
        '\n' => Some((28, false)),
        '\t' => Some((15, false)),
        _ => None,
    }
}

/// Evdev code for a logical key. Character keys use their US position, which is what
/// shortcut conventions such as Ctrl+S refer to.
pub fn key_code(k: &Key) -> Option<u32> {
    match k {
        Key::Modifier(m) => modifier_code(*m),
        Key::Named(n) => named_code(n),
        Key::Char(c) => char_code(*c).map(|(code, _)| code),
    }
}

fn named_keysym(n: &str) -> Option<u32> {
    Some(match n {
        "Enter" => 0xff0d,
        "Tab" => 0xff09,
        "Escape" => 0xff1b,
        "Backspace" => 0xff08,
        "Delete" => 0xffff,
        "Insert" => 0xff63,
        "Space" => 0x20,
        "Home" => 0xff50,
        "Left" => 0xff51,
        "Up" => 0xff52,
        "Right" => 0xff53,
        "Down" => 0xff54,
        "PageUp" => 0xff55,
        "PageDown" => 0xff56,
        "End" => 0xff57,
        "CapsLock" => 0xffe5,
        "PrintScreen" => 0xff61,
        "Menu" => 0xff67,
        f if f.starts_with('F') => {
            let n: u32 = f[1..].parse().ok()?;
            if !(1..=24).contains(&n) {
                return None;
            }
            0xffbe + n - 1
        }
        _ => return None,
    })
}

/// X keysym for a character: Latin-1 maps directly, everything else uses the Unicode range.
pub fn char_keysym(c: char) -> u32 {
    match c {
        '\n' => 0xff0d,
        '\t' => 0xff09,
        c if (' '..='~').contains(&c) || ('\u{a0}'..='\u{ff}').contains(&c) => c as u32,
        c => 0x0100_0000 | c as u32,
    }
}

pub fn keysym(k: &Key) -> Option<u32> {
    match k {
        Key::Modifier(Modifier::Ctrl | Modifier::Primary) => Some(0xffe3),
        Key::Modifier(Modifier::Shift) => Some(0xffe1),
        Key::Modifier(Modifier::Alt) => Some(0xffe9),
        Key::Modifier(Modifier::Super) => Some(0xffeb),
        Key::Modifier(_) => None,
        Key::Named(n) => named_keysym(n),
        Key::Char(c) => Some(char_keysym(*c)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes() {
        assert_eq!(key_code(&"Enter".parse().unwrap()), Some(28));
        assert_eq!(key_code(&"s".parse().unwrap()), Some(31));
        assert_eq!(key_code(&"F11".parse().unwrap()), Some(87));
        assert_eq!(key_code(&"F13".parse().unwrap()), Some(183));
        assert_eq!(key_code(&"F5".parse().unwrap()), Some(63));
        assert_eq!(key_code(&"primary".parse().unwrap()), Some(29));
        assert_eq!(key_code(&"cmd".parse().unwrap()), None);
        assert_eq!(char_code('Q'), Some((16, true)));
        assert_eq!(char_code('?'), Some((53, true)));
        assert_eq!(char_code('é'), None);
        assert_eq!(button_code(Button::Right), 0x111);
    }

    #[test]
    fn keysyms() {
        assert_eq!(char_keysym('a'), 0x61);
        assert_eq!(char_keysym('é'), 0xe9);
        assert_eq!(char_keysym('日'), 0x0100_65e5);
        assert_eq!(keysym(&"F1".parse().unwrap()), Some(0xffbe));
        assert_eq!(keysym(&"F12".parse().unwrap()), Some(0xffc9));
    }
}
