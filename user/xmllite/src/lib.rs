//! gala's xmllite.dll: the XmlLite reader (`CreateXmlReader` and
//! `IXmlReader`, in `reader`) over gala's XML scanner, and the writer
//! (`CreateXmlWriter` and `IXmlWriter`, in `writer`) that writes a
//! document out.
//!
//! Word delay-loads the reader, the writer and
//! `CreateXmlWriterOutputWithEncodingName` (`output`, which is a stream
//! with the encoding to write it in); SQL Server LocalDB imports the
//! reader and the writer. What this DLL does
//! not implement fails honestly: E_NOTIMPL, recorded in the ledger as
//! `xmllite.dll!<interface>::<method>` ("not implemented"), and an
//! export with nothing behind it ends the process (`gala_stubs`), so the
//! ledger names what to implement next.
//!
//! The reader's node types, depths, positions and error codes are
//! Windows', and so is every byte the writer writes: all of it is
//! measured in `user/xmllite_test` and read there.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use gala_heap as _;

mod codepages;
mod output;
mod reader;
mod writer;

// The memory routines come from gala's ntdll, as other DLLs on `alloc`
// take them.
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

/// MSVC-target codegen references this marker symbol directly, so it
/// cannot come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn ExitProcess(code: u32) -> !;
}

pub type HResult = i32;
pub type Guid = [u8; 16];

pub const S_OK: HResult = 0;
pub const S_FALSE: HResult = 1;
pub const E_NOTIMPL: HResult = 0x8000_4001u32 as i32;
pub const E_NOINTERFACE: HResult = 0x8000_4002u32 as i32;
pub const E_POINTER: HResult = 0x8000_4003u32 as i32;
pub const E_INVALIDARG: HResult = 0x8007_0057u32 as i32;
pub const E_OUTOFMEMORY: HResult = 0x8007_000eu32 as i32;
pub const E_ACCESSDENIED: HResult = 0x8007_0005u32 as i32;

/// The ledger's kinds (GalaRecordMissing's codes). Kind 6 ("stub")
/// is not among them and must not be: it belongs to an export that
/// reports itself and ends the process (`gala_stubs`), so a row of
/// this DLL's wearing it would name a death that never happened. A
/// call answered as Windows answers it whose effect is absent all the
/// same -- a property taken and acted on nowhere -- is a gap, and
/// `LEDGER_UNSUPPORTED` is its kind.
pub const LEDGER_UNSUPPORTED: u32 = 2;
pub const LEDGER_NOT_IMPLEMENTED: u32 = 3;
pub const LEDGER_OTHER: u32 = 5;
/// Not a gap: the first call that reached something gala answers,
/// recorded so a run says a program got here at all.
pub const LEDGER_REACHED: u32 = 9;

/// What every ledger line of this DLL starts with.
const PREFIX: &str = "xmllite.dll!";
/// The ledger keeps this many characters of a name.
const ROOM: usize = 96;

/// Takes a reference on an object a program handed a property, as the
/// reader and the writer hold one (measured: the object is AddRef'd and
/// never asked for an interface). A null pointer is no object, and
/// holding it costs nothing.
pub(crate) fn add_ref_of(object: Obj) {
    if object.is_null() {
        return;
    }
    // SAFETY: a COM object from the caller; slot 1 of its vtable is
    // IUnknown::AddRef.
    unsafe {
        let vtable = *(object as *const *const usize);
        let f: extern "system" fn(Obj) -> u32 = core::mem::transmute(*vtable.add(1));
        f(object);
    }
}

/// Gives a held reference back.
pub(crate) fn release_of(object: Obj) {
    if object.is_null() {
        return;
    }
    // SAFETY: a COM object the caller handed a property and this DLL
    // took a reference on; slot 2 of its vtable is IUnknown::Release.
    unsafe {
        let vtable = *(object as *const *const usize);
        let f: extern "system" fn(Obj) -> u32 = core::mem::transmute(*vtable.add(2));
        f(object);
    }
}

/// A GUID from its parts, as the SDK writes one.
pub const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

/// A GUID's first field, which names it in a ledger line's detail.
pub fn data1(g: &Guid) -> u64 {
    u64::from(u32::from_le_bytes([g[0], g[1], g[2], g[3]]))
}

/// A COM object pointer, as the interfaces here take and hand out.
pub type Obj = *mut core::ffi::c_void;

/// A COM vtable: an object's first field, and function pointers only.
#[repr(C)]
pub struct VTable<const N: usize>(pub [*const (); N]);
// SAFETY: function pointers only.
unsafe impl<const N: usize> Sync for VTable<N> {}

/// `IUnknown`, which every object answers for.
pub const IID_IUNKNOWN: Guid = guid(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
/// `IStream`, which a stream is asked for first (objidl.idl).
pub const IID_ISTREAM: Guid = guid(0x0000_000c, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
/// `ISequentialStream`: all a reader needs of an input and a writer of
/// an output, for a stream that is no more than that.
pub const IID_ISEQUENTIAL_STREAM: Guid =
    guid(0x0c73_3a30, 0x2a1c, 0x11ce, [0xad, 0xe5, 0x00, 0xaa, 0x00, 0x44, 0x77, 0x3d]);

/// Records `xmllite.dll!<what>` in the ledger (printable ASCII; cut at
/// 96 bytes).
pub fn record(what: &[u8], kind: u32, detail: u64) {
    let mut name = [0u8; ROOM];
    let mut n = 0;
    for &b in PREFIX.as_bytes().iter().chain(what) {
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

/// Records `xmllite.dll!<what> <value>`: a long value keeps its start
/// and ends in `..`.
pub fn record_head(what: &str, value: &str, kind: u32, detail: u64) {
    let mut s = String::from(what);
    s.push(' ');
    let room = ROOM.saturating_sub(PREFIX.len() + s.len());
    let n = value.chars().count();
    if n > room && room > 2 {
        s.extend(value.chars().take(room - 2));
        s.push_str("..");
    } else {
        s.push_str(value);
    }
    record(s.as_bytes(), kind, detail);
}

/// A method gala does not implement: recorded, E_NOTIMPL.
pub fn not_implemented(method: &[u8]) -> HResult {
    record(method, LEDGER_NOT_IMPLEMENTED, 0);
    E_NOTIMPL
}

/// The caller's NUL-terminated wide string as text; unpaired surrogates
/// become U+FFFD. None for NULL or no memory.
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

/// Stores `v` at the caller's output, if any.
pub fn put<T>(out: *mut T, v: T) {
    if !out.is_null() {
        // SAFETY: the caller's output.
        unsafe { out.write(v) };
    }
}

/// Appends `x` to `v`, or False when out of memory.
pub fn push<T>(v: &mut Vec<T>, x: T) -> bool {
    if v.try_reserve(1).is_err() {
        return false;
    }
    v.push(x);
    true
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

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

/// A panic is a bug (or the heap ran out inside the reader): its
/// location goes to the ledger (`xmllite.dll!panic <file>`, the line as
/// detail) and the process ends with STATUS_STACK_BUFFER_OVERRUN
/// (fail-fast), as gala's other DLLs do.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let location = info.location();
    let mut buf = [0u8; 80];
    let mut n = 0;
    let file = location.map_or("", |l| l.file());
    for &b in b"panic ".iter().chain(file.as_bytes()) {
        if n == buf.len() {
            break;
        }
        buf[n] = b;
        n += 1;
    }
    record(&buf[..n], LEDGER_OTHER, location.map_or(0, |l| u64::from(l.line())));
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
