//! Minimal owned CoreFoundation values and typed readers.

use crate::ffi::*;

/// An owned (+1) CoreFoundation reference, released on drop.
pub struct Owned(CFTypeRef);

// SAFETY: the CF types used here (strings, numbers, dictionaries, arrays, AX elements) are
// immutable or internally synchronized, and ownership is unique.
unsafe impl Send for Owned {}
unsafe impl Sync for Owned {}

impl Owned {
    /// Takes ownership of a reference returned by a Create/Copy function. `None` if null.
    pub fn from_create(p: CFTypeRef) -> Option<Self> {
        (!p.is_null()).then_some(Self(p))
    }
    pub fn as_ptr(&self) -> CFTypeRef {
        self.0
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: we own one reference.
        unsafe { CFRelease(self.0) }
    }
}

pub fn string(s: &str) -> Owned {
    // SAFETY: valid UTF-8 bytes and length.
    let p = unsafe { CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as CFIndex, kCFStringEncodingUTF8, 0) };
    Owned::from_create(p).expect("CFStringCreateWithBytes failed")
}

fn type_is(p: CFTypeRef, id: CFTypeID) -> bool {
    // SAFETY: p is non-null and a CF object.
    !p.is_null() && unsafe { CFGetTypeID(p) } == id
}

pub fn to_string(p: CFTypeRef) -> Option<String> {
    // SAFETY: type-checked before use; buffer sized by CFStringGetMaximumSizeForEncoding.
    unsafe {
        if !type_is(p, CFStringGetTypeID()) {
            return None;
        }
        let len = CFStringGetLength(p);
        let cap = CFStringGetMaximumSizeForEncoding(len, kCFStringEncodingUTF8) + 1;
        let mut buf = vec![0u8; cap.max(1) as usize];
        if CFStringGetCString(p, buf.as_mut_ptr().cast(), cap, kCFStringEncodingUTF8) == 0 {
            return None;
        }
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        buf.truncate(end);
        String::from_utf8(buf).ok()
    }
}

pub fn to_i64(p: CFTypeRef) -> Option<i64> {
    // SAFETY: type-checked.
    unsafe {
        if type_is(p, CFBooleanGetTypeID()) {
            return Some(CFBooleanGetValue(p) as i64);
        }
        if !type_is(p, CFNumberGetTypeID()) {
            return None;
        }
        let mut v: i64 = 0;
        (CFNumberGetValue(p, kCFNumberSInt64Type, (&mut v as *mut i64).cast()) != 0).then_some(v)
    }
}

pub fn to_f64(p: CFTypeRef) -> Option<f64> {
    // SAFETY: type-checked.
    unsafe {
        if !type_is(p, CFNumberGetTypeID()) {
            return None;
        }
        let mut v: f64 = 0.0;
        (CFNumberGetValue(p, kCFNumberDoubleType, (&mut v as *mut f64).cast()) != 0).then_some(v)
    }
}

pub fn to_bool(p: CFTypeRef) -> Option<bool> {
    // SAFETY: type-checked.
    unsafe {
        if type_is(p, CFBooleanGetTypeID()) {
            return Some(CFBooleanGetValue(p) != 0);
        }
    }
    to_i64(p).map(|v| v != 0)
}

pub fn is_array(p: CFTypeRef) -> bool {
    // SAFETY: plain type query.
    type_is(p, unsafe { CFArrayGetTypeID() })
}

pub fn is_dictionary(p: CFTypeRef) -> bool {
    // SAFETY: plain type query.
    type_is(p, unsafe { CFDictionaryGetTypeID() })
}

/// Borrowed elements of a CFArray (valid while the array lives).
pub fn array_items(a: CFTypeRef) -> Vec<CFTypeRef> {
    if !is_array(a) {
        return vec![];
    }
    // SAFETY: a is a CFArray; indices are in range.
    unsafe { (0..CFArrayGetCount(a)).map(|i| CFArrayGetValueAtIndex(a, i)).collect() }
}

/// Borrowed value for a CFString key (valid while the dictionary lives).
pub fn dict_get(d: CFTypeRef, key: CFStringRef) -> CFTypeRef {
    if !is_dictionary(d) {
        return std::ptr::null();
    }
    // SAFETY: d is a CFDictionary and key a CFString.
    unsafe { CFDictionaryGetValue(d, key) }
}

pub fn dict_get_str(d: CFTypeRef, key: &str) -> CFTypeRef {
    let k = string(key);
    dict_get(d, k.as_ptr())
}

/// A one-entry `{key: bool}` dictionary (for option dictionaries).
pub fn bool_dict(key: CFStringRef, value: bool) -> Owned {
    // SAFETY: key and boolean are valid CF objects; the standard callbacks retain them.
    unsafe {
        let keys = [key];
        let values = [if value { kCFBooleanTrue } else { kCFBooleanFalse }];
        let d = CFDictionaryCreate(std::ptr::null(), keys.as_ptr(), values.as_ptr(), 1, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
        Owned::from_create(d).expect("CFDictionaryCreate failed")
    }
}
