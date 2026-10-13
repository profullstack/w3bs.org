//! The emergency-stop hot key, registered with Carbon's `RegisterEventHotKey`.
//!
//! A registered hot key is delivered only when that exact chord is pressed; nothing else typed
//! is seen, so this is not a keylogger and needs no Input Monitoring permission. Delivery needs
//! the application event loop, so it is registered by the menu-bar control (NSApplication).

use crate::ffi::*;
use std::sync::OnceLock;

/// Control+Option+Command+Escape. Command+Option+Escape alone is Force Quit; adding Control
/// keeps it distinct and hard to hit by accident.
pub const EMERGENCY_STOP: &str = "Control+Option+Command+Escape";
const KEY_ESCAPE: u32 = 0x35;
const SIGNATURE: u32 = u32::from_be_bytes(*b"W3BS");

static CALLBACK: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

unsafe extern "C" fn on_hot_key(_call: EventHandlerCallRef, _event: EventRef, _data: *mut std::ffi::c_void) -> OSStatus {
    if let Some(cb) = CALLBACK.get() {
        cb();
    }
    0
}

/// Register the chord. Call on the main thread before running the application event loop.
/// Returns a description of the chord.
pub fn register_emergency_stop(cb: Box<dyn Fn() + Send + Sync>) -> Result<&'static str, String> {
    if CALLBACK.set(cb).is_err() {
        return Err("the emergency-stop hot key is already registered".into());
    }
    let spec = EventTypeSpec { event_class: kEventClassKeyboard, event_kind: kEventHotKeyPressed };
    let mut handler: EventHandlerRef = std::ptr::null_mut();
    let mut hk: EventHotKeyRef = std::ptr::null_mut();
    // SAFETY: valid target, handler with the documented signature, one type spec; the handler
    // and hot key live for the rest of the process.
    unsafe {
        let target = GetApplicationEventTarget();
        let st = InstallEventHandler(target, on_hot_key, 1, &spec, std::ptr::null_mut(), &mut handler);
        if st != 0 {
            return Err(format!("InstallEventHandler failed ({st})"));
        }
        let st = RegisterEventHotKey(KEY_ESCAPE, controlKey | optionKey | cmdKey, EventHotKeyID { signature: SIGNATURE, id: 1 }, target, 0, &mut hk);
        if st != 0 {
            return Err(format!("RegisterEventHotKey failed ({st}); another application may own {EMERGENCY_STOP}"));
        }
    }
    Ok(EMERGENCY_STOP)
}
