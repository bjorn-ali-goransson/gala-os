//! Putting an object in a stream or a storage and getting it back:
//! WriteClassStm, ReadClassStm, OleSaveToStream, OleLoadFromStream and
//! OleLoad.
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
//! - OleLoad does the same for a storage, and in that order
//!   (`oleload_test`): IStorage::Stat for the class the storage was
//!   saved with, the class activated for the caller's own interface,
//!   and IPersistStorage::Load. A client site turns it into an
//!   activation as well: with one, the object must have IOleObject,
//!   is asked for its OLEMISC bits, and is given the site -- before
//!   the load when it asked for OLEMISC_SETCLIENTSITEFIRST, after it
//!   otherwise; last of all an object with IOleLink is asked to bind
//!   to a source already running, and what it answers is ignored.
//!   The storage's and the site's reference counts are left as they
//!   were, and nothing is cached: two OleLoads of one storage make
//!   two objects.

use core::ffi::c_void;

use super::{E_INVALIDARG, E_POINTER, Guid, HResult, S_OK, com_call, missing, put, release};

const STG_E_WRITEFAULT: HResult = 0x8003_001du32 as i32;
const STG_E_READFAULT: HResult = 0x8003_001eu32 as i32;
const CLSCTX_INPROC_SERVER: u32 = 0x1;
const GUID_NULL: Guid = [0; 16];
const CLSCTX_INPROC_HANDLER: u32 = 0x2;
const REGDB_E_CLASSNOTREG: HResult = 0x8004_0154u32 as i32;
/// IID_IPersistStream {00000109-0000-0000-C000-000000000046}.
const IID_IPERSISTSTREAM: Guid = super::ole_iid(0x109);
/// IID_IPersistStorage {0000010A-...}, IOleObject {00000112-...},
/// IOleLink {0000011D-...}.
const IID_IPERSISTSTORAGE: Guid = super::ole_iid(0x10a);
const IID_IOLEOBJECT: Guid = super::ole_iid(0x112);
const IID_IOLELINK: Guid = super::ole_iid(0x11d);

/// The slots OleLoad calls: IStorage::Stat, IPersistStorage::Load,
/// IOleObject::SetClientSite and ::GetMiscStatus,
/// IOleLink::BindIfRunning.
const STORAGE_STAT: usize = 17;
const PERSISTSTORAGE_LOAD: usize = 6;
const OLEOBJECT_SETCLIENTSITE: usize = 3;
const OLEOBJECT_GETMISCSTATUS: usize = 22;
const OLELINK_BINDIFRUNNING: usize = 10;

/// A STATSTG is 80 bytes on x64 and holds the class id at offset 56;
/// STATFLAG_NONAME (1) is what OleLoad asks for, since it wants only
/// the class. DVASPECT_CONTENT (1) is the aspect the misc bits are
/// asked for, and OLEMISC_SETCLIENTSITEFIRST the one bit they are
/// read for.
const STATSTG_BYTES: usize = 80;
const STATSTG_CLSID: usize = 56;
const STATFLAG_NONAME: u32 = 1;
const DVASPECT_CONTENT: u32 = 1;
const OLEMISC_SETCLIENTSITEFIRST: u32 = 0x2_0000;

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

