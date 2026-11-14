//! Windows Runtime error origination (roerrorapi.h, restrictederrorinfo.h),
//! as documented on learn.microsoft.com:
//!
//! - RoOriginateError / RoOriginateErrorW / RoOriginateLanguageException
//!   make an error object (the HRESULT, the message, at most 512
//!   characters, and the language exception held) and make it the
//!   thread's error (SetRestrictedErrorInfo) unless the reporting flags
//!   say RO_ERROR_REPORTING_SUPPRESSSETERRORINFO; TRUE when the error was
//!   originated, FALSE for a success HRESULT. No debugger is notified
//!   (gala has none to tell).
//! - The thread has one error slot, as on Windows: SetRestrictedErrorInfo
//!   puts an object there (its IErrorInfo, if it has one, is what
//!   GetErrorInfo hands out), SetErrorInfo replaces it; GetRestrictedErrorInfo
//!   and GetErrorInfo take it and leave the slot empty (S_FALSE and NULL
//!   when it is). RoClearError empties it.
//! - The error object answers IRestrictedErrorInfo (GetErrorDetails: the
//!   system's text for the HRESULT when FormatMessage has one, the HRESULT,
//!   the message, no capability SID; GetReference: none), IErrorInfo (the
//!   message as the description) and ILanguageExceptionErrorInfo (the
//!   language exception). BSTRs come from oleaut32.
//! - RoGetErrorReportingFlags / RoSetErrorReportingFlags keep the
//!   process's flags, RO_ERROR_REPORTING_USESETERRORINFO at start (Wine's
//!   roapi tests read that on Windows). RoCaptureErrorContext captures
//!   nothing (gala keeps no stack back traces for error objects): S_OK.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use super::{
    E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER, Guid, HResult, IID_IUNKNOWN, S_FALSE,
    S_OK, VTable, add_ref, alloc, com_call, free, put, release, thread_state,
    winrt::{HString, units},
};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn FormatMessageW(
        flags: u32,
        source: *const c_void,
        id: u32,
        language: u32,
        buffer: *mut u16,
        size: u32,
        args: *const c_void,
    ) -> u32;
}

/// A GUID in its binary layout from its text form's fields.
const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

/// {82BA7092-4C88-427D-A7BC-16DD93FEB67E}
pub(crate) const IID_IRESTRICTEDERRORINFO: Guid =
    guid(0x82ba7092, 0x4c88, 0x427d, [0xa7, 0xbc, 0x16, 0xdd, 0x93, 0xfe, 0xb6, 0x7e]);
/// {1CF2B120-547D-101B-8E65-08002B2BA0B6}
pub(crate) const IID_IERRORINFO: Guid =
    guid(0x1cf2b120, 0x547d, 0x101b, [0x8e, 0x65, 0x08, 0x00, 0x2b, 0x2b, 0xa0, 0xb6]);
/// {04A2DBF3-DF83-116C-0946-0812ABF6E07D}
const IID_ILANGUAGEEXCEPTIONERRORINFO: Guid =
    guid(0x04a2dbf3, 0xdf83, 0x116c, [0x09, 0x46, 0x08, 0x12, 0xab, 0xf6, 0xe0, 0x7d]);

const RO_ERROR_REPORTING_SUPPRESSSETERRORINFO: u32 = 0x8;
const RO_ERROR_REPORTING_USESETERRORINFO: u32 = 0x4;
/// The documented limit of an originated error's message.
const MAX_MESSAGE: usize = 512;

static REPORTING_FLAGS: AtomicU32 = AtomicU32::new(RO_ERROR_REPORTING_USESETERRORINFO);

/// An originated error: three interfaces, then its state.
#[repr(C)]
struct ErrorObject {
    restricted: *const (),
    error_info: *const (),
    language: *const (),
    refs: AtomicU32,
    hr: HResult,
    /// The language exception (held), or 0.
    exception: usize,
    len: usize,
    message: [u16; MAX_MESSAGE],
}

const PTR: usize = core::mem::size_of::<usize>();

/// The object behind interface pointer `this` at slot `slot`.
fn object<'a>(this: usize, slot: usize) -> &'a ErrorObject {
    // SAFETY: `this` is the object's interface pointer number `slot`.
    unsafe { &*((this - slot * PTR) as *const ErrorObject) }
}

fn query(o: &ErrorObject, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if iid.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *iid };
    let base = o as *const ErrorObject as usize;
    let p = if iid == IID_IUNKNOWN || iid == IID_IRESTRICTEDERRORINFO {
        base
    } else if iid == IID_IERRORINFO {
        base + PTR
    } else if iid == IID_ILANGUAGEEXCEPTIONERRORINFO {
        base + 2 * PTR
    } else {
        return E_NOINTERFACE;
    };
    o.refs.fetch_add(1, Ordering::Relaxed);
    put(out, p as *mut c_void);
    S_OK
}

fn add(o: &ErrorObject) -> u32 {
    o.refs.fetch_add(1, Ordering::Relaxed) + 1
}

