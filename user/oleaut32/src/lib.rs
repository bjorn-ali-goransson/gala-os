//! gala's oleaut32.dll: OLE Automation's data types, as documented:
//! BSTRs (a 4-byte byte length before the text, a NUL after it), VARIANTs
//! (clearing, copying, converting between numbers, booleans and strings
//! in the invariant/en-US format), SAFEARRAYs (safearray.rs) and
//! VarCyFromDec. gala has no registry, so no type library is registered
//! (TYPE_E_LIBNOTREGISTERED) and loading .tlb files is not there yet
//! (recorded). Exports have the standard ordinals (build.rs).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;

mod safearray;
mod variant;

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

/// MSVC-target codegen references this marker symbol directly.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
}

pub type HResult = i32;

pub const S_OK: HResult = 0;
pub const E_NOTIMPL: HResult = 0x8000_4001u32 as i32;
pub const E_OUTOFMEMORY: HResult = 0x8007_000eu32 as i32;
pub const E_INVALIDARG: HResult = 0x8007_0057u32 as i32;
pub const DISP_E_OVERFLOW: HResult = 0x8002_000au32 as i32;
const TYPE_E_LIBNOTREGISTERED: HResult = 0x8002_801du32 as i32;
const TYPE_E_CANTLOADLIBRARY: HResult = 0x8002_9c4au32 as i32;

pub fn missing(function: &[u8], detail: u64) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(function.as_ptr(), function.len(), 2, detail) };
}

pub fn alloc_zeroed(size: usize) -> *mut u8 {
    // SAFETY: plain call; HEAP_ZERO_MEMORY.
    unsafe { HeapAlloc(GetProcessHeap(), 0x8, size) }
}

pub fn free(p: *mut u8) {
    if !p.is_null() {
        // SAFETY: memory from `alloc_zeroed`.
        unsafe { HeapFree(GetProcessHeap(), 0, p) };
    }
}

/// Stores `value` at the caller's output pointer.
pub fn put<T>(out: *mut T, value: T) {
    // SAFETY: the caller's output pointer (checked non-null by callers).
    unsafe { out.write(value) };
}

/// The vtable entry `i` of COM object `obj`, called as `fn(obj, args)`.
macro_rules! com_call {
    ($obj:expr, $i:expr, fn($($t:ty),*) -> $r:ty $(, $a:expr)*) => {{
        let obj = $obj as *mut *const usize;
        // SAFETY: a COM object whose vtable has entry $i of this type.
        let f: extern "system" fn(*mut *const usize $(, $t)*) -> $r =
            unsafe { core::mem::transmute(*(*obj).add($i)) };
        f(obj $(, $a)*)
    }};
}
pub(crate) use com_call;

pub fn add_ref(obj: usize) {
    if obj != 0 {
        com_call!(obj, 1, fn() -> u32);
    }
}

pub fn release(obj: usize) {
    if obj != 0 {
        com_call!(obj, 2, fn() -> u32);
    }
}

// ---- BSTR ----

/// A new BSTR of `bytes` bytes (zeroed, then NUL-terminated), or null.
fn bstr_bytes(bytes: usize) -> *mut u16 {
    let Ok(prefix) = u32::try_from(bytes) else { return core::ptr::null_mut() };
    // The length, the text, a NUL character (and an odd byte's padding).
    let block = alloc_zeroed(4 + bytes + 3);
    if block.is_null() {
        return block.cast();
    }
    // SAFETY: a block of that size.
    unsafe {
        *(block as *mut u32) = prefix;
        block.add(4).cast()
    }
}

/// The length of a NUL-terminated UTF-16 string.
pub fn wide_len(s: *const u16) -> usize {
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    unsafe {
        while *s.add(n) != 0 {
            n += 1;
        }
    }
    n
}

#[unsafe(no_mangle)]
pub extern "system" fn SysAllocString(s: *const u16) -> *mut u16 {
    if s.is_null() {
        return core::ptr::null_mut();
    }
    SysAllocStringLen(s, wide_len(s) as u32)
}

/// SysAllocStringLen: `len` characters copied from `s` (if any).
#[unsafe(no_mangle)]
pub extern "system" fn SysAllocStringLen(s: *const u16, len: u32) -> *mut u16 {
    let p = bstr_bytes(2 * len as usize);
    if !p.is_null() && !s.is_null() {
        // SAFETY: `len` characters at `s`; the new string holds them.
        unsafe { core::ptr::copy_nonoverlapping(s, p, len as usize) };
    }
    p
}

/// SysAllocStringByteLen: `len` bytes copied from `s` (if any).
#[unsafe(no_mangle)]
pub extern "system" fn SysAllocStringByteLen(s: *const u8, len: u32) -> *mut u16 {
    let p = bstr_bytes(len as usize);
    if !p.is_null() && !s.is_null() {
        // SAFETY: as above, in bytes.
        unsafe { core::ptr::copy_nonoverlapping(s, p.cast::<u8>(), len as usize) };
    }
    p
}

