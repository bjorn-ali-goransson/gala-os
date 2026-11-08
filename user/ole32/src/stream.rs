//! CreateStreamOnHGlobal and GetHGlobalFromStream: an IStream on a
//! global memory block, as documented for them and for
//! IStream/ISequentialStream, with the details Wine's conformance tests
//! observe on Windows (`dlls/ole32/tests/hglobalstream.c`):
//!
//! - The stream starts with the block's contents and GlobalSize; reads
//!   stop at the end with S_OK and a short count; writes past the end
//!   grow the block (GlobalReAlloc with GMEM_MOVEABLE, so a GMEM_FIXED
//!   block moves and the stream's handle changes).
//! - Clones share the block and its size (one `Block`, counted), each
//!   with its own position; the block is freed with the last of them
//!   when fDeleteOnRelease.
//! - Positions are 32-bit: Seek takes the move's low 32 bits as signed
//!   and fails with STG_E_SEEKERROR below 0 or past 4 GiB (and for an
//!   unknown origin), still reporting the position; SetSize takes the
//!   low 32 bits.
//! - A block the caller freed reads as empty and fails SetSize and Write
//!   with E_OUTOFMEMORY (GlobalLock/GlobalReAlloc fail).
//!
//! Like Windows' stream, it is not serialized: callers that share one
//! between threads lock it themselves.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use super::{
    E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER, Guid, HResult, IID_IUNKNOWN, S_OK,
    VTable, alloc, com_call, free, ole_iid, put,
};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> usize;
    fn GlobalReAlloc(mem: usize, bytes: usize, flags: u32) -> usize;
    pub(crate) fn GlobalFree(mem: usize) -> usize;
    pub(crate) fn GlobalLock(mem: usize) -> *mut u8;
    pub(crate) fn GlobalUnlock(mem: usize) -> i32;
    fn GlobalSize(mem: usize) -> usize;
}

const GMEM_MOVEABLE: u32 = 0x2;
const GMEM_NODISCARD: u32 = 0x20;
const GMEM_SHARE: u32 = 0x2000;

const IID_ISTREAM: Guid = ole_iid(0xc);
/// {0c733a30-2a1c-11ce-ade5-00aa0044773d}
const IID_ISEQUENTIALSTREAM: Guid = [
    0x30, 0x3a, 0x73, 0x0c, 0x1c, 0x2a, 0xce, 0x11, 0xad, 0xe5, 0x00, 0xaa, 0x00, 0x44, 0x77, 0x3d,
];
const STG_E_INVALIDFUNCTION: HResult = 0x8003_0001u32 as i32;
const STG_E_INVALIDPOINTER: HResult = 0x8003_0009u32 as i32;
const STG_E_SEEKERROR: HResult = 0x8003_0019u32 as i32;

/// The memory block clones share.
#[repr(C)]
struct Block {
    refs: AtomicU32,
    handle: usize,
    size: u32,
    delete: bool,
}

#[repr(C)]
struct Stream {
    vtable: *const usize,
    refs: AtomicU32,
    block: *mut Block,
    position: u32,
}

impl Stream {
    fn block(&mut self) -> &mut Block {
        // SAFETY: the stream holds a reference to its block.
        unsafe { &mut *self.block }
    }
}

impl Block {
    /// Resizes the block (GlobalReAlloc, which may move fixed memory).
    fn resize(&mut self, size: u32) -> bool {
        if size == self.size {
            return true;
        }
        // SAFETY: the stream's block handle.
        let h = unsafe { GlobalReAlloc(self.handle, size as usize, GMEM_MOVEABLE) };
        if h == 0 {
            return false;
        }
        self.handle = h;
        self.size = size;
        true
    }

    /// Runs `f` on the block's memory, locked; None when it cannot be
    /// locked (freed or discarded under the stream).
    fn with_memory<R>(&mut self, f: impl FnOnce(*mut u8) -> R) -> Option<R> {
        // SAFETY: the stream's block handle, unlocked again after `f`.
        let p = unsafe { GlobalLock(self.handle) };
        if p.is_null() {
            return None;
        }
        let r = f(p);
        // SAFETY: as above.
        unsafe { GlobalUnlock(self.handle) };
        Some(r)
    }
}

/// A new stream object on `block` (one more reference to it).
fn new_stream(block: *mut Block, position: u32) -> *mut Stream {
    let p = alloc(core::mem::size_of::<Stream>()).cast::<Stream>();
    if !p.is_null() {
        // SAFETY: fresh memory of the object's size; the block is live.
        unsafe {
            (*block).refs.fetch_add(1, Ordering::Relaxed);
            p.write(Stream {
                vtable: VTABLE.0.as_ptr().cast(),
                refs: AtomicU32::new(1),
                block,
                position,
            });
        }
    }
    p
}