fn drop_ref(o: &ErrorObject) -> u32 {
    let left = o.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        if o.exception != 0 {
            release(o.exception as *mut c_void);
        }
        free(o as *const ErrorObject as *mut u8);
    }
    left
}

/// A BSTR copy of `text` (NULL for empty or without oleaut32).
fn bstr(text: &[u16]) -> usize {
    if text.is_empty() {
        return 0;
    }
    let f = super::props::export(&super::props::OLEAUT32, b"SysAllocStringLen\0");
    if f == 0 {
        super::missing(b"ole32.dll!IRestrictedErrorInfo(oleaut32 for BSTRs)", 0);
        return 0;
    }
    // SAFETY: oleaut32's SysAllocStringLen(const OLECHAR*, UINT).
    let f =
        unsafe { core::mem::transmute::<usize, extern "system" fn(*const u16, u32) -> usize>(f) };
    f(text.as_ptr(), text.len() as u32)
}

/// The system's text for `hr` as a BSTR (NULL when FormatMessage has
/// none), its trailing line end dropped.
fn system_text(hr: HResult) -> usize {
    let mut buf = [0u16; MAX_MESSAGE];
    // SAFETY: a buffer of MAX_MESSAGE characters; FORMAT_MESSAGE_FROM_SYSTEM
    // | FORMAT_MESSAGE_IGNORE_INSERTS.
    let n = unsafe {
        FormatMessageW(
            0x1000 | 0x200,
            core::ptr::null(),
            hr as u32,
            0,
            buf.as_mut_ptr(),
            buf.len() as u32,
            core::ptr::null(),
        )
    } as usize;
    let mut text = &buf[..n.min(buf.len())];
    while let [rest @ .., 0x0d | 0x0a] = text {
        text = rest;
    }
    bstr(text)
}

// IRestrictedErrorInfo (slot 0).
extern "system" fn r_query(this: usize, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    query(object(this, 0), iid, out)
}
extern "system" fn r_add_ref(this: usize) -> u32 {
    add(object(this, 0))
}
extern "system" fn r_release(this: usize) -> u32 {
    drop_ref(object(this, 0))
}
extern "system" fn r_get_error_details(
    this: usize,
    description: *mut usize,
    error: *mut HResult,
    restricted: *mut usize,
    sid: *mut usize,
) -> HResult {
    if description.is_null() || error.is_null() || restricted.is_null() || sid.is_null() {
        return E_INVALIDARG;
    }
    let o = object(this, 0);
    put(description, system_text(o.hr));
    put(error, o.hr);
    put(restricted, bstr(&o.message[..o.len]));
    put(sid, 0);
    S_OK
}
extern "system" fn r_get_reference(_this: usize, reference: *mut usize) -> HResult {
    if reference.is_null() {
        return E_INVALIDARG;
    }
    put(reference, 0);
    S_OK
}

// IErrorInfo (slot 1).
extern "system" fn e_query(this: usize, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    query(object(this, 1), iid, out)
}
extern "system" fn e_add_ref(this: usize) -> u32 {
    add(object(this, 1))
}
extern "system" fn e_release(this: usize) -> u32 {
    drop_ref(object(this, 1))
}
extern "system" fn e_get_guid(_this: usize, out: *mut Guid) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, [0; 16]);
    S_OK
}
extern "system" fn e_none(_this: usize, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, 0);
    S_OK
}
extern "system" fn e_get_description(this: usize, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let o = object(this, 1);
    put(out, bstr(&o.message[..o.len]));
    S_OK
}
extern "system" fn e_get_help_context(_this: usize, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, 0);
    S_OK
}

// ILanguageExceptionErrorInfo (slot 2).
extern "system" fn l_query(this: usize, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    query(object(this, 2), iid, out)
}
extern "system" fn l_add_ref(this: usize) -> u32 {
    add(object(this, 2))
}
extern "system" fn l_release(this: usize) -> u32 {
    drop_ref(object(this, 2))
}
extern "system" fn l_get_language_exception(this: usize, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let o = object(this, 2);
    if o.exception != 0 {
        add_ref(o.exception as *mut c_void);
    }
    put(out, o.exception);
    S_OK
}

static RESTRICTED_VTABLE: VTable<5> = VTable([
    r_query as *const (),
    r_add_ref as *const (),
    r_release as *const (),
    r_get_error_details as *const (),
    r_get_reference as *const (),
]);
static ERROR_INFO_VTABLE: VTable<8> = VTable([
    e_query as *const (),
    e_add_ref as *const (),
    e_release as *const (),
    e_get_guid as *const (),
    e_none as *const (),
    e_get_description as *const (),
    e_none as *const (),
    e_get_help_context as *const (),
]);
static LANGUAGE_VTABLE: VTable<4> = VTable([
    l_query as *const (),
    l_add_ref as *const (),
    l_release as *const (),
    l_get_language_exception as *const (),
]);

