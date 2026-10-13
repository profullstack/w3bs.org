//! Raw declarations for the C frameworks this adapter uses: CoreFoundation, CoreGraphics
//! (displays, windows, events, session), ApplicationServices (Accessibility), ImageIO (PNG
//! encoding) and Carbon (secure input state, the emergency-stop hot key).
//!
//! Declared by hand, from the public SDK headers, so the exact signatures are visible in review
//! and no binding crate's API churn reaches the adapter. Only documented symbols are linked; the
//! two functions that may be missing on a given macOS release (`CGDisplayCreateImage`,
//! `_AXUIElementGetWindow`) are looked up at run time with `dlsym` instead.

#![allow(non_upper_case_globals, non_snake_case, dead_code)]

use std::ffi::{c_char, c_void};

pub type CFTypeRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFMutableDataRef = *mut c_void;
pub type CFArrayRef = *const c_void;
pub type CFIndex = isize;
pub type CFTypeID = usize;
pub type Boolean = u8;

pub type CGDirectDisplayID = u32;
pub type CGError = i32;
pub type CGWindowID = u32;
pub type CGEventRef = *mut c_void;
pub type CGEventSourceRef = *mut c_void;
pub type CGDisplayModeRef = *mut c_void;
pub type CGImageRef = *mut c_void;
pub type CGImageDestinationRef = *mut c_void;

pub type AXUIElementRef = *const c_void;
pub type AXError = i32;
pub type OSStatus = i32;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

/// Opaque callback tables; only their addresses are passed.
#[repr(C)]
pub struct CFDictionaryKeyCallBacks {
    _private: [usize; 6],
}
#[repr(C)]
pub struct CFDictionaryValueCallBacks {
    _private: [usize; 5],
}

pub const kCFStringEncodingUTF8: u32 = 0x0800_0100;
pub const kCFNumberSInt64Type: CFIndex = 4;
pub const kCFNumberDoubleType: CFIndex = 13;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub fn CFRelease(cf: CFTypeRef);
    pub fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    pub fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
    pub fn CFStringGetTypeID() -> CFTypeID;
    pub fn CFNumberGetTypeID() -> CFTypeID;
    pub fn CFBooleanGetTypeID() -> CFTypeID;
    pub fn CFArrayGetTypeID() -> CFTypeID;
    pub fn CFDictionaryGetTypeID() -> CFTypeID;
    pub fn CFStringCreateWithBytes(alloc: *const c_void, bytes: *const u8, len: CFIndex, encoding: u32, external: Boolean) -> CFStringRef;
    pub fn CFStringGetLength(s: CFStringRef) -> CFIndex;
    pub fn CFStringGetMaximumSizeForEncoding(len: CFIndex, encoding: u32) -> CFIndex;
    pub fn CFStringGetCString(s: CFStringRef, buf: *mut c_char, size: CFIndex, encoding: u32) -> Boolean;
    pub fn CFNumberGetValue(n: CFTypeRef, the_type: CFIndex, value: *mut c_void) -> Boolean;
    pub fn CFBooleanGetValue(b: CFTypeRef) -> Boolean;
    pub fn CFDictionaryGetValue(d: CFDictionaryRef, key: *const c_void) -> *const c_void;
    pub fn CFDictionaryCreate(alloc: *const c_void, keys: *const *const c_void, values: *const *const c_void, n: CFIndex, kcb: *const CFDictionaryKeyCallBacks, vcb: *const CFDictionaryValueCallBacks) -> CFDictionaryRef;
    pub fn CFArrayGetCount(a: CFArrayRef) -> CFIndex;
    pub fn CFArrayGetValueAtIndex(a: CFArrayRef, i: CFIndex) -> *const c_void;
    pub fn CFDataCreateMutable(alloc: *const c_void, capacity: CFIndex) -> CFMutableDataRef;
    pub fn CFDataGetLength(d: CFTypeRef) -> CFIndex;
    pub fn CFDataGetBytePtr(d: CFTypeRef) -> *const u8;
    pub static kCFBooleanTrue: CFTypeRef;
    pub static kCFBooleanFalse: CFTypeRef;
    pub static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
    pub static kCFTypeDictionaryValueCallBacks: CFDictionaryValueCallBacks;
}

