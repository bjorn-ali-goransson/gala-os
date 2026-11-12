//! IWICStream (CreateStream), an IStream over one of:
//! - the caller's memory (InitializeFromMemory): reads stop at its end
//!   (S_OK, fewer bytes), writes go into it and are STG_E_MEDIUMFULL,
//!   writing nothing, when they would pass its end; Seek past the end is
//!   E_INVALIDARG, before the start or with a high dword set
//!   HRESULT_FROM_WIN32(ERROR_ARITHMETIC_OVERFLOW); SetSize, CopyTo,
//!   Revert, Lock/UnlockRegion and Clone are E_NOTIMPL, Commit S_OK;
//!   Stat gives the size, no name, type STGTY_STREAM and zero for the
//!   rest (Wine's stream test, run on Windows);
//! - a file (InitializeFromFilename with GENERIC_READ): read whole when
//!   it is opened and then served as memory (other access is not
//!   supported: recorded, E_NOTIMPL);
//! - another IStream (InitializeFromIStream): every call is passed on.
//!
//! A stream initialized once is WINCODEC_ERR_WRONGSTATE to initialize
//! again; one never initialized is WINCODEC_ERR_NOTINITIALIZED.
//! InitializeFromIStreamRegion is not implemented (recorded).
//!
//! `read_all` is the decoders' reader of any IStream.

use alloc::vec::Vec;

use crate::{
    E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, Guid, HResult, Header, Obj, S_OK, Spin, VTable,
    WINCODEC_ERR_NOTINITIALIZED, WINCODEC_ERR_WRONGSTATE, add_ref, gap, guid, not_implemented, put,
    query, release_object, slot, try_box,
};

const STG_E_MEDIUMFULL: HResult = 0x8003_0070_u32 as i32;
/// HRESULT_FROM_WIN32(ERROR_ARITHMETIC_OVERFLOW).
const ARITHMETIC_OVERFLOW: HResult = 0x8007_0216_u32 as i32;
const STREAM_SEEK_SET: u32 = 0;
const STREAM_SEEK_CUR: u32 = 1;
const STREAM_SEEK_END: u32 = 2;
const STGTY_STREAM: u32 = 2;
const GENERIC_READ: u32 = 0x8000_0000;
/// The largest stream the decoders read (a guard against a stream that
/// never ends).
const MAX_STREAM: usize = 512 << 20;

const IID_ISEQUENTIALSTREAM: Guid =
    guid(0x0c73_3a30, 0x2a1c, 0x11ce, [0xad, 0xe5, 0x00, 0xaa, 0x00, 0x44, 0x77, 0x3d]);
const IID_ISTREAM: Guid = guid(0x0000_000c, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IWICSTREAM: Guid =
    guid(0x135f_f860, 0x22b7, 0x4ddf, [0xb0, 0xf6, 0x21, 0x8f, 0x4f, 0x29, 0x9a, 0x43]);

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: usize,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> usize;
    fn ReadFile(h: usize, buf: *mut u8, len: u32, read: *mut u32, overlapped: usize) -> i32;
    fn CloseHandle(h: usize) -> i32;
    fn GetLastError() -> u32;
}

enum Backing {
    None,
    /// The caller's memory.
    Memory {
        base: *mut u8,
        len: usize,
        pos: usize,
    },
    /// A file's contents.
    Owned {
        data: Vec<u8>,
        pos: usize,
    },
    /// Another IStream (a reference held).
    Stream(Obj),
}
// SAFETY: the caller's memory and stream are the caller's to share.
unsafe impl Send for Backing {}

#[repr(C)]
pub struct Stream {
    h: Header,
    state: Spin<Backing>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        if let Backing::Stream(s) = self.state.with(|b| core::mem::replace(b, Backing::None)) {
            crate::Source(s).release();
        }
    }
}

static STREAM_VTBL: VTable<18> = VTable([
    stream_query as *const (),
    add_ref as *const (),
    stream_release as *const (),
    read as *const (),
    write as *const (),
    seek as *const (),
    set_size as *const (),
    copy_to as *const (),
    commit as *const (),
    revert as *const (),
    lock_region as *const (),
    unlock_region as *const (),
    stat as *const (),
    clone as *const (),
    initialize_from_istream as *const (),
    initialize_from_filename as *const (),
    initialize_from_memory as *const (),
    initialize_from_istream_region as *const (),
]);

