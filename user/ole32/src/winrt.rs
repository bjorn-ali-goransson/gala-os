//! Windows Runtime strings (HSTRING, winstring.h) and activation
//! (roapi.h). `api-ms-win-core-winrt-*` resolve to this DLL, and
//! combase.dll forwards its exports here (ARCHITECTURE "COM: ole32 and
//! oleaut32").
//!
//! An HSTRING points at a header laid out as Windows lays it out (Wine's
//! string tests read it on Windows: test_hstring_struct): flags, length,
//! two unused words, then the text pointer, 24 bytes on x64 and 20 on
//! x86, the size of HSTRING_HEADER. WindowsCreateString's strings follow
//! the header with a reference count and the NUL-terminated text, in one
//! block; WindowsCreateStringReference ("fast-pass", flags 1) puts the
//! header in the caller's HSTRING_HEADER and points at the caller's text,
//! which must outlive it. A NULL HSTRING is the empty string. Behaviour
//! is as documented on learn.microsoft.com and as Wine's conformance
//! tests observe on Windows (dlls/combase/tests/string.c).
//!
//! Activation (RoGetActivationFactory, RoActivateInstance) reads the
//! class's registration as Windows does,
//! `HKLM\SOFTWARE\Microsoft\WindowsRuntime\ActivatableClassId\<class>`:
//! DllPath (REG_SZ or REG_EXPAND_SZ) names the DLL, whose
//! DllGetActivationFactory gives the factory; ActivationType 0 is
//! in-process (an out-of-process server, 1, is not supported: recorded);
//! Threading is not looked at, as ThreadingModel is not for CLSIDs (gala
//! has no cross-apartment calls). The caller's thread must be in an
//! apartment or the MTA must exist (CO_E_NOTINITIALIZED, as Wine's
//! roapi tests show Windows doing). A class not registered is
//! REGDB_E_CLASSNOTREG, recorded with its name.

use core::ffi::c_void;
use core::sync::atomic::{AtomicI32, Ordering};

use super::{
    CO_E_NOTINITIALIZED, E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, GalaRecordMissing,
    GetLastError, GetProcAddress, Guid, HResult, LoadLibraryW, MTA_THREADS, S_OK, alloc, apartment,
    com_call, free, put, release,
};

const REGDB_E_CLASSNOTREG: HResult = 0x8004_0154u32 as i32;
const REGDB_E_READREGDB: HResult = 0x8004_0150u32 as i32;
const E_FAIL: HResult = 0x8000_4005u32 as i32;
const E_BOUNDS: HResult = 0x8000_000bu32 as i32;
const E_STRING_NOT_NULL_TERMINATED: HResult = 0x8000_0017u32 as i32;
const MISSING_CLASS: u32 = 2;

/// The header of an HSTRING (also an HSTRING_HEADER's content), as
/// Windows lays it out.
#[repr(C)]
pub struct HString {
    /// REFERENCE: a reference string (the caller owns header and text).
    flags: u32,
    len: u32,
    _unused: [u32; 2],
    text: *const u16,
}

/// An allocated string: the header, its reference count, the text.
#[repr(C)]
struct Owned {
    header: HString,
    refs: AtomicI32,
    text: [u16; 0],
}

const REFERENCE: u32 = 1;
/// HSTRING_HEADER's size (24 bytes on x64, 20 on x86): the header of a
/// reference string lives in the caller's.
const HEADER_SIZE: usize = if core::mem::size_of::<usize>() == 8 { 24 } else { 20 };
const _: () = assert!(core::mem::size_of::<HString>() == HEADER_SIZE);

fn header<'a>(s: *mut HString) -> &'a HString {
    // SAFETY: a live HSTRING (callers check NULL).
    unsafe { &*s }
}

/// The reference count of an allocated (not reference) string.
fn refs<'a>(s: *mut HString) -> &'a AtomicI32 {
    // SAFETY: an allocated string's header starts its Owned block.
    unsafe { &(*s.cast::<Owned>()).refs }
}

/// A new allocated string of `len` units (the text left to the caller
/// but its terminator), or None without memory.
fn allocate(len: u32) -> Option<(*mut HString, *mut u16)> {
    let offset = core::mem::offset_of!(Owned, text);
    let size = offset.checked_add((len as usize).checked_add(1)?.checked_mul(2)?)?;
    let p = alloc(size);
    if p.is_null() {
        return None;
    }
    // SAFETY: a fresh block holding the header, the count and len + 1
    // units.
    unsafe {
        let text = p.add(offset).cast::<u16>();
        text.add(len as usize).write(0);
        p.cast::<HString>().write(HString { flags: 0, len, _unused: [0; 2], text });
        p.add(core::mem::offset_of!(Owned, refs)).cast::<AtomicI32>().write(AtomicI32::new(1));
        Some((p.cast(), text))
    }
}