// CGWindowList options and event constants (CGWindow.h, CGEventTypes.h).
pub const kCGWindowListOptionOnScreenOnly: u32 = 1 << 0;
pub const kCGWindowListExcludeDesktopElements: u32 = 1 << 4;
pub const kCGNullWindowID: CGWindowID = 0;

pub const kCGEventSourceStatePrivate: i32 = -1;
pub const kCGEventSourceStateCombinedSessionState: i32 = 0;
pub const kCGEventSourceStateHIDSystemState: i32 = 1;
pub const kCGHIDEventTap: u32 = 0;

pub const kCGEventLeftMouseDown: u32 = 1;
pub const kCGEventLeftMouseUp: u32 = 2;
pub const kCGEventRightMouseDown: u32 = 3;
pub const kCGEventRightMouseUp: u32 = 4;
pub const kCGEventMouseMoved: u32 = 5;
pub const kCGEventLeftMouseDragged: u32 = 6;
pub const kCGEventRightMouseDragged: u32 = 7;
pub const kCGEventKeyDown: u32 = 10;
pub const kCGEventKeyUp: u32 = 11;
pub const kCGEventFlagsChanged: u32 = 12;
pub const kCGEventOtherMouseDown: u32 = 25;
pub const kCGEventOtherMouseUp: u32 = 26;
pub const kCGEventOtherMouseDragged: u32 = 27;
pub const kCGAnyInputEventType: u32 = !0;

pub const kCGMouseEventClickState: u32 = 1;
pub const kCGMouseEventButtonNumber: u32 = 3;
pub const kCGEventSourceUserData: u32 = 42;
pub const kCGScrollEventUnitLine: u32 = 1;

pub const kCGEventFlagMaskShift: u64 = 0x0002_0000;
pub const kCGEventFlagMaskControl: u64 = 0x0004_0000;
pub const kCGEventFlagMaskAlternate: u64 = 0x0008_0000;
pub const kCGEventFlagMaskCommand: u64 = 0x0010_0000;
pub const kCGEventFlagMaskSecondaryFn: u64 = 0x0080_0000;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub fn CGGetActiveDisplayList(max: u32, displays: *mut CGDirectDisplayID, count: *mut u32) -> CGError;
    pub fn CGMainDisplayID() -> CGDirectDisplayID;
    pub fn CGDisplayBounds(d: CGDirectDisplayID) -> CGRect;
    pub fn CGDisplayRotation(d: CGDirectDisplayID) -> f64;
    pub fn CGDisplayIsBuiltin(d: CGDirectDisplayID) -> u32;
    pub fn CGDisplayCopyDisplayMode(d: CGDirectDisplayID) -> CGDisplayModeRef;
    pub fn CGDisplayModeGetWidth(m: CGDisplayModeRef) -> usize;
    pub fn CGDisplayModeGetPixelWidth(m: CGDisplayModeRef) -> usize;
    pub fn CGDisplayModeRelease(m: CGDisplayModeRef);

    pub fn CGWindowListCopyWindowInfo(option: u32, relative_to: CGWindowID) -> CFArrayRef;
    pub fn CGRectMakeWithDictionaryRepresentation(dict: CFDictionaryRef, rect: *mut CGRect) -> bool;
    pub static kCGWindowNumber: CFStringRef;
    pub static kCGWindowLayer: CFStringRef;
    pub static kCGWindowBounds: CFStringRef;
    pub static kCGWindowOwnerPID: CFStringRef;
    pub static kCGWindowOwnerName: CFStringRef;
    pub static kCGWindowName: CFStringRef;
    pub static kCGWindowAlpha: CFStringRef;

    pub fn CGEventSourceCreate(state: i32) -> CGEventSourceRef;
    pub fn CGEventSourceSetUserData(src: CGEventSourceRef, data: i64);
    pub fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
    pub fn CGEventCreate(src: CGEventSourceRef) -> CGEventRef;
    pub fn CGEventGetLocation(e: CGEventRef) -> CGPoint;
    pub fn CGEventCreateMouseEvent(src: CGEventSourceRef, mouse_type: u32, pos: CGPoint, button: u32) -> CGEventRef;
    pub fn CGEventCreateKeyboardEvent(src: CGEventSourceRef, key: u16, down: bool) -> CGEventRef;
    pub fn CGEventCreateScrollWheelEvent2(src: CGEventSourceRef, units: u32, wheel_count: u32, wheel1: i32, wheel2: i32, wheel3: i32) -> CGEventRef;
    pub fn CGEventKeyboardSetUnicodeString(e: CGEventRef, len: usize, s: *const u16);
    pub fn CGEventSetFlags(e: CGEventRef, flags: u64);
    pub fn CGEventSetType(e: CGEventRef, t: u32);
    pub fn CGEventSetIntegerValueField(e: CGEventRef, field: u32, value: i64);
    pub fn CGEventPost(tap: u32, e: CGEventRef);

    pub fn CGPreflightScreenCaptureAccess() -> bool;
    pub fn CGRequestScreenCaptureAccess() -> bool;
    pub fn CGPreflightPostEventAccess() -> bool;
    pub fn CGRequestPostEventAccess() -> bool;

    pub fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
    pub fn CGImageGetWidth(img: CGImageRef) -> usize;
    pub fn CGImageGetHeight(img: CGImageRef) -> usize;
    pub fn CGImageRelease(img: CGImageRef);
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    pub fn CGImageDestinationCreateWithData(data: CFMutableDataRef, uti: CFStringRef, count: usize, options: CFDictionaryRef) -> CGImageDestinationRef;
    pub fn CGImageDestinationAddImage(dest: CGImageDestinationRef, img: CGImageRef, props: CFDictionaryRef);
    pub fn CGImageDestinationFinalize(dest: CGImageDestinationRef) -> bool;
}

