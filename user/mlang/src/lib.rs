//! gala's mlang.dll: Multi Language Support, the class CMultiLanguage
//! (ARCHITECTURE "Multi Language Support"), registered as an in-process
//! server (the registry seed) and created through ole32's
//! CoCreateInstance and this DLL's DllGetClassObject. Office's shared
//! code asks for it on the way to laying text out.
//!
//! What it answers is what a program can learn from tables and from the
//! font a DC has selected: the code-page bits of a code page and back
//! (`tables`), the script of a code page, and the code pages of a font
//! (`fonts`, over gdi32's GetTextCharsetInfo). What needs encode tables
//! gala's NLS does not have yet, a face's Unicode coverage or the MIME
//! database fails honestly: E_NOTIMPL, recorded in the ledger as
//! `mlang.dll!<interface>::<method>` ("not implemented").

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

use alloc::string::String;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use gala_heap as _;

mod com;
mod fonts;
mod tables;

// The 32-bit build (SysWOW64) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

// The x64 build's memory routines come from gala's ntdll (as other
// DLLs on `alloc` take them).
#[cfg(not(target_arch = "x86"))]
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
#[cfg(not(target_arch = "x86"))]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

pub type HResult = i32;
pub type Guid = [u8; 16];

pub const S_OK: HResult = 0;
pub const S_FALSE: HResult = 1;
pub const E_NOTIMPL: HResult = 0x8000_4001u32 as i32;
pub const E_NOINTERFACE: HResult = 0x8000_4002u32 as i32;
pub const E_POINTER: HResult = 0x8000_4003u32 as i32;
pub const E_FAIL: HResult = 0x8000_4005u32 as i32;
pub const E_INVALIDARG: HResult = 0x8007_0057u32 as i32;
pub const E_OUTOFMEMORY: HResult = 0x8007_000Eu32 as i32;
const CLASS_E_NOAGGREGATION: HResult = 0x8004_0110u32 as i32;
const CLASS_E_CLASSNOTAVAILABLE: HResult = 0x8004_0111u32 as i32;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

/// The ledger's kinds (GalaRecordMissing's codes).
pub const LEDGER_UNSUPPORTED: u32 = 2;
pub const LEDGER_NOT_IMPLEMENTED: u32 = 3;
pub const LEDGER_OTHER: u32 = 5;
pub const LEDGER_REACHED: u32 = 9;

/// One bit per method gala answers, so that the first call which
/// reached it leaves a row and the thousandth leaves none: a ledger
/// entry is keyed by its detail as well as its name, so a row written
/// per call would be a row per argument value and would crowd out the
/// table (OBSERVABILITY "Missing-features ledger").
pub const REACHED_CREATED: u32 = 0;
pub const REACHED_CODE_PAGE_TO_CODE_PAGES: u32 = 1;
pub const REACHED_CODE_PAGES_TO_CODE_PAGE: u32 = 2;
pub const REACHED_CODE_PAGE_TO_SCRIPT_ID: u32 = 3;
pub const REACHED_FONT_CODE_PAGES: u32 = 4;
pub const REACHED_MAP_FONT: u32 = 5;
pub const REACHED_RELEASE_FONT: u32 = 6;
pub const REACHED_RESET_FONT_MAPPING: u32 = 7;

static REACHED: AtomicU32 = AtomicU32::new(0);

/// Records that a method gala answers was reached, once: a run says
/// which of mlang's methods a program actually called, beside the rows
/// for the ones it asked for and did not get.
pub fn reached(bit: u32, what: &[u8]) {
    if REACHED.fetch_or(1 << bit, Ordering::Relaxed) & (1 << bit) == 0 {
        record(what, LEDGER_REACHED, 0);
    }
}

/// Records `mlang.dll!<what>` in the ledger (printable ASCII; cut at 96
/// bytes).
pub fn record(what: &[u8], kind: u32, detail: u64) {
    let mut name = [0u8; 96];
    let mut n = 0;
    for &b in b"mlang.dll!".iter().chain(what) {
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

/// CLSID_CMultiLanguage, the one class this DLL serves.
const CLSID_CMULTILANGUAGE: Guid =
    guid(0x275c_23e2, 0x3747, 0x11d0, [0x9f, 0xea, 0x00, 0xaa, 0x00, 0x3f, 0x86, 0x46]);
pub const IID_IUNKNOWN: Guid = guid(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_ICLASSFACTORY: Guid = guid(1, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);

/// Records an interface a QueryInterface did not answer:
/// `QI? <object> {xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}` (the whole IID:
/// its first dword alone does not name it), the first dword as detail.
pub fn record_iid(what: &str, g: &Guid) {
    let mut s = String::from(what);
    s.push_str(" {");
    let hex = |s: &mut String, b: u8| {
        for n in [b >> 4, b & 15] {
            s.push(char::from(b"0123456789ABCDEF"[usize::from(n)]));
        }
    };
    for i in [3, 2, 1, 0] {
        hex(&mut s, g[i]);
    }
    s.push('-');
    for i in [5, 4] {
        hex(&mut s, g[i]);
    }
    s.push('-');
    for i in [7, 6] {
        hex(&mut s, g[i]);
    }
    s.push('-');
    for (i, &b) in g[8..].iter().enumerate() {
        if i == 2 {
            s.push('-');
        }
        hex(&mut s, b);
    }
    s.push('}');
    record(s.as_bytes(), LEDGER_OTHER, data1(g));
}

/// The first dword of a GUID (a ledger detail).
pub fn data1(g: &Guid) -> u64 {
    u64::from(u32::from_le_bytes([g[0], g[1], g[2], g[3]]))
}

// ---- The class object ----

#[repr(C)]
pub struct VTable<const N: usize>(pub [*const (); N]);
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
        record_iid("QI? CMultiLanguage class object", &iid);
        put(out, core::ptr::null_mut());
        E_NOINTERFACE
    }
}

/// The class object is static: its references are not counted.
extern "system" fn factory_add_ref(_this: *mut c_void) -> u32 {
    2
}
extern "system" fn factory_release(_this: *mut c_void) -> u32 {
    1
}

/// CreateInstance. With an outer object Windows serves the inner
/// IUnknown and refuses every other interface (CLASS_E_NOAGGREGATION);
/// gala serves the same object, whose QueryInterface does not delegate
/// to the outer -- recorded, so a program that really aggregates shows.
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
        if riid.is_null() {
            return E_POINTER;
        }
        // SAFETY: the caller's IID.
        if unsafe { *riid } != IID_IUNKNOWN {
            return CLASS_E_NOAGGREGATION;
        }
        record(b"CMultiLanguage(aggregated: the inner does not delegate)", LEDGER_UNSUPPORTED, 0);
    }
    com::new_object(riid, out)
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

/// DllGetClassObject: the class object of CMultiLanguage; another class
/// is recorded, CLASS_E_CLASSNOTAVAILABLE.
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
    if c != CLSID_CMULTILANGUAGE {
        record(b"DllGetClassObject(class)", LEDGER_UNSUPPORTED, data1(&c));
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    factory_query(&FACTORY as *const Factory as *mut c_void, riid, out)
}

/// DllCanUnloadNow: S_FALSE (gala does not unload mlang).
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

/// A panic is a bug (or the heap ran out): its location goes to the
/// ledger (`mlang.dll!panic <file>`, the line as detail) and the process
/// ends with STATUS_STACK_BUFFER_OVERRUN (fail-fast), as gala's other
/// DLLs do.
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
