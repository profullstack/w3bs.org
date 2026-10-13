//! Linux adapters for the Agentic Desktop Layer.
//!
//! - [`vela`]: Vela compositor command socket (observation, focus, occlusion, lock and
//!   authentication-prompt interlocks). Clean-room from interface facts.
//! - [`portal`]: KDE Plasma / GNOME input and capture through the XDG desktop portals.
//! - [`logind`]: lock, VT switch and suspend events from systemd-logind.
//! - [`keymap`]: portable keys to evdev codes and keysyms.

pub mod keymap;
#[cfg(feature = "dbus")]
pub mod logind;
#[cfg(feature = "dbus")]
pub mod portal;
pub mod vela;
