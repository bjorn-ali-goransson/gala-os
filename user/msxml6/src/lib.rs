//! gala's msxml6.dll: MSXML 6's DOM, the class DOMDocument60
//! (ARCHITECTURE "msxml6"), registered as an in-process server (the registry seed) and created through ole32's
//! CoCreateInstance and this DLL's DllGetClassObject.
//!
//! The document parses with gala's XML reader (`gala_xml`, shared with
//! galapkg) into a tree owned by the document (`dom`), selects with XPath
//! 1.0 (`xpath`), and hands programs node objects that wrap the document
//! and an index (`com`). What it does not implement fails honestly:
//! E_NOTIMPL, recorded in the ledger as `msxml6.dll!<interface>::<method>`
//! ("not implemented"); loads it cannot do (URLs, streams) are recorded
//! and fail.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;

use gala_heap as _;

// The 32-bit build (SysWOW64) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

mod com;
mod dom;
mod xpath;

// The x64 build's memory routines come from gala's ntdll (as other
// DLLs on `alloc` take them).
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

/// MSVC-target codegen references this marker symbol directly (ntrt's
/// x86 runtime defines it on x86).
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

pub type HResult = i32;
pub type Guid = [u8; 16];
pub type Bstr = *mut u16;

pub const S_OK: HResult = 0;
pub const S_FALSE: HResult = 1;
pub const E_NOTIMPL: HResult = 0x8000_4001u32 as i32;
pub const E_NOINTERFACE: HResult = 0x8000_4002u32 as i32;
pub const E_POINTER: HResult = 0x8000_4003u32 as i32;
pub const E_FAIL: HResult = 0x8000_4005u32 as i32;
pub const E_OUTOFMEMORY: HResult = 0x8007_000eu32 as i32;
pub const E_INVALIDARG: HResult = 0x8007_0057u32 as i32;
const CLASS_E_NOAGGREGATION: HResult = 0x8004_0110u32 as i32;
const CLASS_E_CLASSNOTAVAILABLE: HResult = 0x8004_0111u32 as i32;

pub const VARIANT_TRUE: i16 = -1;
pub const VARIANT_FALSE: i16 = 0;
pub const VT_EMPTY: u16 = 0;
pub const VT_NULL: u16 = 1;
pub const VT_I2: u16 = 2;
pub const VT_I4: u16 = 3;
pub const VT_BSTR: u16 = 8;
pub const VT_BOOL: u16 = 11;
pub const VT_UNKNOWN: u16 = 13;
pub const VT_UI4: u16 = 19;
pub const VT_INT: u16 = 22;
pub const VT_BYREF: u16 = 0x4000;

/// A VARIANT (24 bytes on x64, 16 on x86).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Variant {
    pub vt: u16,
    pub reserved: [u16; 3],
    pub value: [usize; 2],
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "oleaut32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "oleaut32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn SysAllocStringLen(s: *const u16, len: u32) -> Bstr;
}

/// The ledger's kinds (GalaRecordMissing's codes).
pub const LEDGER_UNSUPPORTED: u32 = 2;
pub const LEDGER_NOT_IMPLEMENTED: u32 = 3;
pub const LEDGER_OTHER: u32 = 5;

/// Records `msxml6.dll!<what>` in the ledger (printable ASCII; cut at
/// 96 bytes).
pub fn record(what: &[u8], kind: u32, detail: u64) {
    let mut name = [0u8; 96];
    let mut n = 0;
    for &b in b"msxml6.dll!".iter().chain(what) {
        if n == name.len() {
            break;
        }
        name[n] = match b {
            b'\\' => b'/',
            b'"' => b'\'',
            0x20..=0x7e => b,
            _ => b'?',
        };
        n += 1;
    }
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(name.as_ptr(), n, kind, detail) };
}

/// A method gala does not implement: recorded, E_NOTIMPL.
pub fn not_implemented(method: &[u8]) -> HResult {
    record(method, LEDGER_NOT_IMPLEMENTED, 0);
    E_NOTIMPL
}

/// The caller's NUL-terminated wide string (a BSTR, or any string) as
/// text; unpaired surrogates become U+FFFD. None for NULL or no memory.
pub fn text(s: *const u16) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    while unsafe { *s.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: n units, read above.
    let units = unsafe { core::slice::from_raw_parts(s, n) };
    let mut out = String::new();
    out.try_reserve(n).ok()?;
    for c in char::decode_utf16(units.iter().copied()) {
        out.push(c.unwrap_or('\u{fffd}'));
    }
    Some(out)
}

/// `s` as a new BSTR (NULL when out of memory).
pub fn bstr(s: &str) -> Bstr {
    let mut units: Vec<u16> = Vec::new();
    if units.try_reserve(s.len()).is_err() {
        return core::ptr::null_mut();
    }
    units.extend(s.encode_utf16());
    // SAFETY: `units` holds that many characters.
    unsafe { SysAllocStringLen(units.as_ptr(), units.len() as u32) }
}