/// A new allocated HSTRING holding the units of `text` (NULL for none).
fn create(text: &[u16], out: *mut *mut HString) -> HResult {
    put(out, core::ptr::null_mut());
    if text.is_empty() {
        return S_OK;
    }
    let Some((s, buf)) = allocate(text.len() as u32) else { return E_OUTOFMEMORY };
    // SAFETY: the new string's buffer holds text.len() units.
    unsafe { core::ptr::copy_nonoverlapping(text.as_ptr(), buf, text.len()) };
    put(out, s);
    S_OK
}

/// The `len` units at `text` (empty for 0).
fn slice<'a>(text: *const u16, len: u32) -> &'a [u16] {
    if len == 0 {
        return &[];
    }
    // SAFETY: the caller's `len` units at `text`.
    unsafe { core::slice::from_raw_parts(text, len as usize) }
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
    if len != 0 && text.is_null() {
        return E_POINTER;
    }
    create(slice(text, len), out)
}

/// WindowsCreateStringReference: a string over the caller's text, which
/// must be NUL-terminated at `len` (even for 0: E_STRING_NOT_NULL_TERMINATED,
/// what Windows returns, Wine's string tests note), its header in the
/// caller's HSTRING_HEADER.
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
    if text.is_null() {
        return if len == 0 { S_OK } else { E_POINTER };
    }
    // SAFETY: the caller's text holds `len` units and its terminator.
    if unsafe { *text.add(len as usize) } != 0 {
        return E_STRING_NOT_NULL_TERMINATED;
    }
    if len == 0 {
        return S_OK;
    }
    put(header, HString { flags: REFERENCE, len, _unused: [0; 2], text });
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
    if refs(s).fetch_sub(1, Ordering::AcqRel) == 1 {
        free(s.cast());
    }
    S_OK
}

/// The same string with a reference more, or an allocated copy of a
/// reference string (NULL for NULL).
fn duplicate(s: *mut HString, out: *mut *mut HString) -> HResult {
    put(out, core::ptr::null_mut());
    if s.is_null() {
        return S_OK;
    }
    if header(s).flags & REFERENCE != 0 {
        return create(units(s), out);
    }
    refs(s).fetch_add(1, Ordering::Relaxed);
    put(out, s);
    S_OK
}

/// WindowsDuplicateString: the same string with a reference more, or an
/// allocated copy of a reference string.
#[unsafe(no_mangle)]
pub extern "system" fn WindowsDuplicateString(s: *mut HString, out: *mut *mut HString) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    duplicate(s, out)
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

pub(crate) fn units<'a>(s: *mut HString) -> &'a [u16] {
    let mut n = 0;
    let p = WindowsGetStringRawBuffer(s, &mut n);
    slice(p, n)
}

/// WindowsStringHasEmbeddedNull: whether a NUL is among the string's
/// units (its terminator not counted).
#[unsafe(no_mangle)]
pub extern "system" fn WindowsStringHasEmbeddedNull(s: *mut HString, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, i32::from(units(s).contains(&0)));
    S_OK
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

/// WindowsSubstring: the units from `start` (a new string; NULL when
/// empty); E_BOUNDS past the end.
#[unsafe(no_mangle)]
pub extern "system" fn WindowsSubstring(
    s: *mut HString,
    start: u32,
    out: *mut *mut HString,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let text = units(s);
    let Some(rest) = text.get(start as usize..) else {
        put(out, core::ptr::null_mut());
        return E_BOUNDS;
    };
    create(rest, out)
}

/// WindowsSubstringWithSpecifiedLength: `len` units from `start` (a new
/// string; NULL when empty); E_BOUNDS when they pass the end.
#[unsafe(no_mangle)]
pub extern "system" fn WindowsSubstringWithSpecifiedLength(
    s: *mut HString,
    start: u32,
    len: u32,
    out: *mut *mut HString,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let text = units(s);
    let end = (start as usize).checked_add(len as usize);
    let Some(part) = end.and_then(|end| text.get(start as usize..end)) else {
        put(out, core::ptr::null_mut());
        return E_BOUNDS;
    };
    create(part, out)
}

