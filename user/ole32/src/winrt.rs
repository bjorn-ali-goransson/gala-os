//! Windows Runtime strings (HSTRING, winstring.h) and activation
//! (roapi.h). `api-ms-win-core-winrt-*` resolve to this DLL.
//!
//! An HSTRING is gala's own layout (opaque to callers): a header with
//! the length, a pointer to NUL-terminated UTF-16 and a reference count.
//! WindowsCreateString allocates the header and a copy of the text in one
//! block; WindowsCreateStringReference ("fast-pass") puts the header in
//! the caller's HSTRING_HEADER (24 bytes on x64, the size of ours) and
//! points at the caller's text, which must outlive it. A NULL HSTRING is
//! the empty string. Behaviour is as documented on learn.microsoft.com
//! (WindowsCreateString, WindowsCreateStringReference, WindowsDeleteString,
//! WindowsDuplicateString, WindowsGetStringRawBuffer, ...).
//!
//! No Windows Runtime classes exist on gala yet: activation reports the
//! class to the ledger and is REGDB_E_CLASSNOTREG, as for a class that is
//! not registered on Windows.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use super::{
    E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, GalaRecordMissing, Guid, HResult, S_OK, alloc, free,
    put,
};

const REGDB_E_CLASSNOTREG: HResult = 0x8004_0154u32 as i32;
const MISSING_CLASS: u32 = 2;

/// The header of an HSTRING (also an HSTRING_HEADER's content).
#[repr(C)]
pub struct HString {
    /// 1: a reference string (the caller owns header and text).
    flags: u32,
    len: u32,
    text: *const u16,
    refs: AtomicU32,
}

const REFERENCE: u32 = 1;
const _: () = assert!(core::mem::size_of::<HString>() == 24);

fn header<'a>(s: *mut HString) -> &'a HString {
    // SAFETY: a live HSTRING (callers check NULL).
    unsafe { &*s }
}

/// A new allocated HSTRING holding `len` units at `text`.
fn create(text: *const u16, len: u32, out: *mut *mut HString) -> HResult {
    let size = core::mem::size_of::<HString>() + (len as usize + 1) * 2;
    let p = alloc(size).cast::<HString>();
    if p.is_null() {
        put(out, core::ptr::null_mut());
        return E_OUTOFMEMORY;
    }
    // SAFETY: a fresh block holding the header then len + 1 units; the
    // caller's `len` units at `text`.
    unsafe {
        let copy = p.add(1).cast::<u16>();
        core::ptr::copy_nonoverlapping(text, copy, len as usize);
        copy.add(len as usize).write(0);
        p.write(HString { flags: 0, len, text: copy, refs: AtomicU32::new(1) });
    }
    put(out, p);
    S_OK
}

/// WindowsCreateString: a copy of `len` units (NULL for 0); E_POINTER for
/// a NULL source with a length, E_INVALIDARG without an output.
#[unsafe(no_mangle)]
pub extern "system" fn WindowsCreateString(
    text: *const u16,
    len: u32,
    out: *mut *mut HString,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    if len == 0 {
        return S_OK;
    }
    if text.is_null() {
        return E_POINTER;
    }
    create(text, len, out)
}

/// WindowsCreateStringReference: a string over the caller's text, which
/// must be NUL-terminated at `len` (E_INVALIDARG otherwise), its header
/// in the caller's HSTRING_HEADER.
#[unsafe(no_mangle)]
pub extern "system" fn WindowsCreateStringReference(
    text: *const u16,
    len: u32,
    header: *mut HString,
    out: *mut *mut HString,
) -> HResult {
    if out.is_null() || header.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    if len == 0 {
        return S_OK;
    }
    if text.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's text holds `len` units and its terminator.
    if unsafe { *text.add(len as usize) } != 0 {
        return E_INVALIDARG;
    }
    put(header, HString { flags: REFERENCE, len, text, refs: AtomicU32::new(1) });
    put(out, header);
    S_OK
}