/// OleLoad(pStg, riid, pClientSite, ppvObj): the object a storage holds,
/// activated for the caller's own interface and loaded from the storage.
///
/// Word's WWLIB.DLL delay-loads this one, and a delay-load gala cannot
/// answer leaves the program holding a null it then calls (ROADMAP OF).
/// Which calls are made, in what order, and which refusals are fatal
/// are measured (`oleload_test`); the one thing gala does not do is
/// ole32's default handler for a class nobody registered, which records
/// its own row instead (ARCHITECTURE "COM: ole32 and oleaut32").
#[unsafe(no_mangle)]
pub extern "system" fn OleLoad(
    storage: *mut c_void,
    iid: *const Guid,
    site: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    // Measured: a NULL output pointer is E_INVALIDARG here, not
    // E_POINTER as in OleLoadFromStream above, and is refused before
    // the storage is touched -- where a NULL riid is refused after the
    // Stat, which is why the two are not checked together.
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    if storage.is_null() {
        return E_INVALIDARG;
    }
    // IStorage::Stat(STATFLAG_NONAME): the class the storage was saved
    // with, and the only call made before anything is activated. A Stat
    // that refuses is that error, with nothing activated.
    let mut stat = [0u8; STATSTG_BYTES];
    let hr = com_call!(
        storage,
        STORAGE_STAT,
        fn(*mut u8, u32) -> HResult,
        stat.as_mut_ptr(),
        STATFLAG_NONAME
    );
    if hr < 0 {
        return hr;
    }
    if iid.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the class id in the 80 bytes the storage filled in; a
    // Guid is sixteen bytes of alignment 1, so the offset needs none.
    let id = unsafe { *stat.as_ptr().add(STATSTG_CLSID).cast::<Guid>() };
    if id == GUID_NULL {
        // The one class Windows gives no object for either, where a
        // class that is merely unregistered gets the default handler.
        return REGDB_E_CLASSNOTREG;
    }
    let detail = u64::from(u32::from_le_bytes([id[0], id[1], id[2], id[3]]));
    let mut object: *mut c_void = core::ptr::null_mut();
    // The class is activated for the caller's own interface, so an
    // object that cannot make it is refused here, before anything is
    // loaded. CoCreateInstance records the activation's own gaps.
    let hr = super::class::CoCreateInstance(
        &id,
        core::ptr::null_mut(),
        CLSCTX_INPROC_SERVER | CLSCTX_INPROC_HANDLER,
        iid,
        &mut object,
    );
    if hr < 0 {
        if hr == REGDB_E_CLASSNOTREG {
            // Windows does not stop here: ole32 puts its own default
            // handler in the class's place, reads 'Ole', 'CompObj' and
            // 'OlePres000' out of the storage, and hands back an object
            // carrying the cached presentation. gala has no default
            // handler, so the row names the class that went unanswered
            // rather than a lie taking its place.
            missing(b"ole32.dll!OleLoad(no default handler for the class)", detail);
        }
        return hr;
    }
    // A client site makes this an activation as well as a load. With
    // one, IOleObject is required: an object that refuses it is never
    // loaded, where the same object without a site loads happily.
    let mut ole_object: *mut c_void = core::ptr::null_mut();
    let mut site_first = false;
    if !site.is_null() {
        let hr = com_call!(
            object,
            0,
            fn(*const Guid, *mut *mut c_void) -> HResult,
            &IID_IOLEOBJECT,
            &mut ole_object
        );
        if hr < 0 {
            release(object);
            return hr;
        }
        let mut status = 0u32;
        // GetMiscStatus refusing is not an error, and the bits such a
        // call wrote are not read: only a status it answered S_OK for
        // decides the order.
        let hr = com_call!(
            ole_object,
            OLEOBJECT_GETMISCSTATUS,
            fn(u32, *mut u32) -> HResult,
            DVASPECT_CONTENT,
            &mut status
        );
        site_first = hr >= 0 && status & OLEMISC_SETCLIENTSITEFIRST != 0;
        if site_first {
            let hr =
                com_call!(ole_object, OLEOBJECT_SETCLIENTSITE, fn(*mut c_void) -> HResult, site);
            if hr < 0 {
                release(ole_object);
                release(object);
                return hr;
            }
        }
    }
    let mut persist: *mut c_void = core::ptr::null_mut();
    let hr = com_call!(
        object,
        0,
        fn(*const Guid, *mut *mut c_void) -> HResult,
        &IID_IPERSISTSTORAGE,
        &mut persist
    );
    if hr < 0 {
        // The class was activated and cannot be loaded: the row names
        // the class, which is what a program that stops here needs.
        missing(b"ole32.dll!OleLoad(no IPersistStorage)", detail);
        if !ole_object.is_null() {
            release(ole_object);
        }
        release(object);
        return hr;
    }
    // IPersistStorage::Load, given the caller's own storage.
    let hr = com_call!(persist, PERSISTSTORAGE_LOAD, fn(*mut c_void) -> HResult, storage);
    release(persist);
    if hr < 0 {
        if !ole_object.is_null() {
            release(ole_object);
        }
        release(object);
        return hr;
    }
    if !ole_object.is_null() {
        if !site_first {
            // The site after the load, which is where an object that
            // asked for nothing special wants it. Refusing is fatal
            // here too, measured: that error, the caller's pointer
            // NULL, and the object that was already loaded let go.
            let hr =
                com_call!(ole_object, OLEOBJECT_SETCLIENTSITE, fn(*mut c_void) -> HResult, site);
            if hr < 0 {
                release(ole_object);
                release(object);
                return hr;
            }
        }
        // Last of all, an object that says it is a link is asked to
        // bind to a source already running. What it answers is
        // ignored, as on Windows: a link whose source is not running
        // is loaded all the same.
        let mut link: *mut c_void = core::ptr::null_mut();
        let hr = com_call!(
            object,
            0,
            fn(*const Guid, *mut *mut c_void) -> HResult,
            &IID_IOLELINK,
            &mut link
        );
        if hr >= 0 && !link.is_null() {
            let _ = com_call!(link, OLELINK_BINDIFRUNNING, fn() -> HResult);
            release(link);
        }
        release(ole_object);
    }
    // The reference CoCreateInstance made is the caller's.
    put(out, object);
    S_OK
}
