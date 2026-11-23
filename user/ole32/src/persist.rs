//! Putting an object in a stream and getting it back: WriteClassStm,
//! ReadClassStm, OleSaveToStream and OleLoadFromStream.
//!
//! A persisted object is its class id and then whatever the object
//! writes: `OleSaveToStream` asks the object for its class id, writes
//! that, and calls `IPersistStream::Save`; `OleLoadFromStream` reads the
//! class id, activates it, and calls `IPersistStream::Load`. Word does
//! the second while it starts up (ROADMAP OF), where every one of these
//! was a missing export.
//!
//! What they do is measured on Windows 11 (`ole32_test`), because the
//! documentation says next to nothing about the order of the calls or
//! what is left behind when one of them refuses:
//!
//! - WriteClassStm writes the sixteen bytes of the CLSID as it sits in
//!   memory, at the stream's position, and leaves the stream after them.
//! - ReadClassStm zeroes the caller's CLSID first; a stream with less
//!   than sixteen bytes left is STG_E_READFAULT, with the CLSID left
//!   zeroed and the stream after what there was.
//! - OleSaveToStream makes two calls on the object and no others
//!   (GetClassID, then Save with fClearDirty TRUE, which sees the stream
//!   after the class id): no IsDirty, no GetSizeMax, no QueryInterface,
//!   and no reference held. GetClassID refusing is that error with
//!   nothing written; Save refusing is that error with the class id left
//!   in the stream. A class id of CLSID_NULL is saved like any other.
//! - OleLoadFromStream makes four calls: the class object is asked to
//!   make the caller's own interface, that object is asked for
//!   IPersistStream, Load is given the stream after the class id, and
//!   the caller's interface is asked for once more -- that last
//!   reference is the caller's. An empty stream is STG_E_READFAULT, a
//!   class that is not registered REGDB_E_CLASSNOTREG, one with no
//!   IPersistStream E_NOINTERFACE, and Load refusing is its own error
//!   with the object released and the caller's pointer zeroed.

use core::ffi::c_void;

use super::{E_INVALIDARG, E_POINTER, Guid, HResult, S_OK, com_call, missing, put, release};

const STG_E_WRITEFAULT: HResult = 0x8003_001du32 as i32;
const STG_E_READFAULT: HResult = 0x8003_001eu32 as i32;
const CLSCTX_INPROC_SERVER: u32 = 0x1;
const GUID_NULL: Guid = [0; 16];
/// IID_IPersistStream {00000109-0000-0000-C000-000000000046}.
const IID_IPERSISTSTREAM: Guid = super::ole_iid(0x109);

/// The sixteen bytes of a class id, as both calls move them.
const CLSID_BYTES: u32 = 16;

/// WriteClassStm(pStm, rclsid): the class id at the stream's position.
#[unsafe(no_mangle)]
pub extern "system" fn WriteClassStm(stream: *mut c_void, clsid: *const Guid) -> HResult {
    if stream.is_null() || clsid.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's CLSID.
    let id = unsafe { *clsid };
    let mut written = 0u32;
    // IStream::Write.
    let hr = com_call!(
        stream,
        4,
        fn(*const c_void, u32, *mut u32) -> HResult,
        id.as_ptr().cast(),
        CLSID_BYTES,
        &mut written
    );
    if hr < 0 {
        return hr;
    }
    if written != CLSID_BYTES {
        missing(b"ole32.dll!WriteClassStm(the stream took part of the class id)", written.into());
        return STG_E_WRITEFAULT;
    }
    S_OK
}