/// WindowsDeleteString: one reference fewer (a reference string's is
/// the caller's: nothing to do).
#[unsafe(no_mangle)]
pub extern "system" fn WindowsDeleteString(s: *mut HString) -> HResult {
    if s.is_null() || header(s).flags & REFERENCE != 0 {
        return S_OK;
    }
    if header(s).refs.fetch_sub(1, Ordering::AcqRel) == 1 {
        free(s.cast());
    }
    S_OK
}

/// WindowsDuplicateString: the same string with a reference more, or an
/// allocated copy of a reference string.
#[unsafe(no_mangle)]
pub extern "system" fn WindowsDuplicateString(s: *mut HString, out: *mut *mut HString) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    if s.is_null() {
        return S_OK;
    }
    let h = header(s);
    if h.flags & REFERENCE != 0 {
        return create(h.text, h.len, out);
    }
    h.refs.fetch_add(1, Ordering::Relaxed);
    put(out, s);
    S_OK
}

static EMPTY: [u16; 1] = [0];

/// WindowsGetStringRawBuffer: the text (NUL-terminated) and its length;
/// "" for NULL.
#[unsafe(no_mangle)]
pub extern "system" fn WindowsGetStringRawBuffer(s: *mut HString, len: *mut u32) -> *const u16 {
    let (text, n) = if s.is_null() { (EMPTY.as_ptr(), 0) } else { (header(s).text, header(s).len) };
    if !len.is_null() {
        put(len, n);
    }
    text
}

#[unsafe(no_mangle)]
pub extern "system" fn WindowsGetStringLen(s: *mut HString) -> u32 {
    if s.is_null() { 0 } else { header(s).len }
}

#[unsafe(no_mangle)]
pub extern "system" fn WindowsIsStringEmpty(s: *mut HString) -> i32 {
    i32::from(WindowsGetStringLen(s) == 0)
}

fn units<'a>(s: *mut HString) -> &'a [u16] {
    let mut n = 0;
    let p = WindowsGetStringRawBuffer(s, &mut n);
    // SAFETY: the string's `n` units.
    unsafe { core::slice::from_raw_parts(p, n as usize) }
}

/// WindowsCompareStringOrdinal: -1, 0 or 1 by code unit.
#[unsafe(no_mangle)]
pub extern "system" fn WindowsCompareStringOrdinal(
    a: *mut HString,
    b: *mut HString,
    result: *mut i32,
) -> HResult {
    if result.is_null() {
        return E_INVALIDARG;
    }
    put(result, units(a).cmp(units(b)) as i32);
    S_OK
}

/// Reports `function(class)` to the ledger (ASCII, truncated).
fn record_class(function: &[u8], class: *mut HString) {
    let mut line = [0u8; 160];
    let mut n = 0;
    for &b in function.iter().chain(b"(").chain(units(class).iter().map(|&u| {
        // A static of the right type: non-ASCII becomes '?'.
        if u < 0x80 { &ASCII[u as usize] } else { &ASCII[b'?' as usize] }
    })) {
        if n == line.len() - 1 {
            break;
        }
        line[n] = b;
        n += 1;
    }
    line[n] = b')';
    // SAFETY: a valid buffer of n + 1 bytes.
    unsafe { GalaRecordMissing(line.as_ptr(), n + 1, MISSING_CLASS, 0) };
}

static ASCII: [u8; 128] = {
    let mut t = [0u8; 128];
    let mut i = 0;
    while i < 128 {
        t[i] = i as u8;
        i += 1;
    }
    t
};

/// RoGetActivationFactory: no Windows Runtime class is registered.
#[unsafe(no_mangle)]
pub extern "system" fn RoGetActivationFactory(
    class: *mut HString,
    _iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    record_class(b"ole32.dll!RoGetActivationFactory", class);
    REGDB_E_CLASSNOTREG
}

/// RoActivateInstance: no Windows Runtime class is registered.
#[unsafe(no_mangle)]
pub extern "system" fn RoActivateInstance(class: *mut HString, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    record_class(b"ole32.dll!RoActivateInstance", class);
    REGDB_E_CLASSNOTREG
}
