//! Linux adapters for the Agentic Desktop Layer.
//!
//! - [`vela`]: Vela compositor command socket (observation, focus, occlusion, lock and
//!   authentication-prompt interlocks). Clean-room from interface facts.
//! - [`portal`]: KDE Plasma / GNOME input and capture through the XDG desktop portals.
//! - [`kwin`]: KDE Plasma window list, stacking, focus, activation and the polkit interlock via
//!   KWin scripts; [`kwin::WithKWin`] pairs it with the portal backend.
//! - [`gnome`]: the same window data on GNOME Shell from the W3BS Shell extension;
//!   [`gnome::WithGnome`] pairs it with the portal backend.
//! - [`logind`]: lock, VT switch and suspend events from systemd-logind.
//! - [`keymap`]: portable keys to evdev codes and keysyms.
//! - [`xkb`]: which characters Mutter can type, from the compositor's live keymap.
//! - [`screensaver`]: lock events from the desktop's ScreenSaver service (no logind needed).

pub mod keymap;
#[cfg(feature = "dbus")]
pub mod gnome;
#[cfg(feature = "dbus")]
pub mod kwin;
#[cfg(feature = "dbus")]
pub mod logind;
#[cfg(feature = "dbus")]
pub mod portal;
pub mod vela;
pub mod xkb;
#[cfg(feature = "dbus")]
pub mod screensaver;