pub const kAXErrorSuccess: AXError = 0;
pub const kAXValueCGPointType: u32 = 1;
pub const kAXValueCGSizeType: u32 = 2;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    pub fn AXIsProcessTrusted() -> Boolean;
    pub fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> Boolean;
    pub static kAXTrustedCheckOptionPrompt: CFStringRef;
    pub fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    pub fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    pub fn AXUIElementCopyAttributeValue(el: AXUIElementRef, attr: CFStringRef, value: *mut CFTypeRef) -> AXError;
    pub fn AXUIElementCopyActionNames(el: AXUIElementRef, names: *mut CFArrayRef) -> AXError;
    pub fn AXUIElementSetMessagingTimeout(el: AXUIElementRef, seconds: f32) -> AXError;
    pub fn AXUIElementGetPid(el: AXUIElementRef, pid: *mut i32) -> AXError;
    pub fn AXUIElementGetTypeID() -> CFTypeID;
    pub fn AXValueGetTypeID() -> CFTypeID;
    pub fn AXValueGetValue(v: CFTypeRef, the_type: u32, out: *mut c_void) -> Boolean;
}

// Carbon HIToolbox: secure event input and the system-wide hot key.
pub type EventTargetRef = *mut c_void;
pub type EventHandlerRef = *mut c_void;
pub type EventHandlerCallRef = *mut c_void;
pub type EventRef = *mut c_void;
pub type EventHotKeyRef = *mut c_void;
pub type EventHandlerProc = unsafe extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OSStatus;

#[repr(C)]
pub struct EventTypeSpec {
    pub event_class: u32,
    pub event_kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct EventHotKeyID {
    pub signature: u32,
    pub id: u32,
}

/// `'keyb'`
pub const kEventClassKeyboard: u32 = u32::from_be_bytes(*b"keyb");
pub const kEventHotKeyPressed: u32 = 5;
pub const cmdKey: u32 = 1 << 8;
pub const shiftKey: u32 = 1 << 9;
pub const optionKey: u32 = 1 << 11;
pub const controlKey: u32 = 1 << 12;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    pub fn IsSecureEventInputEnabled() -> Boolean;
    pub fn GetApplicationEventTarget() -> EventTargetRef;
    pub fn InstallEventHandler(target: EventTargetRef, handler: EventHandlerProc, num_types: usize, list: *const EventTypeSpec, user_data: *mut c_void, out: *mut EventHandlerRef) -> OSStatus;
    pub fn RegisterEventHotKey(key_code: u32, modifiers: u32, id: EventHotKeyID, target: EventTargetRef, options: u32, out: *mut EventHotKeyRef) -> OSStatus;
}

unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;

/// Look up a symbol in the already-loaded images, for functions a macOS release may lack.
pub fn optional_symbol(name: &std::ffi::CStr) -> Option<*mut c_void> {
    // SAFETY: dlsym with RTLD_DEFAULT and a NUL-terminated name is always safe to call.
    let p = unsafe { dlsym(RTLD_DEFAULT, name.as_ptr()) };
    (!p.is_null()).then_some(p)
}
