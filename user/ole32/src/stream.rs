//! CreateStreamOnHGlobal: an IStream on memory, as documented for
//! IStream/ISequentialStream (reads stop at the end with S_OK and a short
//! count; writes past the end grow the stream; seeks may go past it).
//! The memory is the process heap rather than an HGLOBAL: a caller's own
//! HGLOBAL is not supported yet (recorded).

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use super::{
    E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, Guid, HResult, IID_IUNKNOWN,
    S_OK, VTable, alloc, free, missing, ole_iid, put,
};

const IID_ISTREAM: Guid = ole_iid(0xc);
/// {0c733a30-2a1c-11ce-ade5-00aa0044773d}
const IID_ISEQUENTIALSTREAM: Guid = [
    0x30, 0x3a, 0x73, 0x0c, 0x1c, 0x2a, 0xce, 0x11, 0xad, 0xe5, 0x00, 0xaa, 0x00, 0x44, 0x77, 0x3d,
];
const STG_E_INVALIDFUNCTION: HResult = 0x8003_0001u32 as i32;
const STG_E_INVALIDPOINTER: HResult = 0x8003_0009u32 as i32;

#[repr(C)]
struct Stream {
    vtable: *const usize,
    refs: AtomicU32,
    data: *mut u8,
    len: usize,
    capacity: usize,
    position: usize,
}

impl Stream {
    /// Room for `size` bytes.
    fn reserve(&mut self, size: usize) -> bool {
        if size <= self.capacity {
            return true;
        }
        let capacity = size.max(self.capacity * 2).max(64);
        let p = alloc(capacity);
        if p.is_null() {
            return false;
        }
        // SAFETY: both buffers hold `len` bytes; the new one is zeroed past it.
        unsafe {
            core::ptr::copy_nonoverlapping(self.data, p, self.len);
            core::ptr::write_bytes(p.add(self.len), 0, capacity - self.len);
        }
        free(self.data);
        self.data = p;
        self.capacity = capacity;
        true
    }
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
    // SAFETY: our object, freed with its last reference.
    unsafe {
        let left = (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            free((*this).data);
            free(this.cast());
        }
        left
    }
}

extern "system" fn read(this: *mut Stream, buf: *mut u8, size: u32, done: *mut u32) -> HResult {
    // SAFETY: our object; the caller's buffer of `size` bytes.
    unsafe {
        let s = &mut *this;
        if buf.is_null() && size > 0 {
            return STG_E_INVALIDPOINTER;
        }
        let n = s.len.saturating_sub(s.position).min(size as usize);
        core::ptr::copy_nonoverlapping(s.data.add(s.position), buf, n);
        s.position += n;
        if !done.is_null() {
            *done = n as u32;
        }
    }
    S_OK
}

extern "system" fn write(this: *mut Stream, buf: *const u8, size: u32, done: *mut u32) -> HResult {
    // SAFETY: our object; the caller's buffer of `size` bytes.
    unsafe {
        let s = &mut *this;
        if buf.is_null() && size > 0 {
            return STG_E_INVALIDPOINTER;
        }
        let end = s.position + size as usize;
        if !s.reserve(end) {
            return E_OUTOFMEMORY;
        }
        core::ptr::copy_nonoverlapping(buf, s.data.add(s.position), size as usize);
        s.position = end;
        s.len = s.len.max(end);
        if !done.is_null() {
            *done = size;
        }
    }
    S_OK
}

/// Seek: STREAM_SEEK_SET (0), CUR (1), END (2).
extern "system" fn seek(this: *mut Stream, distance: i64, origin: u32, new: *mut u64) -> HResult {
    // SAFETY: our object.
    let s = unsafe { &mut *this };
    let base = match origin {
        0 => 0i64,
        1 => s.position as i64,
        2 => s.len as i64,
        _ => return STG_E_INVALIDFUNCTION,
    };
    let Some(target) = base.checked_add(distance).filter(|&t| t >= 0) else {
        return STG_E_INVALIDFUNCTION;
    };
    s.position = target as usize;
    if !new.is_null() {
        put(new, target as u64);
    }
    S_OK
}

extern "system" fn set_size(this: *mut Stream, size: u64) -> HResult {
    // SAFETY: our object.
    let s = unsafe { &mut *this };
    let size = size as usize;
    if !s.reserve(size) {
        return E_OUTOFMEMORY;
    }
    if size < s.len {
        // SAFETY: inside the buffer; bytes past the end read as zero later.
        unsafe { core::ptr::write_bytes(s.data.add(size), 0, s.len - size) };
    }
    s.len = size;
    S_OK
}

extern "system" fn not_impl(_this: *mut Stream) -> HResult {
    missing(b"ole32.dll!IStream(CopyTo/Clone)", 0);
    E_NOTIMPL
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
        *(out.add(16) as *mut u64) = (*this).len as u64;
    }
    S_OK
}

static VTABLE: VTable<14> = VTable([
    query as *const (),
    add_ref as *const (),
    release as *const (),
    read as *const (),
    write as *const (),
    seek as *const (),
    set_size as *const (),
    not_impl as *const (), // CopyTo
    nothing as *const (),  // Commit
    nothing as *const (),  // Revert
    no_locks as *const (), // LockRegion
    no_locks as *const (), // UnlockRegion
    stat as *const (),
    not_impl as *const (), // Clone
]);

#[unsafe(no_mangle)]
pub extern "system" fn CreateStreamOnHGlobal(
    global: usize,
    _delete: i32,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    if global != 0 {
        missing(b"ole32.dll!CreateStreamOnHGlobal(an HGLOBAL)", 0);
        return E_NOTIMPL;
    }
    let p = alloc(core::mem::size_of::<Stream>()).cast::<Stream>();
    if p.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: fresh memory of the object's size.
    unsafe {
        p.write(Stream {
            vtable: VTABLE.0.as_ptr().cast(),
            refs: AtomicU32::new(1),
            data: core::ptr::null_mut(),
            len: 0,
            capacity: 0,
            position: 0,
        })
    };
    put(out, p.cast());
    S_OK
}