/// WindowsConcatString: `a` then `b`; one of them alone when the other is
/// empty (duplicated: the same string, or a copy of a reference string).
#[unsafe(no_mangle)]
pub extern "system" fn WindowsConcatString(
    a: *mut HString,
    b: *mut HString,
    out: *mut *mut HString,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let (x, y) = (units(a), units(b));
    if y.is_empty() {
        return duplicate(a, out);
    }
    if x.is_empty() {
        return duplicate(b, out);
    }
    put(out, core::ptr::null_mut());
    let Some(len) = x.len().checked_add(y.len()).and_then(|n| u32::try_from(n).ok()) else {
        return E_OUTOFMEMORY;
    };
    let Some((s, buf)) = allocate(len) else { return E_OUTOFMEMORY };
    // SAFETY: the new string's buffer holds both.
    unsafe {
        core::ptr::copy_nonoverlapping(x.as_ptr(), buf, x.len());
        core::ptr::copy_nonoverlapping(y.as_ptr(), buf.add(x.len()), y.len());
    }
    put(out, s);
    S_OK
}

/// WindowsTrimStringStart/End: `s` without the units at its start (or
/// end) that occur in `trim`; `s` itself (duplicated) when none do.
/// E_INVALIDARG for an empty `trim` or no output.
fn trim(s: *mut HString, set: *mut HString, out: *mut *mut HString, start: bool) -> HResult {
    if out.is_null() || units(set).is_empty() {
        return E_INVALIDARG;
    }
    let (text, set) = (units(s), units(set));
    let kept = if start {
        let from = text.iter().position(|c| !set.contains(c)).unwrap_or(text.len());
        &text[from..]
    } else {
        let to = text.iter().rposition(|c| !set.contains(c)).map_or(0, |i| i + 1);
        &text[..to]
    };
    if kept.len() == text.len() {
        return duplicate(s, out);
    }
    create(kept, out)
}

#[unsafe(no_mangle)]
pub extern "system" fn WindowsTrimStringStart(
    s: *mut HString,
    set: *mut HString,
    out: *mut *mut HString,
) -> HResult {
    trim(s, set, out, true)
}

#[unsafe(no_mangle)]
pub extern "system" fn WindowsTrimStringEnd(
    s: *mut HString,
    set: *mut HString,
    out: *mut *mut HString,
) -> HResult {
    trim(s, set, out, false)
}

/// WindowsPreallocateStringBuffer: a string of `len` units for the caller
/// to fill (its terminator set), handed back as the buffer handle; for 0,
/// no string (a NULL handle) and an empty text.
#[unsafe(no_mangle)]
pub extern "system" fn WindowsPreallocateStringBuffer(
    len: u32,
    text: *mut *mut u16,
    buffer: *mut *mut HString,
) -> HResult {
    if text.is_null() || buffer.is_null() {
        return E_POINTER;
    }
    if len == 0 {
        put(text, EMPTY.as_ptr().cast_mut());
        put(buffer, core::ptr::null_mut());
        return S_OK;
    }
    let Some((s, buf)) = allocate(len) else {
        put(text, core::ptr::null_mut());
        put(buffer, core::ptr::null_mut());
        return E_OUTOFMEMORY;
    };
    put(text, buf);
    put(buffer, s);
    S_OK
}

/// WindowsPromoteStringBuffer: the buffer becomes the string, if the
/// caller left its terminator (E_INVALIDARG otherwise).
#[unsafe(no_mangle)]
pub extern "system" fn WindowsPromoteStringBuffer(
    buffer: *mut HString,
    out: *mut *mut HString,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if buffer.is_null() {
        return S_OK;
    }
    let h = header(buffer);
    // SAFETY: the buffer's len + 1 units.
    if unsafe { *h.text.add(h.len as usize) } != 0 {
        return E_INVALIDARG;
    }
    put(out, buffer);
    S_OK
}

/// WindowsDeleteStringBuffer: frees a buffer never promoted.
#[unsafe(no_mangle)]
pub extern "system" fn WindowsDeleteStringBuffer(buffer: *mut HString) -> HResult {
    WindowsDeleteString(buffer)
}

/// Reports `function(class)` to the ledger (ASCII, truncated), the
/// HRESULT returned as its detail.
fn record_class(function: &[u8], class: *mut HString, detail: HResult) {
    let mut line = [0u8; 160];
    let mut n = 0;
    for b in function
        .iter()
        .copied()
        .chain(*b"(")
        .chain(units(class).iter().map(|&u| if u < 0x80 { u as u8 } else { b'?' }))
    {
        if n == line.len() - 1 {
            break;
        }
        line[n] = b;
        n += 1;
    }
    line[n] = b')';
    // SAFETY: a valid buffer of n + 1 bytes.
    unsafe { GalaRecordMissing(line.as_ptr(), n + 1, MISSING_CLASS, u64::from(detail as u32)) };
}

