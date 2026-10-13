//! TCC permissions the adapter depends on, and the System Settings panes that grant them.
//!
//! Nothing here prompts unless the human asked (`w3bs desktop doctor --prompt` or the menu-bar
//! control's onboarding item). Input Monitoring is not needed: the adapter never reads keys.

use crate::cf;
use crate::ffi::*;

pub const ACCESSIBILITY_PANE: &str = "System Settings > Privacy & Security > Accessibility";
pub const SCREEN_RECORDING_PANE: &str = "System Settings > Privacy & Security > Screen Recording (\"Screen & System Audio Recording\" on macOS 15 and later)";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Permissions {
    /// Accessibility (AXIsProcessTrusted): window focus and the accessibility tree.
    pub accessibility: bool,
    /// Posting synthetic events (CGPreflightPostEventAccess), granted from the Accessibility pane.
    pub post_events: bool,
    /// Screen Recording (CGPreflightScreenCaptureAccess): captures and window titles.
    pub screen_capture: bool,
}

pub fn accessibility_trusted() -> bool {
    // SAFETY: plain query.
    unsafe { AXIsProcessTrusted() != 0 }
}

/// Current state, without any prompt.
pub fn check() -> Permissions {
    // SAFETY: plain queries.
    unsafe { Permissions { accessibility: accessibility_trusted(), post_events: CGPreflightPostEventAccess(), screen_capture: CGPreflightScreenCaptureAccess() } }
}

/// Ask macOS to show its prompts for whatever is missing (each prompt appears at most once per
/// process; afterwards the human has to use System Settings). Returns the state afterwards.
pub fn request() -> Permissions {
    let before = check();
    // SAFETY: the option dictionary is a valid CFDictionary; the requests are plain calls.
    unsafe {
        if !before.accessibility {
            let opts = cf::bool_dict(kAXTrustedCheckOptionPrompt, true);
            AXIsProcessTrustedWithOptions(opts.as_ptr());
        }
        if !before.post_events {
            CGRequestPostEventAccess();
        }
        if !before.screen_capture {
            CGRequestScreenCaptureAccess();
        }
    }
    check()
}