/// IWICImagingFactory::CreateStream.
pub fn create(out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    match try_box(Stream { h: Header::new(STREAM_VTBL.addr()), state: Spin::new(Backing::None) }) {
        Some(s) => {
            put(out, s as Obj);
            S_OK
        }
        None => E_OUTOFMEMORY,
    }
}

fn this<'a>(o: Obj) -> &'a Stream {
    // SAFETY: one of our streams.
    unsafe { &*(o as *const Stream) }
}

extern "system" fn stream_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    query(this, &[IID_IWICSTREAM, IID_ISTREAM, IID_ISEQUENTIALSTREAM], riid, out)
}

extern "system" fn stream_release(this: Obj) -> u32 {
    release_object::<Stream>(this)
}

/// What a call does with the backing: memory (its bytes and position),
/// another stream, or nothing yet.
enum Target<'a> {
    Bytes(&'a mut [u8], &'a mut usize, bool),
    Inner(Obj),
    Nothing,
}

fn with_target<R>(o: Obj, f: impl FnOnce(Target) -> R) -> R {
    let inner = this(o).state.with(|b| match b {
        Backing::Stream(s) => Some(*s),
        _ => None,
    });
    if let Some(s) = inner {
        // Called outside the lock: the other stream may take its time.
        return f(Target::Inner(s));
    }
    this(o).state.with(|b| match b {
        Backing::None | Backing::Stream(_) => f(Target::Nothing),
        Backing::Memory { base, len, pos } => {
            // SAFETY: the caller's memory of `len` bytes.
            let bytes = unsafe { core::slice::from_raw_parts_mut(*base, *len) };
            f(Target::Bytes(bytes, pos, true))
        }
        Backing::Owned { data, pos } => f(Target::Bytes(&mut data[..], pos, false)),
    })
}

type FnRead = extern "system" fn(Obj, *mut u8, u32, *mut u32) -> HResult;
type FnSeek = extern "system" fn(Obj, i64, u32, *mut u64) -> HResult;

extern "system" fn read(this: Obj, buf: *mut u8, n: u32, done: *mut u32) -> HResult {
    with_target(this, |t| match t {
        Target::Inner(s) => {
            // SAFETY: ISequentialStream::Read.
            let f: FnRead = unsafe { core::mem::transmute(slot(s, 3)) };
            f(s, buf, n, done)
        }
        Target::Nothing => WINCODEC_ERR_NOTINITIALIZED,
        Target::Bytes(bytes, pos, _) => {
            if buf.is_null() {
                return E_INVALIDARG;
            }
            let k = (n as usize).min(bytes.len() - *pos);
            // SAFETY: the caller's buffer of `n` bytes (it may overlap
            // the stream's memory).
            unsafe { core::ptr::copy(bytes.as_ptr().add(*pos), buf, k) };
            *pos += k;
            put(done, k as u32);
            S_OK
        }
    })
}

extern "system" fn write(this: Obj, buf: *const u8, n: u32, done: *mut u32) -> HResult {
    with_target(this, |t| match t {
        Target::Inner(s) => {
            // SAFETY: ISequentialStream::Write (Read's shape).
            let f: extern "system" fn(Obj, *const u8, u32, *mut u32) -> HResult =
                unsafe { core::mem::transmute(slot(s, 4)) };
            f(s, buf, n, done)
        }
        Target::Nothing => WINCODEC_ERR_NOTINITIALIZED,
        Target::Bytes(bytes, pos, writable) => {
            if buf.is_null() {
                return E_INVALIDARG;
            }
            if !writable {
                gap(b"IWICStream::Write(file)", 0);
                return E_NOTIMPL;
            }
            if n as usize > bytes.len() - *pos {
                return STG_E_MEDIUMFULL;
            }
            // SAFETY: the caller's `n` bytes (they may overlap).
            unsafe { core::ptr::copy(buf, bytes.as_mut_ptr().add(*pos), n as usize) };
            *pos += n as usize;
            put(done, n);
            S_OK
        }
    })
}