/// ReadClassStm(pStm, pclsid): the class id at the stream's position,
/// or STG_E_READFAULT with the caller's class id zeroed.
#[unsafe(no_mangle)]
pub extern "system" fn ReadClassStm(stream: *mut c_void, clsid: *mut Guid) -> HResult {
    if stream.is_null() || clsid.is_null() {
        return E_INVALIDARG;
    }
    // Zeroed before the read, and so left zeroed by a short one.
    put(clsid, GUID_NULL);
    let mut id = GUID_NULL;
    let mut got = 0u32;
    // IStream::Read.
    let hr = com_call!(
        stream,
        3,
        fn(*mut c_void, u32, *mut u32) -> HResult,
        id.as_mut_ptr().cast(),
        CLSID_BYTES,
        &mut got
    );
    if hr < 0 {
        return hr;
    }
    if got != CLSID_BYTES {
        return STG_E_READFAULT;
    }
    put(clsid, id);
    S_OK
}

/// OleSaveToStream(pPStm, pStm): the object's class id, then whatever
/// its IPersistStream::Save writes.
#[unsafe(no_mangle)]
pub extern "system" fn OleSaveToStream(persist: *mut c_void, stream: *mut c_void) -> HResult {
    if persist.is_null() || stream.is_null() {
        return E_INVALIDARG;
    }
    let mut id = GUID_NULL;
    // IPersist::GetClassID.
    let hr = com_call!(persist, 3, fn(*mut Guid) -> HResult, &mut id);
    if hr < 0 {
        return hr;
    }
    let hr = WriteClassStm(stream, &id);
    if hr < 0 {
        return hr;
    }
    // IPersistStream::Save(pStm, fClearDirty TRUE). Nothing is undone
    // when it refuses: the class id stays, as on Windows.
    com_call!(persist, 6, fn(*mut c_void, i32) -> HResult, stream, 1)
}

/// OleLoadFromStream(pStm, iidInterface, ppvObj): the object the stream
/// was saved from, loaded and asked for `iidInterface`.
#[unsafe(no_mangle)]
pub extern "system" fn OleLoadFromStream(
    stream: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if stream.is_null() || iid.is_null() {
        return E_INVALIDARG;
    }
    let mut id = GUID_NULL;
    let hr = ReadClassStm(stream, &mut id);
    if hr < 0 {
        // A stream too short for a class id is a program reading
        // something gala wrote differently, or reading the wrong place:
        // the row says which call found it.
        missing(b"ole32.dll!OleLoadFromStream(no class id in the stream)", 0);
        return hr;
    }
    let detail = u64::from(u32::from_le_bytes([id[0], id[1], id[2], id[3]]));
    let mut object: *mut c_void = core::ptr::null_mut();
    // The class object is asked for the caller's own interface, as on
    // Windows -- so a class that cannot make it is refused here, before
    // anything is loaded. CoCreateInstance records the activation's own
    // gaps (an unregistered class, a server DLL that is not there).
    let hr = super::class::CoCreateInstance(
        &id,
        core::ptr::null_mut(),
        CLSCTX_INPROC_SERVER,
        iid,
        &mut object,
    );
    if hr < 0 {
        return hr;
    }
    let mut persist: *mut c_void = core::ptr::null_mut();
    let hr = com_call!(
        object,
        0,
        fn(*const Guid, *mut *mut c_void) -> HResult,
        &IID_IPERSISTSTREAM,
        &mut persist
    );
    if hr < 0 {
        // The class was activated and cannot be loaded: the row names
        // the class, which is what a program that stops here needs.
        missing(b"ole32.dll!OleLoadFromStream(no IPersistStream)", detail);
        release(object);
        return hr;
    }
    // IPersistStream::Load, given the stream after the class id.
    let hr = com_call!(persist, 5, fn(*mut c_void) -> HResult, stream);
    if hr < 0 {
        release(persist);
        release(object);
        return hr;
    }
    // The caller's interface once more: that reference is the caller's,
    // and the two of ours go back.
    let hr = com_call!(persist, 0, fn(*const Guid, *mut *mut c_void) -> HResult, iid, out);
    release(persist);
    release(object);
    if hr < 0 {
        put(out, core::ptr::null_mut());
    }
    hr
}