/// Originates `hr` with `message` and `exception` (held): the thread's
/// error unless suppressed. FALSE for a success HRESULT.
fn originate(hr: HResult, message: &[u16], exception: *mut c_void) -> i32 {
    if hr >= 0 {
        return 0;
    }
    if REPORTING_FLAGS.load(Ordering::Relaxed) & RO_ERROR_REPORTING_SUPPRESSSETERRORINFO != 0 {
        return 0;
    }
    let p = alloc(core::mem::size_of::<ErrorObject>()).cast::<ErrorObject>();
    if p.is_null() {
        return 0;
    }
    let len = message.iter().position(|&c| c == 0).unwrap_or(message.len()).min(MAX_MESSAGE);
    let mut text = [0u16; MAX_MESSAGE];
    text[..len].copy_from_slice(&message[..len]);
    if !exception.is_null() {
        add_ref(exception);
    }
    // SAFETY: fresh memory of the object's size.
    unsafe {
        p.write(ErrorObject {
            restricted: RESTRICTED_VTABLE.0.as_ptr().cast(),
            error_info: ERROR_INFO_VTABLE.0.as_ptr().cast(),
            language: LANGUAGE_VTABLE.0.as_ptr().cast(),
            refs: AtomicU32::new(1),
            hr,
            exception: exception as usize,
            len,
            message: text,
        })
    };
    let hr = SetRestrictedErrorInfo(p.cast());
    release(p.cast());
    i32::from(hr >= 0)
}

/// RoOriginateError: originates `hr` with an HSTRING message.
#[unsafe(no_mangle)]
pub extern "system" fn RoOriginateError(hr: HResult, message: *mut HString) -> i32 {
    originate(hr, units(message), core::ptr::null_mut())
}

/// RoOriginateErrorW: originates `hr` with `max` characters of `message`
/// (0: up to its NUL).
#[unsafe(no_mangle)]
pub extern "system" fn RoOriginateErrorW(hr: HResult, max: u32, message: *const u16) -> i32 {
    originate(hr, wide_text(message, max), core::ptr::null_mut())
}

/// The caller's text: `max` characters, or up to its NUL for 0 (at most
/// MAX_MESSAGE either way).
fn wide_text<'a>(text: *const u16, max: u32) -> &'a [u16] {
    if text.is_null() {
        return &[];
    }
    let limit = if max == 0 { MAX_MESSAGE } else { (max as usize).min(MAX_MESSAGE) };
    let mut n = 0;
    // SAFETY: the caller's text, read up to its NUL or the limit.
    while n < limit && unsafe { *text.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: the n characters just read.
    unsafe { core::slice::from_raw_parts(text, n) }
}

/// RoOriginateLanguageException: RoOriginateError holding the language's
/// exception object.
#[unsafe(no_mangle)]
pub extern "system" fn RoOriginateLanguageException(
    hr: HResult,
    message: *mut HString,
    exception: *mut c_void,
) -> i32 {
    originate(hr, units(message), exception)
}

/// SetRestrictedErrorInfo: `info` (or nothing, for NULL) becomes the
/// thread's error.
#[unsafe(no_mangle)]
pub extern "system" fn SetRestrictedErrorInfo(info: *mut c_void) -> HResult {
    let Some(state) = thread_state() else { return E_OUTOFMEMORY };
    let mut error_info: *mut c_void = core::ptr::null_mut();
    if !info.is_null() {
        add_ref(info);
        com_call!(
            info,
            0,
            fn(*const Guid, *mut *mut c_void) -> HResult,
            &IID_IERRORINFO,
            &mut error_info
        );
    }
    let old = core::mem::replace(&mut state.restricted_info, info as usize);
    let old_error = core::mem::replace(&mut state.error_info, error_info as usize);
    for p in [old, old_error] {
        if p != 0 {
            release(p as *mut c_void);
        }
    }
    S_OK
}

/// GetRestrictedErrorInfo: hands over the thread's restricted error
/// object and empties the slot; S_FALSE and NULL when there is none.
#[unsafe(no_mangle)]
pub extern "system" fn GetRestrictedErrorInfo(out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let Some(state) = thread_state() else { return E_OUTOFMEMORY };
    let info = core::mem::replace(&mut state.restricted_info, 0);
    if info == 0 {
        return S_FALSE;
    }
    let error_info = core::mem::replace(&mut state.error_info, 0);
    if error_info != 0 {
        release(error_info as *mut c_void);
    }
    put(out, info as *mut c_void);
    S_OK
}

/// RoClearError: the thread has no error.
#[unsafe(no_mangle)]
pub extern "system" fn RoClearError() {
    SetRestrictedErrorInfo(core::ptr::null_mut());
}

/// RoCaptureErrorContext: nothing to capture (no stack back traces are
/// kept for error objects).
#[unsafe(no_mangle)]
pub extern "system" fn RoCaptureErrorContext(_hr: HResult) -> HResult {
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn RoGetErrorReportingFlags(flags: *mut u32) -> HResult {
    if flags.is_null() {
        return E_POINTER;
    }
    put(flags, REPORTING_FLAGS.load(Ordering::Relaxed));
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn RoSetErrorReportingFlags(flags: u32) -> HResult {
    REPORTING_FLAGS.store(flags, Ordering::Relaxed);
    S_OK
}