extern "system" fn seek(this: Obj, offset: i64, origin: u32, new: *mut u64) -> HResult {
    with_target(this, |t| match t {
        Target::Inner(s) => {
            // SAFETY: IStream::Seek.
            let f: FnSeek = unsafe { core::mem::transmute(slot(s, 5)) };
            f(s, offset, origin, new)
        }
        Target::Nothing => WINCODEC_ERR_NOTINITIALIZED,
        Target::Bytes(bytes, pos, _) => {
            let from = match origin {
                STREAM_SEEK_SET => 0,
                STREAM_SEEK_CUR => *pos as i64,
                STREAM_SEEK_END => bytes.len() as i64,
                _ => return E_INVALIDARG,
            };
            let to = from.saturating_add(offset);
            if to < 0 || to > u32::MAX as i64 {
                return ARITHMETIC_OVERFLOW;
            }
            if to as usize > bytes.len() {
                return E_INVALIDARG;
            }
            *pos = to as usize;
            put(new, to as u64);
            S_OK
        }
    })
}

/// The IStream methods a memory stream does not have (passed on to
/// another stream).
macro_rules! forward_or_notimpl {
    ($name:ident, $slot:expr, ($($arg:ident: $t:ty),*)) => {
        extern "system" fn $name(this: Obj, $($arg: $t),*) -> HResult {
            with_target(this, |t| match t {
                Target::Inner(s) => {
                    // SAFETY: the IStream method in slot $slot, whose
                    // arguments these are.
                    let f: extern "system" fn(Obj, $($t),*) -> HResult =
                        unsafe { core::mem::transmute(slot(s, $slot)) };
                    f(s, $($arg),*)
                }
                Target::Nothing => WINCODEC_ERR_NOTINITIALIZED,
                Target::Bytes(..) => E_NOTIMPL,
            })
        }
    };
}

forward_or_notimpl!(set_size, 6, (size: u64));
forward_or_notimpl!(copy_to, 7, (to: Obj, n: u64, read: *mut u64, written: *mut u64));
forward_or_notimpl!(revert, 9, ());
forward_or_notimpl!(lock_region, 10, (offset: u64, n: u64, kind: u32));
forward_or_notimpl!(unlock_region, 11, (offset: u64, n: u64, kind: u32));
forward_or_notimpl!(clone, 13, (out: *mut Obj));

extern "system" fn commit(this: Obj, flags: u32) -> HResult {
    with_target(this, |t| match t {
        Target::Inner(s) => {
            // SAFETY: IStream::Commit.
            let f: extern "system" fn(Obj, u32) -> HResult =
                unsafe { core::mem::transmute(slot(s, 8)) };
            f(s, flags)
        }
        Target::Nothing => WINCODEC_ERR_NOTINITIALIZED,
        Target::Bytes(..) => S_OK,
    })
}

/// STATSTG.
#[repr(C)]
struct StatStg {
    name: *mut u16,
    kind: u32,
    size: u64,
    mtime: u64,
    ctime: u64,
    atime: u64,
    mode: u32,
    locks: u32,
    clsid: Guid,
    state: u32,
    reserved: u32,
}

extern "system" fn stat(this: Obj, out: *mut StatStg, flags: u32) -> HResult {
    with_target(this, |t| match t {
        Target::Inner(s) => {
            // SAFETY: IStream::Stat.
            let f: extern "system" fn(Obj, *mut StatStg, u32) -> HResult =
                unsafe { core::mem::transmute(slot(s, 12)) };
            f(s, out, flags)
        }
        Target::Nothing => WINCODEC_ERR_NOTINITIALIZED,
        Target::Bytes(bytes, _, _) => {
            if out.is_null() {
                return E_INVALIDARG;
            }
            put(
                out,
                StatStg {
                    name: core::ptr::null_mut(),
                    kind: STGTY_STREAM,
                    size: bytes.len() as u64,
                    mtime: 0,
                    ctime: 0,
                    atime: 0,
                    mode: 0,
                    locks: 0,
                    clsid: [0; 16],
                    state: 0,
                    reserved: 0,
                },
            );
            S_OK
        }
    })
}

/// Sets the backing of a stream not yet initialized.
fn initialize(o: Obj, b: Backing) -> HResult {
    let refused = this(o).state.with(|s| {
        if matches!(s, Backing::None) {
            *s = b;
            None
        } else {
            Some(b)
        }
    });
    match refused {
        None => S_OK,
        Some(Backing::Stream(s)) => {
            crate::Source(s).release();
            WINCODEC_ERR_WRONGSTATE
        }
        Some(_) => WINCODEC_ERR_WRONGSTATE,
    }
}

extern "system" fn initialize_from_memory(this: Obj, base: *mut u8, len: u32) -> HResult {
    if base.is_null() {
        return E_INVALIDARG;
    }
    initialize(this, Backing::Memory { base, len: len as usize, pos: 0 })
}