const IID_IACTIVATIONFACTORY: Guid = super::ole_iid(0x35);
use super::class::MAX_PATH;

/// The class's in-process DLL (NUL-terminated, expanded) from its
/// registration, or the HRESULT Windows gives.
fn class_dll(class: *mut HString) -> Result<[u16; MAX_PATH], HResult> {
    const ROOT: &str =
        "\\Registry\\Machine\\SOFTWARE\\Microsoft\\WindowsRuntime\\ActivatableClassId\\";
    let name = units(class);
    // A class name is a dotted name: an empty one or one with a
    // separator would name another key.
    if name.is_empty() || name.contains(&u16::from(b'\\')) || name.contains(&0) {
        return Err(REGDB_E_CLASSNOTREG);
    }
    let mut path = [0u16; 512];
    let mut n = 0;
    for c in ROOT.encode_utf16().chain(name.iter().copied()) {
        *path.get_mut(n).ok_or(REGDB_E_CLASSNOTREG)? = c;
        n += 1;
    }
    let key = &path[..n];
    if !super::class::key_exists(key) {
        return Err(REGDB_E_CLASSNOTREG);
    }
    if let Some(kind) = super::class::dword_value(key, "ActivationType")
        && kind != 0
    {
        record_class(b"ole32.dll!RoGetActivationFactory(out-of-process server)", class, E_NOTIMPL);
        return Err(E_NOTIMPL);
    }
    let raw = super::class::string_value(key, "DllPath").ok_or(REGDB_E_READREGDB)?;
    super::class::expand(&raw).ok_or(REGDB_E_READREGDB)
}

type GetActivationFactory = extern "system" fn(*mut HString, *mut *mut c_void) -> HResult;

/// RoGetActivationFactory: the factory of a registered class, asked for
/// `iid`.
#[unsafe(no_mangle)]
pub extern "system" fn RoGetActivationFactory(
    class: *mut HString,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() || iid.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    if apartment().is_none() && MTA_THREADS.load(Ordering::Relaxed) == 0 {
        return CO_E_NOTINITIALIZED;
    }
    let path = match class_dll(class) {
        Ok(path) => path,
        Err(hr) => {
            if hr != E_NOTIMPL {
                record_class(b"ole32.dll!RoGetActivationFactory", class, hr);
            }
            return hr;
        }
    };
    // SAFETY: a NUL-terminated path.
    let module = unsafe { LoadLibraryW(path.as_ptr()) };
    if module.is_null() {
        // SAFETY: plain call.
        let error = unsafe { GetLastError() };
        let hr = (0x8007_0000 | (error & 0xffff)) as HResult;
        record_class(b"ole32.dll!RoGetActivationFactory(server DLL not found)", class, hr);
        return hr;
    }
    // SAFETY: a NUL-terminated export name.
    let f = unsafe { GetProcAddress(module, c"DllGetActivationFactory".as_ptr().cast()) };
    if f.is_null() {
        record_class(
            b"ole32.dll!RoGetActivationFactory(no DllGetActivationFactory)",
            class,
            E_FAIL,
        );
        return E_FAIL;
    }
    // SAFETY: DllGetActivationFactory's documented signature.
    let get: GetActivationFactory = unsafe { core::mem::transmute(f) };
    let mut factory: *mut c_void = core::ptr::null_mut();
    let hr = get(class, &mut factory);
    if hr < 0 {
        record_class(b"ole32.dll!RoGetActivationFactory(the DLL refused)", class, hr);
        return hr;
    }
    let hr = com_call!(factory, 0, fn(*const Guid, *mut *mut c_void) -> HResult, iid, out);
    release(factory);
    hr
}

/// RoActivateInstance: the class factory's ActivateInstance.
#[unsafe(no_mangle)]
pub extern "system" fn RoActivateInstance(class: *mut HString, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let mut factory: *mut c_void = core::ptr::null_mut();
    let hr = RoGetActivationFactory(class, &IID_IACTIVATIONFACTORY, &mut factory);
    if hr < 0 {
        return hr;
    }
    // IActivationFactory::ActivateInstance, after IInspectable's six.
    let hr = com_call!(factory, 6, fn(*mut *mut c_void) -> HResult, out);
    release(factory);
    hr
}
