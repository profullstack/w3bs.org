//! Per-OS adapter selection and native control (tray / panel / menu bar).

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{make_adapter, run_tray};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{make_adapter, permission_checks, run_tray};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{make_adapter, run_tray};
