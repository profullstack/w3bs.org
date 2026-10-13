//! Windows adapter for the Agentic Desktop Layer.
//!
//! * Input: `SendInput` (absolute virtual-desktop pointer motion, buttons incl. XBUTTON1/2,
//!   wheel and tilt, virtual-key codes, `KEYEVENTF_UNICODE` text).
//! * Windows: `EnumWindows` + DWM (cloaking, extended frame bounds), `WindowFromPoint` for
//!   occlusion, `GetForegroundWindow` for focus.
//! * Capture: GDI `BitBlt` of one monitor, encoded as PNG.
//! * Accessibility: UI Automation (`IUIAutomation`, control view).
//! * Session: a hidden window on its own thread receives WTS session changes, power and display
//!   broadcasts and the emergency-stop hotkey.
//!
//! **Coordinate model.** The process is per-monitor DPI aware (v2), so every Win32 API it calls
//! speaks physical pixels of the virtual screen. Desktop logical coordinates on Windows are
//! therefore *virtual-screen physical pixels*: the primary monitor's top-left is (0, 0) and
//! monitors left of or above it have negative origins. Each display reports `scale = 1.0`
//! (captures are taken at exactly the display's pixel size, so capture pixels map 1:1), and the
//! monitor's DPI scale (e.g. 150 %) is stated in the display name. Windows has no single
//! DIP plane across monitors with different scale factors, so DIPs cannot serve as one
//! consistent desktop space; physical pixels can.
//!
//! The pure mapping code (`map`) is portable so its tests run on every CI host; everything that
//! touches Win32 is compiled only on Windows.

pub mod map;

#[cfg(windows)]
mod adapter;
#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod input;
#[cfg(windows)]
mod session;
#[cfg(windows)]
mod uia;
#[cfg(windows)]
pub mod win;

#[cfg(windows)]
pub use adapter::WindowsAdapter;
#[cfg(windows)]
pub use session::{hotkey_description, register_hotkey, HOTKEYS, HOTKEY_ID};