extern "system" fn query(this: *mut Stream, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID.
    match unsafe { *iid } {
        IID_IUNKNOWN | IID_ISTREAM | IID_ISEQUENTIALSTREAM => {
            add_ref(this);
            put(out, this.cast());
            S_OK
        }
        _ => {
            put(out, core::ptr::null_mut());
            E_NOINTERFACE
        }
    }
}

extern "system" fn add_ref(this: *mut Stream) -> u32 {
    // SAFETY: our object.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

extern "system" fn release(this: *mut Stream) -> u32 {
    // SAFETY: our object, freed with its last reference; the block with
    // its last stream.
    unsafe {
        let left = (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            let block = (*this).block;
            if (*block).refs.fetch_sub(1, Ordering::AcqRel) == 1 {
                if (*block).delete {
                    GlobalFree((*block).handle);
                }
                free(block.cast());
            }
            free(this.cast());
        }
        left
    }
}

extern "system" fn read(this: *mut Stream, buf: *mut u8, size: u32, done: *mut u32) -> HResult {
    // SAFETY: our object.
    let s = unsafe { &mut *this };
    if buf.is_null() && size > 0 {
        return STG_E_INVALIDPOINTER;
    }
    let position = s.position;
    let n = s.block().size.saturating_sub(position).min(size);
    let n = if n == 0 {
        0
    } else {
        // SAFETY: the block holds `size` bytes; the caller's buffer `n`.
        let copied = s.block().with_memory(|p| unsafe {
            core::ptr::copy_nonoverlapping(p.add(position as usize), buf, n as usize)
        });
        if copied.is_some() { n } else { 0 }
    };
    s.position += n;
    if !done.is_null() {
        put(done, n);
    }
    S_OK
}

extern "system" fn write(this: *mut Stream, buf: *const u8, size: u32, done: *mut u32) -> HResult {
    // SAFETY: our object.
    let s = unsafe { &mut *this };
    if !done.is_null() {
        put(done, 0);
    }
    if buf.is_null() && size > 0 {
        return STG_E_INVALIDPOINTER;
    }
    if size == 0 {
        return S_OK;
    }
    let position = s.position;
    let Some(end) = position.checked_add(size) else {
        return E_OUTOFMEMORY;
    };
    let block = s.block();
    if end > block.size && !block.resize(end) {
        return E_OUTOFMEMORY;
    }
    // SAFETY: the block holds at least `end` bytes; the caller's buffer
    // `size`.
    let copied = block.with_memory(|p| unsafe {
        core::ptr::copy_nonoverlapping(buf, p.add(position as usize), size as usize)
    });
    if copied.is_none() {
        return E_OUTOFMEMORY;
    }
    s.position = end;
    if !done.is_null() {
        put(done, size);
    }
    S_OK
}

/// Seek: STREAM_SEEK_SET (0), CUR (1), END (2), by the move's low 32
/// bits (signed); the position reported either way.
extern "system" fn seek(this: *mut Stream, distance: i64, origin: u32, new: *mut u64) -> HResult {
    // SAFETY: our object.
    let s = unsafe { &mut *this };
    let base = match origin {
        0 => Some(0i64),
        1 => Some(i64::from(s.position)),
        2 => Some(i64::from(s.block().size)),
        _ => None,
    };
    let target = base
        .map(|b| b + i64::from(distance as i32))
        .filter(|&t| (0..=i64::from(u32::MAX)).contains(&t));
    let r = match target {
        Some(t) => {
            s.position = t as u32;
            S_OK
        }
        None => STG_E_SEEKERROR,
    };
    if !new.is_null() {
        put(new, u64::from(s.position));
    }
    r
}

extern "system" fn set_size(this: *mut Stream, size: u64) -> HResult {
    // SAFETY: our object.
    let s = unsafe { &mut *this };
    if s.block().resize(size as u32) { S_OK } else { E_OUTOFMEMORY }
}

/// CopyTo: reads up to `count` bytes from here and writes them to
/// `target`, reporting what was read and what `target` took.
extern "system" fn copy_to(
    this: *mut Stream,
    target: *mut c_void,
    count: u64,
    read_out: *mut u64,
    written_out: *mut u64,
) -> HResult {
    if target.is_null() {
        return STG_E_INVALIDPOINTER;
    }
    let (mut total_read, mut total_written, mut left) = (0u64, 0u64, count);
    let mut chunk = [0u8; 4096];
    let mut r = S_OK;
    while left > 0 {
        let mut n = 0u32;
        let want = left.min(chunk.len() as u64) as u32;
        read(this, chunk.as_mut_ptr(), want, &mut n);
        if n == 0 {
            break;
        }
        total_read += u64::from(n);
        left -= u64::from(n);
        let mut w = 0u32;
        r = com_call!(
            target,
            4,
            fn(*const u8, u32, *mut u32) -> HResult,
            chunk.as_ptr(),
            n,
            &mut w
        );
        total_written += u64::from(w);
        if r < 0 {
            break;
        }
    }
    if !read_out.is_null() {
        put(read_out, total_read);
    }
    if !written_out.is_null() {
        put(written_out, total_written);
    }
    r
}

extern "system" fn nothing(_this: *mut Stream) -> HResult {
    S_OK
}

extern "system" fn no_locks(_this: *mut Stream) -> HResult {
    STG_E_INVALIDFUNCTION
}

/// Stat: STATSTG with STGTY_STREAM (2) and the size; no name (the
/// stream has none) whatever the flag.
extern "system" fn stat(this: *mut Stream, out: *mut u8, _flag: u32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: our object; the caller's 80-byte STATSTG.
    unsafe {
        core::ptr::write_bytes(out, 0, 80);
        *(out.add(8) as *mut u32) = 2;
        *(out.add(16) as *mut u64) = u64::from((*(*this).block).size);
    }
    S_OK
}

/// Clone: a stream on the same block at the same position.
extern "system" fn clone(this: *mut Stream, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return STG_E_INVALIDPOINTER;
    }
    // SAFETY: our object.
    let (block, position) = unsafe { ((*this).block, (*this).position) };
    let p = new_stream(block, position);
    put(out, p.cast());
    if p.is_null() { E_OUTOFMEMORY } else { S_OK }
}