extern "system" fn initialize_from_istream(this: Obj, other: Obj) -> HResult {
    if other.is_null() {
        return E_INVALIDARG;
    }
    crate::Source(other).add_ref();
    initialize(this, Backing::Stream(other))
}

extern "system" fn initialize_from_istream_region(
    _this: Obj,
    _other: Obj,
    _offset: u64,
    _size: u64,
) -> HResult {
    not_implemented(b"IWICStream::InitializeFromIStreamRegion")
}

extern "system" fn initialize_from_filename(this: Obj, path: *const u16, access: u32) -> HResult {
    if path.is_null() {
        return E_INVALIDARG;
    }
    if access & !GENERIC_READ != 0 {
        gap(b"IWICStream::InitializeFromFilename(access)", access.into());
        return E_NOTIMPL;
    }
    match read_file(path) {
        Ok(data) => initialize(this, Backing::Owned { data, pos: 0 }),
        Err(e) => e,
    }
}

/// HRESULT_FROM_WIN32.
fn from_win32(e: u32) -> HResult {
    if e == 0 { crate::E_FAIL } else { (0x8007_0000 | (e & 0xffff)) as i32 }
}

/// A file's contents (opened for reading, shared for reading).
pub fn read_file(path: *const u16) -> Result<Vec<u8>, HResult> {
    const FILE_SHARE_READ: u32 = 1;
    const OPEN_EXISTING: u32 = 3;
    // SAFETY: the caller's NUL-terminated path.
    let h = unsafe { CreateFileW(path, GENERIC_READ, FILE_SHARE_READ, 0, OPEN_EXISTING, 0, 0) };
    if h == usize::MAX {
        // SAFETY: plain call.
        return Err(from_win32(unsafe { GetLastError() }));
    }
    let r = read_handle(h);
    // SAFETY: our handle.
    unsafe { CloseHandle(h) };
    r
}

/// Reads an open file handle from where it stands to its end.
pub fn read_handle(h: usize) -> Result<Vec<u8>, HResult> {
    let mut data: Vec<u8> = Vec::new();
    loop {
        if data.len() >= MAX_STREAM {
            gap(b"read(file too large)", data.len() as u64);
            return Err(E_OUTOFMEMORY);
        }
        data.try_reserve(64 << 10).map_err(|_| E_OUTOFMEMORY)?;
        let at = data.len();
        let room = (data.capacity() - at).min(1 << 20) as u32;
        let mut got = 0;
        // SAFETY: `room` bytes of spare capacity.
        let ok = unsafe { ReadFile(h, data.as_mut_ptr().add(at), room, &mut got, 0) };
        if ok == 0 {
            // SAFETY: plain call.
            return Err(from_win32(unsafe { GetLastError() }));
        }
        if got == 0 {
            return Ok(data);
        }
        // SAFETY: ReadFile filled `got` bytes.
        unsafe { data.set_len(at + got as usize) };
    }
}

/// Everything in IStream `s` from its start (the decoders' input).
pub fn read_all(s: Obj) -> Result<Vec<u8>, HResult> {
    // SAFETY: IStream::Seek and ISequentialStream::Read.
    let (seek, read): (FnSeek, FnRead) = unsafe {
        (
            core::mem::transmute::<*const (), FnSeek>(slot(s, 5)),
            core::mem::transmute::<*const (), FnRead>(slot(s, 3)),
        )
    };
    let r = seek(s, 0, STREAM_SEEK_SET, core::ptr::null_mut());
    if r < 0 {
        return Err(r);
    }
    let mut data: Vec<u8> = Vec::new();
    loop {
        if data.len() >= MAX_STREAM {
            gap(b"read(stream too large)", data.len() as u64);
            return Err(E_OUTOFMEMORY);
        }
        data.try_reserve(64 << 10).map_err(|_| E_OUTOFMEMORY)?;
        let at = data.len();
        let room = (data.capacity() - at).min(1 << 20) as u32;
        let mut got = 0u32;
        // SAFETY: `room` bytes of spare capacity.
        let r = read(s, unsafe { data.as_mut_ptr().add(at) }, room, &mut got);
        if r < 0 {
            return Err(r);
        }
        let got = got.min(room);
        // SAFETY: Read filled `got` bytes.
        unsafe { data.set_len(at + got as usize) };
        if got == 0 || r == crate::S_FALSE {
            return Ok(data);
        }
    }
}
