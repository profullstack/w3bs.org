//! Linux adapters for the Agentic Desktop Layer.
//!
//! - [`vela`]: Vela compositor command socket (observation, focus, occlusion, lock and
//!   authentication-prompt interlocks). Clean-room from interface facts.
//! - [`portal`]: KDE Plasma / GNOME input and capture through the XDG desktop portals.
//! - [`kwin`]: KDE Plasma window list, stacking, focus, activation and the polkit interlock via
//!   KWin scripts; [`kwin::WithKWin`] pairs it with the portal backend.
//! - [`logind`]: lock, VT switch and suspend events from systemd-logind.
//! - [`keymap`]: portable keys to evdev codes and keysyms.

pub mod keymap;
#[cfg(feature = "dbus")]
pub mod kwin;
#[cfg(feature = "dbus")]
pub mod logind;
#[cfg(feature = "dbus")]
pub mod portal;
pub mod vela;
