//! Portable key and modifier names (PRD section 6).
//!
//! `Key` is a *logical* key. Adapters map it to the platform's physical code for the active
//! layout; text insertion is a separate action and never goes through key names.
//!
//! Modifiers are never silently translated: `ctrl` is Control on every platform. Agents that
//! want "the platform's shortcut modifier" say `primary`, which is Command on macOS and Control
//! elsewhere.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modifier {
    Ctrl,
    Alt,
    Shift,
    /// Windows key / Super / Meta on Linux; Command on macOS.
    Super,
    /// Control everywhere except macOS, where it is Command.
    Primary,
    /// macOS Command, unsupported elsewhere.
    Command,
    /// macOS Option, unsupported elsewhere.
    Option,
}

impl Modifier {
    fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => Self::Ctrl,
            "alt" => Self::Alt,
            "shift" => Self::Shift,
            "super" | "win" | "meta" | "logo" => Self::Super,
            "primary" | "mod" => Self::Primary,
            "cmd" | "command" => Self::Command,
            "option" | "opt" => Self::Option,
            _ => return None,
        })
    }
    pub fn key(self) -> Key {
        Key::Modifier(self)
    }
}

/// Named non-character keys every adapter must know how to map.
pub const NAMED_KEYS: &[&str] = &[
    "Enter", "Tab", "Escape", "Backspace", "Delete", "Insert", "Space", "Home", "End", "PageUp",
    "PageDown", "Up", "Down", "Left", "Right", "CapsLock", "PrintScreen", "Menu", "F1", "F2", "F3",
    "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14", "F15", "F16", "F17",
    "F18", "F19", "F20", "F21", "F22", "F23", "F24",
];

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Modifier(Modifier),
    /// One of `NAMED_KEYS`, canonical spelling.
    Named(&'static str),
    /// A single character key on the active layout, e.g. `s` in Ctrl+S. Lowercase for letters.
    Char(char),
}

impl Key {
    pub fn is_modifier(&self) -> bool {
        matches!(self, Key::Modifier(_))
    }
}

impl FromStr for Key {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        if let Some(m) = Modifier::parse(s) {
            return Ok(Key::Modifier(m));
        }
        let aliases = [("Return", "Enter"), ("Esc", "Escape"), ("PgUp", "PageUp"), ("PgDn", "PageDown"),
            ("ArrowUp", "Up"), ("ArrowDown", "Down"), ("ArrowLeft", "Left"), ("ArrowRight", "Right"), ("Del", "Delete")];
        let wanted = aliases.iter().find(|(a, _)| a.eq_ignore_ascii_case(s)).map(|(_, c)| *c).unwrap_or(s);
        if let Some(n) = NAMED_KEYS.iter().find(|n| n.eq_ignore_ascii_case(wanted)) {
            return Ok(Key::Named(n));
        }
        let mut chars = s.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if !c.is_control() => Ok(Key::Char(c.to_lowercase().next().unwrap_or(c))),
            _ => Err(format!("unknown key {s:?}")),
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Modifier(m) => write!(f, "{}", serde_json::to_value(m).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()),
            Key::Named(n) => f.write_str(n),
            Key::Char(c) => write!(f, "{c}"),
        }
    }
}

impl Serialize for Key {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// A shortcut such as `Ctrl+Shift+S`: modifiers held, one final key pressed, all released in reverse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyCombo {
    pub modifiers: Vec<Modifier>,
    pub key: Key,
}

impl FromStr for KeyCombo {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        // A literal "+" key is written as "Ctrl++" or "+".
        let parts: Vec<&str> = if let Some(head) = s.strip_suffix("++") {
            head.split('+').chain(std::iter::once("+")).collect()
        } else {
            s.split('+').collect()
        };
        if parts.is_empty() || parts.len() > crate::limits::MAX_SHORTCUT_KEYS || parts.iter().any(|p| p.is_empty()) {
            return Err(format!("invalid shortcut {s:?}"));
        }
        let (last, mods) = parts.split_last().unwrap();
        let mut modifiers = Vec::new();
        for m in mods {
            let m = Modifier::parse(m).ok_or_else(|| format!("{m:?} is not a modifier in {s:?}"))?;
            if !modifiers.contains(&m) {
                modifiers.push(m);
            }
        }
        Ok(KeyCombo { modifiers, key: last.parse()? })
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for m in &self.modifiers {
            write!(f, "{}+", Key::Modifier(*m))?;
        }
        write!(f, "{}", self.key)
    }
}

impl Serialize for KeyCombo {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for KeyCombo {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shortcuts() {
        let c: KeyCombo = "Ctrl+Shift+S".parse().unwrap();
        assert_eq!(c.modifiers, vec![Modifier::Ctrl, Modifier::Shift]);
        assert_eq!(c.key, Key::Char('s'));
        assert_eq!(c.to_string(), "ctrl+shift+s");
        let plus: KeyCombo = "Ctrl++".parse().unwrap();
        assert_eq!(plus.key, Key::Char('+'));
        assert_eq!("alt+tab".parse::<KeyCombo>().unwrap().key, Key::Named("Tab"));
        assert!("Ctrl+".parse::<KeyCombo>().is_err());
        assert!("Foo+S".parse::<KeyCombo>().is_err());
        assert_eq!("return".parse::<Key>().unwrap(), Key::Named("Enter"));
    }
}
