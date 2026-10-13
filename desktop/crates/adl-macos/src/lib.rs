//! Agentic Desktop Layer: the macOS adapter.
//!
//! Core Graphics events for input, `CGWindowList` for windows and occlusion, Accessibility for
//! focus and inspection, `CGDisplayCreateImage` (or `screencapture`) for captures, and polled
//! session state for lock and takeover. Every platform call is gated on `target_os = "macos"`;
//! elsewhere only the portable key map compiles, so the workspace builds on every OS.

pub mod keymap;

#[cfg(target_os = "macos")]
mod cf;
#[cfg(target_os = "macos")]
mod ffi;

#[cfg(target_os = "macos")]
pub mod adapter;
#[cfg(target_os = "macos")]
pub mod hotkey;
#[cfg(target_os = "macos")]
pub mod permissions;

#[cfg(target_os = "macos")]
pub use adapter::MacAdapter;