/// Stores `v` at the caller's output, if any.
pub fn put<T>(out: *mut T, v: T) {
    if !out.is_null() {
        // SAFETY: the caller's output.
        unsafe { out.write(v) };
    }
}

/// A new heap object holding `v` (None when out of memory).
pub fn try_box<T>(v: T) -> Option<*mut T> {
    let layout = core::alloc::Layout::new::<T>();
    // SAFETY: a non-zero-sized layout.
    let p = unsafe { alloc::alloc::alloc(layout) }.cast::<T>();
    if p.is_null() {
        return None;
    }
    // SAFETY: fresh memory for one T.
    unsafe { p.write(v) };
    Some(p)
}

/// Frees an object from `try_box`.
///
/// # Safety
/// `p` came from `try_box::<T>` and is not used after.
pub unsafe fn free_box<T>(p: *mut T) {
    // SAFETY: as the caller promises.
    unsafe {
        core::ptr::drop_in_place(p);
        alloc::alloc::dealloc(p.cast(), core::alloc::Layout::new::<T>());
    }
}

pub const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

const MSXML6_D4: [u8; 8] = [0xa6, 0x5f, 0x00, 0x40, 0x96, 0x32, 0x51, 0xe5];
const CLSID_DOMDOCUMENT60: Guid = guid(0x88d9_6a05, 0xf192, 0x11d4, MSXML6_D4);
pub const IID_IUNKNOWN: Guid = guid(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_ICLASSFACTORY: Guid = guid(1, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);

/// The first dword of a GUID (a ledger detail).
pub fn data1(g: &Guid) -> u64 {
    u64::from(u32::from_le_bytes([g[0], g[1], g[2], g[3]]))
}

// ---- The class objects ----

#[repr(C)]
struct VTable<const N: usize>([*const (); N]);
// SAFETY: function pointers only.
unsafe impl<const N: usize> Sync for VTable<N> {}

extern "system" fn factory_query(
    this: *mut c_void,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() || riid.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *riid };
    if iid == IID_IUNKNOWN || iid == IID_ICLASSFACTORY {
        put(out, this);
        S_OK
    } else {
        put(out, core::ptr::null_mut());
        E_NOINTERFACE
    }
}

/// The class objects are static: their references are not counted.
extern "system" fn factory_add_ref(_this: *mut c_void) -> u32 {
    2
}
extern "system" fn factory_release(_this: *mut c_void) -> u32 {
    1
}

extern "system" fn factory_create(
    _this: *mut c_void,
    outer: *mut c_void,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if !outer.is_null() {
        return CLASS_E_NOAGGREGATION;
    }
    com::new_document(riid, out)
}

extern "system" fn factory_lock(_this: *mut c_void, _lock: i32) -> HResult {
    S_OK
}

static FACTORY_VTABLE: VTable<5> = VTable([
    factory_query as *const (),
    factory_add_ref as *const (),
    factory_release as *const (),
    factory_create as *const (),
    factory_lock as *const (),
]);

#[repr(C)]
struct Factory {
    vtable: *const VTable<5>,
}
// SAFETY: an immutable pointer to a static.
unsafe impl Sync for Factory {}
static FACTORY: Factory = Factory { vtable: &FACTORY_VTABLE };

/// DllGetClassObject: the class object of DOMDocument60; another class
/// (FreeThreadedDOMDocument60 among them: not registered) is recorded,
/// CLASS_E_CLASSNOTAVAILABLE.
#[unsafe(no_mangle)]
pub extern "system" fn DllGetClassObject(
    clsid: *const Guid,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() || clsid.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    // SAFETY: the caller's CLSID.
    let c = unsafe { *clsid };
    if c != CLSID_DOMDOCUMENT60 {
        record(b"DllGetClassObject(class)", LEDGER_UNSUPPORTED, data1(&c));
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    factory_query(&FACTORY as *const Factory as *mut c_void, riid, out)
}

/// DllCanUnloadNow: S_FALSE (gala does not unload msxml6).
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HResult {
    S_FALSE
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn ExitProcess(code: u32) -> !;
}

/// A panic is a bug (or the heap ran out inside the XML reader): its
/// location goes to the ledger (`msxml6.dll!panic <file>`, the line as
/// detail) and the process ends with STATUS_STACK_BUFFER_OVERRUN
/// (fail-fast), as gala's other DLLs do.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut text = [0u8; 80];
    let mut n = 0;
    let line = info.location().map_or(0, |l| {
        for &b in b"panic ".iter().chain(l.file().as_bytes()) {
            if n < text.len() {
                text[n] = b;
                n += 1;
            }
        }
        l.line()
    });
    record(&text[..n], LEDGER_OTHER, line.into());
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