static VTABLE: VTable<14> = VTable([
    query as *const (),
    add_ref as *const (),
    release as *const (),
    read as *const (),
    write as *const (),
    seek as *const (),
    set_size as *const (),
    copy_to as *const (),
    nothing as *const (),  // Commit
    nothing as *const (),  // Revert
    no_locks as *const (), // LockRegion
    no_locks as *const (), // UnlockRegion
    stat as *const (),
    clone as *const (),
]);

/// CreateStreamOnHGlobal: a stream on `global` (its contents and
/// GlobalSize), or on a new empty moveable block when it is NULL.
#[unsafe(no_mangle)]
pub extern "system" fn CreateStreamOnHGlobal(
    global: usize,
    delete: i32,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    // SAFETY: kernel32 calls on the caller's block or a new one.
    let (handle, size) = unsafe {
        if global == 0 {
            let h = GlobalAlloc(GMEM_MOVEABLE | GMEM_NODISCARD | GMEM_SHARE, 0);
            if h == 0 {
                return E_OUTOFMEMORY;
            }
            (h, 0)
        } else {
            (global, GlobalSize(global).min(u32::MAX as usize) as u32)
        }
    };
    let block = alloc(core::mem::size_of::<Block>()).cast::<Block>();
    if block.is_null() {
        if global == 0 {
            // SAFETY: the block just allocated.
            unsafe { GlobalFree(handle) };
        }
        return E_OUTOFMEMORY;
    }
    // SAFETY: fresh memory of the block's size; the stream takes the
    // only reference (refs starts at 0, new_stream counts it).
    unsafe { block.write(Block { refs: AtomicU32::new(0), handle, size, delete: delete != 0 }) };
    let p = new_stream(block, 0);
    if p.is_null() {
        free(block.cast());
        return E_OUTOFMEMORY;
    }
    put(out, p.cast());
    S_OK
}

/// GetHGlobalFromStream: the block under a CreateStreamOnHGlobal stream;
/// E_INVALIDARG for any other stream or a NULL argument.
#[unsafe(no_mangle)]
pub extern "system" fn GetHGlobalFromStream(stream: *mut c_void, out: *mut usize) -> HResult {
    if stream.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    let s = stream.cast::<Stream>();
    // SAFETY: a COM object starts with its vtable; only ours are read
    // further.
    unsafe {
        if (*s).vtable != VTABLE.0.as_ptr().cast() {
            return E_INVALIDARG;
        }
        put(out, (*(*s).block).handle);
    }
    S_OK
}