#[unsafe(no_mangle)]
pub extern "system" fn SysFreeString(s: *mut u16) {
    if !s.is_null() {
        // SAFETY: a BSTR: its block starts 4 bytes before.
        free(unsafe { s.cast::<u8>().sub(4) });
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn SysStringByteLen(s: *const u16) -> u32 {
    if s.is_null() {
        return 0;
    }
    // SAFETY: a BSTR's length prefix.
    unsafe { *(s.cast::<u8>().sub(4) as *const u32) }
}

#[unsafe(no_mangle)]
pub extern "system" fn SysStringLen(s: *const u16) -> u32 {
    SysStringByteLen(s) / 2
}

/// SysReAllocStringLen: a new string replaces `*old` (freed); TRUE.
#[unsafe(no_mangle)]
pub extern "system" fn SysReAllocStringLen(old: *mut *mut u16, s: *const u16, len: u32) -> i32 {
    if old.is_null() {
        return 0;
    }
    let p = SysAllocStringLen(s, len);
    if p.is_null() {
        return 0;
    }
    // SAFETY: the caller's BSTR pointer (the source may be inside it: it
    // was copied first).
    unsafe { SysFreeString(core::mem::replace(&mut *old, p)) };
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn SysReAllocString(old: *mut *mut u16, s: *const u16) -> i32 {
    let len = if s.is_null() { 0 } else { wide_len(s) as u32 };
    SysReAllocStringLen(old, s, len)
}

// ---- Currency ----

/// DECIMAL: wReserved, scale, sign, Hi32, Lo64.
#[repr(C)]
pub struct Decimal {
    reserved: u16,
    scale: u8,
    sign: u8,
    hi: u32,
    lo: u64,
}

/// VarCyFromDec: the value times 10000, rounded half to even (banker's
/// rounding, as Automation's conversions do).
#[unsafe(no_mangle)]
pub extern "system" fn VarCyFromDec(dec: *const Decimal, out: *mut i64) -> HResult {
    if dec.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's DECIMAL.
    let d = unsafe { &*dec };
    if d.scale > 28 {
        return E_INVALIDARG;
    }
    let magnitude = (u128::from(d.hi) << 64) | u128::from(d.lo);
    // value * 10^4 / 10^scale.
    let (mut q, divisor) = if d.scale <= 4 {
        (magnitude.checked_mul(10u128.pow(u32::from(4 - d.scale))), 1u128)
    } else {
        (Some(magnitude), 10u128.pow(u32::from(d.scale - 4)))
    };
    if divisor > 1 {
        q = q.map(|m| {
            let (whole, rest) = (m / divisor, m % divisor);
            let twice = rest * 2;
            if twice > divisor || (twice == divisor && whole % 2 == 1) { whole + 1 } else { whole }
        });
    }
    let Some(q) = q.filter(|&q| q <= i64::MAX as u128 + u128::from(d.sign & 0x80 != 0)) else {
        return DISP_E_OVERFLOW;
    };
    let v = if d.sign & 0x80 != 0 { (q as i128).wrapping_neg() as i64 } else { q as i64 };
    put(out, v);
    S_OK
}

// ---- Type libraries and records ----

fn clear_out(out: *mut *mut c_void) {
    if !out.is_null() {
        put(out, core::ptr::null_mut());
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn LoadRegTypeLib(
    _guid: *const u8,
    major: u16,
    _minor: u16,
    _lcid: u32,
    out: *mut *mut c_void,
) -> HResult {
    clear_out(out);
    missing(b"oleaut32.dll!LoadRegTypeLib(no registry)", major.into());
    TYPE_E_LIBNOTREGISTERED
}

#[unsafe(no_mangle)]
pub extern "system" fn QueryPathOfRegTypeLib(
    _guid: *const u8,
    major: u16,
    _minor: u16,
    _lcid: u32,
    path: *mut *mut u16,
) -> HResult {
    if !path.is_null() {
        put(path, core::ptr::null_mut());
    }
    missing(b"oleaut32.dll!QueryPathOfRegTypeLib(no registry)", major.into());
    TYPE_E_LIBNOTREGISTERED
}

#[unsafe(no_mangle)]
pub extern "system" fn LoadTypeLibEx(
    _file: *const u16,
    kind: u32,
    out: *mut *mut c_void,
) -> HResult {
    clear_out(out);
    missing(b"oleaut32.dll!LoadTypeLibEx", kind.into());
    TYPE_E_CANTLOADLIBRARY
}

#[unsafe(no_mangle)]
pub extern "system" fn LoadTypeLib(file: *const u16, out: *mut *mut c_void) -> HResult {
    LoadTypeLibEx(file, 1, out)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetRecordInfoFromTypeInfo(
    info: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    if info.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    clear_out(out);
    missing(b"oleaut32.dll!GetRecordInfoFromTypeInfo", 0);
    E_NOTIMPL
}

#[unsafe(no_mangle)]
pub extern "system" fn GetRecordInfoFromGuids(
    _lib: *const u8,
    major: u32,
    _minor: u32,
    _lcid: u32,
    _guid: *const u8,
    out: *mut *mut c_void,
) -> HResult {
    clear_out(out);
    missing(b"oleaut32.dll!GetRecordInfoFromGuids(no registry)", major.into());
    TYPE_E_LIBNOTREGISTERED
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
